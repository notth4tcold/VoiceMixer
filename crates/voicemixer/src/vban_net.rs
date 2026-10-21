//! VBAN network thread: receives audio streams into strips, sends buses as
//! streams, and accepts VBAN-TEXT control scripts.

use rtrb::{Consumer, Producer};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::Duration;
use voicemixer_core::params::{VbanFormat, VbanParams, MAX_CHANNELS};
use voicemixer_core::{vban, Controller};

pub fn start(
    ctl: Controller,
    ins: Vec<Producer<f32>>,
    outs: Vec<Consumer<f32>>,
    rate: u32,
) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("vban".into())
        .spawn(move || Net::new(ctl, ins, outs, rate).run())?;
    Ok(())
}

struct InState {
    resampler: MultiResampler,
}

struct Net {
    ctl: Controller,
    ins: Vec<Producer<f32>>,
    outs: Vec<Consumer<f32>>,
    in_state: Vec<InState>,
    out_frame: Vec<u32>,
    rate: u32,
    cfg: VbanParams,
    cfg_gen: u64,
    socket: Option<(UdpSocket, u16)>,
    decoded: Vec<f32>,
    frame8: Vec<f32>,
}

impl Net {
    fn new(ctl: Controller, ins: Vec<Producer<f32>>, outs: Vec<Consumer<f32>>, rate: u32) -> Self {
        let n_in = ins.len();
        let n_out = outs.len();
        Self {
            ctl,
            ins,
            outs,
            in_state: (0..n_in)
                .map(|_| InState {
                    resampler: MultiResampler::new(rate, rate),
                })
                .collect(),
            out_frame: vec![0; n_out],
            rate,
            cfg: VbanParams::default(),
            cfg_gen: 0,
            socket: None,
            decoded: Vec::new(),
            frame8: Vec::new(),
        }
    }

    fn run(mut self) {
        let mut buf = vec![0u8; 2048];
        loop {
            let g = self.ctl.shared.generation();
            if g != self.cfg_gen {
                self.cfg = self.ctl.shared.params.lock().vban.clone();
                self.cfg_gen = g;
            }
            if !self.cfg.enabled {
                self.socket = None;
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            if self.socket.as_ref().map(|s| s.1) != Some(self.cfg.listen_port) {
                self.socket = bind(self.cfg.listen_port, &self.ctl);
                if self.socket.is_none() {
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
            }
            while let Some(Ok((len, from))) = self
                .socket
                .as_ref()
                .map(|(sock, _)| sock.recv_from(&mut buf))
            {
                let pkt = buf[..len].to_vec();
                self.receive(&pkt, from);
            }
            self.send();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn receive(&mut self, pkt: &[u8], from: SocketAddr) {
        let Some((h, data)) = vban::parse(pkt) else {
            return;
        };
        match h.sub_protocol {
            vban::SUB_AUDIO => {
                let Some(i) = self.cfg.incoming.iter().position(|s| {
                    s.enabled
                        && s.name.eq_ignore_ascii_case(&h.name)
                        && ip_matches(&s.ip, from.ip())
                }) else {
                    return;
                };
                let Some(src_rate) = h.sample_rate() else {
                    return;
                };
                if !vban::decode_audio(&h, data, &mut self.decoded) {
                    return;
                }
                let ch = h.channels as usize;
                let state = &mut self.in_state[i];
                if state.resampler.from != src_rate {
                    state.resampler = MultiResampler::new(src_rate, self.rate);
                }
                self.frame8.clear();
                for f in self.decoded.chunks_exact(ch) {
                    let mut frame = [0f32; MAX_CHANNELS];
                    if ch == 1 {
                        frame[0] = f[0];
                        frame[1] = f[0];
                    } else {
                        let k = ch.min(MAX_CHANNELS);
                        frame[..k].copy_from_slice(&f[..k]);
                    }
                    self.frame8.extend_from_slice(&frame);
                }
                let mut out = Vec::with_capacity(self.frame8.len() + 64);
                state.resampler.process(&self.frame8, &mut out);
                let prod = &mut self.ins[i];
                let n = prod.slots().min(out.len()) / MAX_CHANNELS * MAX_CHANNELS;
                if let Ok(chunk) = prod.write_chunk_uninit(n) {
                    chunk.fill_from_iter(out);
                }
            }
            vban::SUB_TEXT if self.cfg.text_enabled => {
                if !self.cfg.text_stream.is_empty()
                    && !self.cfg.text_stream.eq_ignore_ascii_case(&h.name)
                {
                    return;
                }
                if let Some(text) = vban::text_payload(&h, data) {
                    if let Err(e) = self.ctl.run_script(&text) {
                        log::warn!("VBAN text from {from}: {e}");
                    }
                }
            }
            _ => {}
        }
    }

    fn send(&mut self) {
        let Some((sock, _)) = &self.socket else {
            return;
        };
        let Some(rate_index) = vban::rate_index(self.rate) else {
            return;
        };
        for (i, cons) in self.outs.iter_mut().enumerate() {
            let cfg = &self.cfg.outgoing[i];
            if !cfg.enabled || cfg.ip.is_empty() {
                let k = cons.slots();
                if let Ok(c) = cons.read_chunk(k) {
                    c.commit_all();
                }
                continue;
            }
            let Ok(ip) = cfg.ip.parse::<IpAddr>() else {
                continue;
            };
            let dest = SocketAddr::new(ip, cfg.port);
            let ch = (cfg.channels as usize).clamp(1, MAX_CHANNELS);
            let (data_type, bps) = match cfg.format {
                VbanFormat::Int16 => (vban::DATA_I16, 2),
                VbanFormat::Int24 => (vban::DATA_I24, 3),
                VbanFormat::Float32 => (vban::DATA_F32, 4),
            };
            let frames = vban::frames_per_packet(ch, bps);
            while cons.slots() >= frames * MAX_CHANNELS {
                let Ok(chunk) = cons.read_chunk(frames * MAX_CHANNELS) else {
                    break;
                };
                let samples: Vec<f32> = chunk
                    .into_iter()
                    .enumerate()
                    .filter(|(j, _)| j % MAX_CHANNELS < ch)
                    .map(|(_, v)| v)
                    .collect();
                let h = vban::Header {
                    sub_protocol: vban::SUB_AUDIO,
                    rate_index,
                    samples: frames as u16,
                    channels: ch as u16,
                    data_type,
                    codec: 0,
                    name: cfg.name.clone(),
                    frame: self.out_frame[i],
                };
                self.out_frame[i] = self.out_frame[i].wrapping_add(1);
                let mut pkt = Vec::with_capacity(vban::MAX_PACKET);
                vban::write_header(&h, &mut pkt);
                vban::encode_audio(&samples, data_type, &mut pkt);
                if let Err(e) = sock.send_to(&pkt, dest) {
                    log::warn!("VBAN send to {dest}: {e}");
                    break;
                }
            }
        }
    }
}

fn bind(port: u16, ctl: &Controller) -> Option<(UdpSocket, u16)> {
    match UdpSocket::bind(("0.0.0.0", port)) {
        Ok(s) => {
            let _ = s.set_nonblocking(true);
            Some((s, port))
        }
        Err(e) => {
            *ctl.shared.status.message.lock() =
                format!("VBAN: cannot listen on UDP port {port}: {e}");
            None
        }
    }
}

fn ip_matches(filter: &str, ip: IpAddr) -> bool {
    filter.trim().is_empty() || filter.trim().parse::<IpAddr>().is_ok_and(|f| f == ip)
}

/// Linear resampler for 8-channel interleaved frames.
struct MultiResampler {
    from: u32,
    step: f64,
    pos: f64,
    buf: Vec<f32>,
}

impl MultiResampler {
    fn new(from: u32, to: u32) -> Self {
        Self {
            from,
            step: from as f64 / to.max(1) as f64,
            pos: 0.0,
            buf: Vec::new(),
        }
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.step == 1.0 {
            out.extend_from_slice(input);
            return;
        }
        self.buf.extend_from_slice(input);
        let frames = self.buf.len() / MAX_CHANNELS;
        while self.pos + 1.0 < frames as f64 {
            let i = self.pos as usize;
            let t = (self.pos - i as f64) as f32;
            for c in 0..MAX_CHANNELS {
                let a = self.buf[i * MAX_CHANNELS + c];
                let b = self.buf[(i + 1) * MAX_CHANNELS + c];
                out.push(a + (b - a) * t);
            }
            self.pos += self.step;
        }
        let used = (self.pos as usize).min(frames);
        self.buf.drain(..used * MAX_CHANNELS);
        self.pos -= used as f64;
    }
}
