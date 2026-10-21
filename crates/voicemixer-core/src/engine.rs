//! The mixing engine: strips -> routing matrix -> buses.
//!
//! The audio backend pushes captured audio with [`Engine::push_input`] and
//! asks for playback audio with [`Engine::pull_output`]. Whenever an output
//! runs short, the engine processes another block for *all* buses, so every
//! output stays in step no matter which device callback arrives first.

use crate::dsp::*;
use crate::fifo::Fifo;
use crate::params::*;
use crate::shared::{Meters, Shared};
use rtrb::{Consumer, Producer};
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub const MAX_BLOCK: usize = 512;
const INPUT_FIFO_FRAMES: usize = 16384;
const OUTPUT_FIFO_FRAMES: usize = 16384;

/// Ring buffers connecting the engine to non-realtime helpers.
#[derive(Default)]
pub struct Taps {
    /// Stereo interleaved recording feed, consumed by the recorder thread.
    pub rec: Option<Producer<f32>>,
    /// Stereo interleaved playback at the engine rate, produced by the player thread.
    pub tape: Option<Consumer<f32>>,
    /// Per incoming VBAN stream, 8-channel interleaved at the engine rate.
    pub vban_in: Vec<Consumer<f32>>,
    /// Per outgoing VBAN stream, 8-channel interleaved.
    pub vban_out: Vec<Producer<f32>>,
}

struct StripDsp {
    index: usize,
    channels: usize,
    gain: Smooth,
    mute: Smooth,
    gate: Gate,
    comp: Compressor,
    color: [[Biquad; 3]; 2],
    color_on: bool,
    eq: [[Biquad; 3]; MAX_CHANNELS],
    eq_on: bool,
    chorus: Chorus,
    karaoke: Karaoke,
    limiter: Limiter,
    mono: bool,
    mute_center: bool,
    pan: (f32, f32),
}

impl StripDsp {
    fn new(index: usize, fs: f32) -> Self {
        Self {
            index,
            channels: strip_channels(index),
            gain: Smooth::new(1.0, 20.0, fs),
            mute: Smooth::new(1.0, 10.0, fs),
            gate: Gate::default(),
            comp: Compressor::default(),
            color: [[Biquad::default(); 3]; 2],
            color_on: false,
            eq: [[Biquad::default(); 3]; MAX_CHANNELS],
            eq_on: false,
            chorus: Chorus::new(fs),
            karaoke: Karaoke::default(),
            limiter: Limiter::default(),
            mono: false,
            mute_center: false,
            pan: (0.0, 0.0),
        }
    }

    fn configure(&mut self, p: &StripParams, fs: f32) {
        let virt = is_virtual_strip(self.index);
        self.gain.target = db_to_lin(p.gain_db);
        self.mono = p.mono;
        self.mute_center = virt && p.mute_center;
        self.pan = (p.pan_x.clamp(-1.0, 1.0), p.pan_y.clamp(-1.0, 1.0));
        self.karaoke.configure(p.karaoke, fs);
        self.limiter.configure(p.limit_db, fs);
        if virt {
            let g = p.eq_gain;
            self.eq_on = g.iter().any(|v| v.abs() > 0.01);
            for ch in &mut self.eq {
                ch[0].set(PeqKind::LowShelf, fs, 120.0, 0.707, g[0]);
                ch[1].set(PeqKind::Peak, fs, 1000.0, 0.7, g[1]);
                ch[2].set(PeqKind::HighShelf, fs, 6000.0, 0.707, g[2]);
            }
        } else {
            self.gate.configure(&p.gate, fs);
            self.comp.configure(&p.comp, fs);
            let (x, y) = (p.color_x.clamp(-1.0, 1.0), p.color_y.clamp(-1.0, 1.0));
            self.color_on = x.abs() > 0.001 || y.abs() > 0.001;
            for ch in &mut self.color {
                ch[0].set(PeqKind::LowShelf, fs, 250.0, 0.707, -x * 6.0);
                ch[1].set(PeqKind::HighShelf, fs, 4000.0, 0.707, x * 6.0);
                if y >= 0.0 {
                    ch[2].set(PeqKind::Peak, fs, 3500.0, 1.0, y * 6.0);
                } else {
                    ch[2].set(PeqKind::Peak, fs, 200.0, 0.8, -y * 6.0);
                }
            }
            self.chorus.configure(p.fx_x, p.fx_y, fs);
        }
    }

    fn process(&mut self, buf: &mut [Vec<f32>], n: usize) {
        let ch = self.channels;
        if is_virtual_strip(self.index) {
            if self.eq_on {
                for (c, bands) in self.eq.iter_mut().enumerate().take(ch) {
                    for b in bands.iter_mut() {
                        b.process_slice(&mut buf[c][..n]);
                    }
                }
            }
            if self.mute_center {
                buf[2][..n].fill(0.0);
            }
        } else {
            self.gate.process(buf, ch, n);
            self.comp.process(buf, ch, n);
            if self.color_on {
                for (c, bands) in self.color.iter_mut().enumerate() {
                    for b in bands.iter_mut() {
                        b.process_slice(&mut buf[c][..n]);
                    }
                }
            }
        }
        self.karaoke.process(buf, n);
        if self.mono {
            mono_pairs(buf, ch, n);
        }
        if !is_virtual_strip(self.index) {
            self.chorus.process(buf, n);
        }
        self.apply_position(buf, n);
        for f in 0..n {
            let g = self.gain.tick() * self.mute.tick();
            for c in buf.iter_mut().take(ch) {
                c[f] *= g;
            }
        }
        self.limiter.process(buf, ch, n);
    }

    fn apply_position(&mut self, buf: &mut [Vec<f32>], n: usize) {
        let (x, y) = self.pan;
        if x.abs() < 1e-4 && y.abs() < 1e-4 {
            return;
        }
        let gl = (1.0 - x).min(1.0);
        let gr = (1.0 + x).min(1.0);
        if self.channels == 2 {
            let width = 1.0 + y;
            let (l, r) = buf.split_at_mut(1);
            for f in 0..n {
                let mid = 0.5 * (l[0][f] + r[0][f]);
                let side = 0.5 * (l[0][f] - r[0][f]) * width;
                l[0][f] = (mid + side) * gl;
                r[0][f] = (mid - side) * gr;
            }
        } else {
            let front = (1.0 + y).min(1.0);
            let rear = (1.0 - y).min(1.0);
            let gains = [
                gl * front,
                gr * front,
                front,
                1.0,
                gl * rear,
                gr * rear,
                gl * rear,
                gr * rear,
            ];
            for (c, g) in gains.iter().enumerate() {
                for v in &mut buf[c][..n] {
                    *v *= g;
                }
            }
        }
    }
}

struct BusDsp {
    gain: Smooth,
    mute: Smooth,
    peq: [[Biquad; PEQ_BANDS]; MAX_CHANNELS],
    peq_on: [[bool; PEQ_BANDS]; MAX_CHANNELS],
    eq_on: bool,
    mono: bool,
    mode: BusMode,
}

impl BusDsp {
    fn new(fs: f32) -> Self {
        Self {
            gain: Smooth::new(1.0, 20.0, fs),
            mute: Smooth::new(1.0, 10.0, fs),
            peq: [[Biquad::default(); PEQ_BANDS]; MAX_CHANNELS],
            peq_on: [[false; PEQ_BANDS]; MAX_CHANNELS],
            eq_on: false,
            mono: false,
            mode: BusMode::Normal,
        }
    }

    fn configure(&mut self, p: &BusParams, fs: f32) {
        self.gain.target = db_to_lin(p.gain_db);
        self.mute.target = if p.mute { 0.0 } else { 1.0 };
        self.mono = p.mono;
        self.mode = p.mode;
        self.eq_on = p.eq_on;
        for (c, bands) in p.eq.iter().enumerate().take(MAX_CHANNELS) {
            for (k, b) in bands.iter().enumerate().take(PEQ_BANDS) {
                self.peq[c][k].set(b.kind, fs, b.freq, b.q, b.gain_db);
                self.peq_on[c][k] = b.on && (!b.kind.uses_gain() || b.gain_db.abs() > 0.01);
            }
        }
    }

    fn process(&mut self, buf: &mut [Vec<f32>], n: usize) {
        apply_bus_mode(self.mode, buf, n);
        if self.eq_on {
            for c in 0..MAX_CHANNELS {
                for k in 0..PEQ_BANDS {
                    if self.peq_on[c][k] {
                        self.peq[c][k].process_slice(&mut buf[c][..n]);
                    }
                }
            }
        }
        if self.mono {
            mono_pairs(buf, MAX_CHANNELS, n);
        }
        for f in 0..n {
            let g = self.gain.tick() * self.mute.tick();
            for c in buf.iter_mut() {
                c[f] *= g;
            }
        }
    }
}

pub struct Engine {
    shared: Arc<Shared>,
    fs: f32,
    last_gen: u64,
    p: MixerParams,
    strips: Vec<StripDsp>,
    buses: Vec<BusDsp>,
    sbuf: Vec<Vec<Vec<f32>>>,
    bbuf: Vec<Vec<Vec<f32>>>,
    tape_buf: [Vec<f32>; 2],
    tape_gain: Smooth,
    inputs: Vec<Fifo>,
    outputs: Vec<Fifo>,
    taps: Taps,
    max_input_latency: usize,
}

fn planar(channels: usize) -> Vec<Vec<f32>> {
    (0..channels).map(|_| vec![0.0; MAX_BLOCK]).collect()
}

impl Engine {
    pub fn new(shared: Arc<Shared>, sample_rate: u32, taps: Taps) -> Self {
        let fs = sample_rate as f32;
        let p = shared.snapshot();
        let last_gen = shared.generation();
        let max_input_latency = (p.settings.buffer_frames as usize * 4).max(2048);
        let mut e = Self {
            shared,
            fs,
            last_gen,
            p,
            strips: (0..NUM_STRIPS).map(|i| StripDsp::new(i, fs)).collect(),
            buses: (0..NUM_BUSES).map(|_| BusDsp::new(fs)).collect(),
            sbuf: (0..NUM_STRIPS).map(|_| planar(MAX_CHANNELS)).collect(),
            bbuf: (0..NUM_BUSES).map(|_| planar(MAX_CHANNELS)).collect(),
            tape_buf: [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]],
            tape_gain: Smooth::new(1.0, 20.0, fs),
            inputs: (0..NUM_STRIPS)
                .map(|_| Fifo::new(INPUT_FIFO_FRAMES))
                .collect(),
            outputs: (0..NUM_BUSES)
                .map(|_| Fifo::new(OUTPUT_FIFO_FRAMES))
                .collect(),
            taps,
            max_input_latency,
        };
        e.configure();
        e.shared
            .status
            .sample_rate
            .store(sample_rate, Ordering::Relaxed);
        e
    }

    pub fn sample_rate(&self) -> u32 {
        self.fs as u32
    }

    /// Feeds captured audio into a strip.
    pub fn push_input(&mut self, strip: usize, interleaved: &[f32], channels: usize) {
        if let Some(f) = self.inputs.get_mut(strip) {
            f.push_interleaved(interleaved, channels);
        }
    }

    /// Fills `out` (interleaved, `channels` wide) with the next frames of a bus.
    pub fn pull_output(&mut self, bus: usize, out: &mut [f32], channels: usize) {
        if bus >= NUM_BUSES || channels == 0 {
            out.fill(0.0);
            return;
        }
        let frames = out.len() / channels;
        let have = self.outputs[bus].len();
        // An output that was idle (nobody listening) has piled up stale audio.
        if have > frames * 2 + MAX_BLOCK {
            self.outputs[bus].discard(have - frames);
        }
        let have = self.outputs[bus].len();
        if have < frames {
            self.run(frames - have);
        }
        self.outputs[bus].pop_interleaved(out, channels);
    }

    /// Processes `frames` frames for every bus.
    pub fn run(&mut self, frames: usize) {
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(MAX_BLOCK);
            self.run_block(n);
            done += n;
        }
    }

    fn sync_params(&mut self) {
        let g = self.shared.generation();
        if g == self.last_gen {
            return;
        }
        // Never block the audio thread; if the GUI holds the lock, retry next block.
        let shared = self.shared.clone();
        let Some(guard) = shared.params.try_lock() else {
            return;
        };
        self.p.clone_from(&guard);
        drop(guard);
        self.last_gen = g;
        self.configure();
    }

    fn configure(&mut self) {
        for (dsp, p) in self.strips.iter_mut().zip(&self.p.strips) {
            dsp.configure(p, self.fs);
        }
        for (dsp, p) in self.buses.iter_mut().zip(&self.p.buses) {
            dsp.configure(p, self.fs);
        }
        self.tape_gain.target = db_to_lin(self.p.tape.gain_db);
    }

    fn run_block(&mut self, n: usize) {
        self.sync_params();
        let shared = self.shared.clone();
        let meters = &shared.meters;

        // 1. Inputs, keeping capture latency bounded.
        for s in 0..NUM_STRIPS {
            let fifo = &mut self.inputs[s];
            let excess = fifo.len().saturating_sub(n + self.max_input_latency);
            fifo.discard(excess);
            let got = fifo.pop_planar(&mut self.sbuf[s], MAX_CHANNELS, n);
            if got < n && got > 0 {
                self.shared.status.underruns.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.mix_vban_inputs(n);

        // 2. Strip processing.
        let any_solo = self.p.strips.iter().any(|s| s.solo);
        for s in 0..NUM_STRIPS {
            let sp = &self.p.strips[s];
            let audible = !sp.mute && (!any_solo || sp.solo);
            let dsp = &mut self.strips[s];
            dsp.mute.target = if audible { 1.0 } else { 0.0 };
            dsp.process(&mut self.sbuf[s], n);
            for c in 0..dsp.channels {
                Meters::put(&meters.strips[s][c], peak(&self.sbuf[s][c][..n]));
            }
            if !is_virtual_strip(s) {
                meters.comp_gr[s]
                    .store((-dsp.comp.gain_reduction_db()).to_bits(), Ordering::Relaxed);
                meters.gate_open[s].store(dsp.gate.is_open(), Ordering::Relaxed);
            }
        }

        // 3. Tape playback.
        let tape_frames = self.pull_tape(n);

        // 4. Routing matrix.
        for b in 0..NUM_BUSES {
            let bus = &mut self.bbuf[b];
            for ch in bus.iter_mut() {
                ch[..n].fill(0.0);
            }
            if self.p.buses[b].mode == BusMode::Composite {
                for (k, &code) in self.p.composite.iter().enumerate() {
                    if let Some((s, c)) = composite_source(code) {
                        bus[k][..n].copy_from_slice(&self.sbuf[s][c][..n]);
                    }
                }
            } else {
                for s in 0..NUM_STRIPS {
                    if !self.p.strips[s].routes[b] {
                        continue;
                    }
                    for c in 0..strip_channels(s) {
                        let src = &self.sbuf[s][c][..n];
                        for (d, v) in bus[c][..n].iter_mut().zip(src) {
                            *d += v;
                        }
                    }
                }
            }
            if tape_frames > 0 && self.p.tape.routes[b] {
                for c in 0..2 {
                    for (d, v) in bus[c][..n].iter_mut().zip(&self.tape_buf[c][..n]) {
                        *d += v;
                    }
                }
            }
        }

        // 5. Bus processing and outputs.
        for b in 0..NUM_BUSES {
            self.buses[b].process(&mut self.bbuf[b], n);
            for c in 0..MAX_CHANNELS {
                Meters::put(&meters.buses[b][c], peak(&self.bbuf[b][c][..n]));
            }
            self.outputs[b].push_planar(&self.bbuf[b], n);
        }

        self.feed_recorder(n);
        self.feed_vban_outputs(n);
    }

    fn mix_vban_inputs(&mut self, n: usize) {
        let enabled = self.p.vban.enabled;
        for (i, cons) in self.taps.vban_in.iter_mut().enumerate() {
            let cfg = &self.p.vban.incoming[i];
            let avail = cons.slots() / MAX_CHANNELS;
            if !enabled || !cfg.enabled {
                if avail > 0 {
                    if let Ok(chunk) = cons.read_chunk(avail * MAX_CHANNELS) {
                        chunk.commit_all();
                    }
                }
                continue;
            }
            // Drop backlog beyond ~4 blocks so network jitter cannot grow latency forever.
            let backlog = avail.saturating_sub(n * 4);
            if backlog > 0 {
                if let Ok(chunk) = cons.read_chunk(backlog * MAX_CHANNELS) {
                    chunk.commit_all();
                }
            }
            let take = (avail - backlog).min(n);
            let s = cfg.strip.min(NUM_STRIPS - 1);
            let ch = strip_channels(s);
            if let Ok(chunk) = cons.read_chunk(take * MAX_CHANNELS) {
                for (j, v) in chunk.into_iter().enumerate() {
                    let (f, c) = (j / MAX_CHANNELS, j % MAX_CHANNELS);
                    if c < ch {
                        self.sbuf[s][c][f] += v;
                    }
                }
            }
        }
    }

    fn pull_tape(&mut self, n: usize) -> usize {
        let Some(cons) = self.taps.tape.as_mut() else {
            return 0;
        };
        if self.shared.tape.flush.swap(false, Ordering::AcqRel) {
            let k = cons.slots();
            if let Ok(chunk) = cons.read_chunk(k) {
                chunk.commit_all();
            }
        }
        let take = (cons.slots() / 2).min(n);
        if take == 0 {
            return 0;
        }
        if let Ok(chunk) = cons.read_chunk(take * 2) {
            for (j, v) in chunk.into_iter().enumerate() {
                self.tape_buf[j % 2][j / 2] = v;
            }
        }
        for c in &mut self.tape_buf {
            c[take..n].fill(0.0);
        }
        for f in 0..n {
            let g = self.tape_gain.tick();
            self.tape_buf[0][f] *= g;
            self.tape_buf[1][f] *= g;
        }
        let meters = &self.shared.meters;
        Meters::put(&meters.tape[0], peak(&self.tape_buf[0][..n]));
        Meters::put(&meters.tape[1], peak(&self.tape_buf[1][..n]));
        self.shared
            .tape
            .played_frames
            .fetch_add(take as u64, Ordering::Relaxed);
        take
    }

    fn feed_recorder(&mut self, n: usize) {
        if !self.shared.tape.recording.load(Ordering::Acquire) {
            return;
        }
        let Some(prod) = self.taps.rec.as_mut() else {
            return;
        };
        let Ok(mut chunk) = prod.write_chunk_uninit(n * 2) else {
            return;
        };
        let t = &self.p.tape;
        let (a, b) = chunk.as_mut_slices();
        let mut write = |j: usize, v: f32| {
            if j < a.len() {
                a[j].write(v);
            } else {
                b[j - a.len()].write(v);
            }
        };
        for f in 0..n {
            let (mut l, mut r) = (0.0, 0.0);
            for s in (0..NUM_STRIPS).filter(|&s| t.arm_strips[s]) {
                l += self.sbuf[s][0][f];
                r += self.sbuf[s][1][f];
            }
            for bus in (0..NUM_BUSES).filter(|&b| t.arm_buses[b]) {
                l += self.bbuf[bus][0][f];
                r += self.bbuf[bus][1][f];
            }
            write(f * 2, l);
            write(f * 2 + 1, r);
        }
        // SAFETY: every one of the n * 2 slots was written above.
        unsafe { chunk.commit_all() };
        self.shared
            .tape
            .recorded_frames
            .fetch_add(n as u64, Ordering::Relaxed);
    }

    fn feed_vban_outputs(&mut self, n: usize) {
        if !self.p.vban.enabled {
            return;
        }
        for (i, prod) in self.taps.vban_out.iter_mut().enumerate() {
            let cfg = &self.p.vban.outgoing[i];
            if !cfg.enabled {
                continue;
            }
            let bus = &self.bbuf[cfg.bus.min(NUM_BUSES - 1)];
            let Ok(mut chunk) = prod.write_chunk_uninit(n * MAX_CHANNELS) else {
                continue;
            };
            let (a, b) = chunk.as_mut_slices();
            for j in 0..n * MAX_CHANNELS {
                let v = bus[j % MAX_CHANNELS][j / MAX_CHANNELS];
                if j < a.len() {
                    a[j].write(v);
                } else {
                    b[j - a.len()].write(v);
                }
            }
            // SAFETY: all n * MAX_CHANNELS slots were written.
            unsafe { chunk.commit_all() };
        }
    }
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(params: MixerParams) -> (Engine, Arc<Shared>) {
        let shared = Arc::new(Shared::new(params));
        (Engine::new(shared.clone(), 48000, Taps::default()), shared)
    }

    fn tone(frames: usize, channels: usize, amp: f32) -> Vec<f32> {
        (0..frames * channels)
            .map(|i| amp * ((i / channels) as f32 * 0.05).sin())
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// Lets parameter smoothers settle by pulling every bus like a device would.
    fn prime(e: &mut Engine) {
        let mut out = vec![0.0; 960];
        for _ in 0..10 {
            for b in 0..NUM_BUSES {
                e.pull_output(b, &mut out, 2);
            }
        }
    }

    #[test]
    fn routes_strip_to_selected_buses_only() {
        let mut p = MixerParams::default();
        p.strips[0].routes = [true, false, false, false, true];
        let (mut e, _) = engine(p);
        e.push_input(0, &tone(1024, 2, 0.5), 2);
        let mut a1 = vec![0.0; 2048];
        let mut a2 = vec![0.0; 2048];
        let mut b2 = vec![0.0; 2048];
        e.pull_output(0, &mut a1, 2);
        e.pull_output(1, &mut a2, 2);
        e.pull_output(4, &mut b2, 2);
        assert!(rms(&a1) > 0.2);
        assert_eq!(rms(&a2), 0.0);
        assert!(rms(&b2) > 0.2);
    }

    #[test]
    fn solo_silences_other_strips() {
        let mut p = MixerParams::default();
        for s in &mut p.strips {
            s.routes = [true, false, false, false, false];
        }
        p.strips[1].solo = true;
        let (mut e, _) = engine(p);
        prime(&mut e);
        e.push_input(0, &tone(1024, 2, 0.5), 2);
        e.push_input(1, &tone(1024, 2, 0.5), 2);
        let mut out = vec![0.0; 2048];
        e.pull_output(0, &mut out, 2);
        // Only the soloed strip is heard: one tone, not two.
        assert!((rms(&out) - rms(&tone(1024, 2, 0.5))).abs() < 0.02);
    }

    #[test]
    fn parameter_changes_reach_the_engine() {
        let (mut e, shared) = engine(MixerParams::default());
        shared.edit(|p| p.strips[3].routes = [false, true, false, false, false]);
        prime(&mut e);
        e.push_input(3, &tone(1024, 2, 0.5), 2);
        let mut a2 = vec![0.0; 2048];
        e.pull_output(1, &mut a2, 2);
        assert!(rms(&a2) > 0.2);
    }

    #[test]
    fn idle_outputs_do_not_accumulate_latency() {
        let (mut e, _) = engine(MixerParams::default());
        let mut out = vec![0.0; 512];
        for _ in 0..50 {
            e.pull_output(0, &mut out, 2);
        }
        e.pull_output(3, &mut out, 2);
        assert!(e.outputs[3].len() <= MAX_BLOCK);
    }

    #[test]
    fn recorder_tap_receives_armed_bus() {
        let shared = Arc::new(Shared::new(MixerParams::default()));
        let (prod, mut cons) = rtrb::RingBuffer::new(1 << 16);
        let mut e = Engine::new(
            shared.clone(),
            48000,
            Taps {
                rec: Some(prod),
                ..Default::default()
            },
        );
        shared.tape.recording.store(true, Ordering::Release);
        shared.edit(|p| p.strips[1].routes = [true, false, false, false, false]);
        e.push_input(1, &tone(2048, 2, 0.5), 2);
        let mut out = vec![0.0; 4096];
        e.pull_output(0, &mut out, 2);
        let n = cons.slots();
        let samples: Vec<f32> = cons.read_chunk(n).unwrap().into_iter().collect();
        assert!(rms(&samples) > 0.1);
    }
}
