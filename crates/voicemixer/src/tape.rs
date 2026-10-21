//! Tape recorder and file player, running on their own thread.
//!
//! Playback decodes with Symphonia (WAV, FLAC, MP3, AAC/M4A, Ogg Vorbis,
//! ALAC, and the audio track of MP4/MKV videos), resamples to the engine
//! rate and feeds the engine through a ring buffer. Recording drains the
//! engine's recorder ring into WAV, or into MP3 through the `lame` program.

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use rtrb::{Consumer, Producer};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;
use voicemixer_core::{config, Action, RecFormat, Shared};

const SEEK_STEP_SECS: f64 = 10.0;

pub struct Tape {
    tx: Sender<Action>,
}

impl Tape {
    pub fn start(
        shared: Arc<Shared>,
        play: Producer<f32>,
        rec: Consumer<f32>,
        rate: u32,
    ) -> anyhow::Result<Self> {
        let (tx, rx) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("tape".into())
            .spawn(move || {
                Worker {
                    shared,
                    play,
                    rec,
                    rate,
                    player: None,
                    pending: Vec::new(),
                    recorder: None,
                }
                .run(rx)
            })?;
        Ok(Self { tx })
    }

    pub fn command(&self, a: Action) {
        let _ = self.tx.send(a);
    }
}

struct Player {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    resampler: Resampler,
}

enum Sink {
    Wav(hound::WavWriter<BufWriter<File>>, RecFormat),
    Lame(Child),
}

struct Recorder {
    sink: Sink,
    path: PathBuf,
    paused: bool,
}

struct Worker {
    shared: Arc<Shared>,
    play: Producer<f32>,
    rec: Consumer<f32>,
    rate: u32,
    player: Option<Player>,
    /// Decoded stereo samples that did not fit in the ring yet.
    pending: Vec<f32>,
    recorder: Option<Recorder>,
}

impl Worker {
    fn run(mut self, rx: Receiver<Action>) {
        loop {
            match rx.recv_timeout(Duration::from_millis(5)) {
                Ok(a) => self.handle(a),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if self.shared.tape.playing.load(Ordering::Relaxed) {
                self.fill();
            }
            self.drain_recording();
        }
        self.stop_recording();
    }

    fn message(&self, msg: impl Into<String>) {
        let msg = msg.into();
        log::info!("tape: {msg}");
        *self.shared.tape.message.lock() = msg;
    }

    fn position_secs(&self) -> f64 {
        self.shared.tape.played_frames.load(Ordering::Relaxed) as f64 / self.rate as f64
    }

    fn handle(&mut self, a: Action) {
        let shared = self.shared.clone();
        let t = &shared.tape;
        match a {
            Action::TapeLoad(path) => self.load(Path::new(&path)),
            Action::TapePlay => {
                if self.player.is_none() {
                    let last = self.shared.params.lock().tape.last_file.clone();
                    if !last.is_empty() {
                        self.load(Path::new(&last));
                    }
                }
                if self.player.is_some() {
                    t.playing.store(true, Ordering::Relaxed);
                } else {
                    self.message("Choose a file to play first");
                }
            }
            Action::TapePause => {
                if let Some(r) = &mut self.recorder {
                    r.paused = !r.paused;
                    t.recording.store(!r.paused, Ordering::Release);
                } else if t.playing.swap(false, Ordering::Relaxed) {
                    self.seek_to(self.position_secs());
                }
            }
            Action::TapeStop => {
                if self.recorder.is_some() {
                    self.stop_recording();
                } else if t.playing.swap(false, Ordering::Relaxed) {
                    self.seek_to(self.position_secs());
                } else {
                    self.seek_to(0.0);
                }
            }
            Action::TapeRecord => {
                if let Some(r) = &mut self.recorder {
                    r.paused = !r.paused;
                    t.recording.store(!r.paused, Ordering::Release);
                } else {
                    self.start_recording();
                }
            }
            Action::TapeRewind => self.seek_to((self.position_secs() - SEEK_STEP_SECS).max(0.0)),
            Action::TapeForward => self.seek_to(self.position_secs() + SEEK_STEP_SECS),
            Action::TapeEject => {
                t.playing.store(false, Ordering::Relaxed);
                t.flush.store(true, Ordering::Release);
                self.player = None;
                self.pending.clear();
                t.played_frames.store(0, Ordering::Relaxed);
                t.total_frames.store(0, Ordering::Relaxed);
                t.file_name.lock().clear();
            }
            _ => {}
        }
    }

    fn load(&mut self, path: &Path) {
        let shared = self.shared.clone();
        let t = &shared.tape;
        t.playing.store(false, Ordering::Relaxed);
        t.flush.store(true, Ordering::Release);
        self.pending.clear();
        match open(path, self.rate) {
            Ok((player, total)) => {
                self.player = Some(player);
                t.played_frames.store(0, Ordering::Relaxed);
                t.total_frames.store(total, Ordering::Relaxed);
                *t.file_name.lock() = path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                let p = path.to_string_lossy().into_owned();
                self.shared.edit(|params| params.tape.last_file = p);
                self.message(String::new());
            }
            Err(e) => {
                self.player = None;
                self.message(format!("Cannot play {}: {e:#}", path.display()));
            }
        }
    }

    fn seek_to(&mut self, secs: f64) {
        let Some(p) = &mut self.player else { return };
        let t = &self.shared.tape;
        t.flush.store(true, Ordering::Release);
        self.pending.clear();
        let time = Time::new(secs.trunc() as u64, secs.fract());
        match p.format.seek(
            SeekMode::Coarse,
            SeekTo::Time {
                time,
                track_id: Some(p.track),
            },
        ) {
            Ok(_) => {
                p.decoder.reset();
                p.resampler.reset();
                t.played_frames
                    .store((secs * self.rate as f64) as u64, Ordering::Relaxed);
            }
            Err(e) => self.message(format!("Seek failed: {e}")),
        }
    }

    fn fill(&mut self) {
        loop {
            if !self.pending.is_empty() {
                let n = self.play.slots().min(self.pending.len()) & !1;
                if n == 0 {
                    return;
                }
                if let Ok(chunk) = self.play.write_chunk_uninit(n) {
                    chunk.fill_from_iter(self.pending.drain(..n));
                }
                if !self.pending.is_empty() {
                    return;
                }
            }
            if self.play.slots() < 8192 {
                return;
            }
            let Some(p) = &mut self.player else { return };
            match next_frames(p, &mut self.pending) {
                Ok(true) => {}
                Ok(false) => {
                    let looping = self.shared.params.lock().tape.loop_play;
                    if looping {
                        self.seek_to(0.0);
                        // Keep the tail of the previous pass that is still in the ring.
                        self.shared.tape.flush.store(false, Ordering::Release);
                    } else {
                        self.shared.tape.playing.store(false, Ordering::Relaxed);
                        self.seek_to(0.0);
                        self.shared.tape.flush.store(false, Ordering::Release);
                    }
                    return;
                }
                Err(e) => {
                    self.shared.tape.playing.store(false, Ordering::Relaxed);
                    self.message(format!("Playback error: {e}"));
                    return;
                }
            }
        }
    }

    fn start_recording(&mut self) {
        let (format, kbps, dir) = {
            let p = self.shared.params.lock();
            (p.tape.format, p.tape.mp3_kbps, p.tape.directory.clone())
        };
        let dir = if dir.is_empty() {
            config::recordings_dir()
        } else {
            PathBuf::from(dir)
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.message(format!("Cannot create {}: {e}", dir.display()));
            return;
        }
        let stamp = chrono::Local::now().format("%Y-%m-%d at %Hh%Mm%Ss");
        let base = dir.join(format!("VoiceMixer {stamp}"));
        let sink = match format {
            RecFormat::Mp3 => match spawn_lame(&base.with_extension("mp3"), self.rate, kbps) {
                Ok(child) => Ok((Sink::Lame(child), base.with_extension("mp3"))),
                Err(e) => {
                    self.message(format!(
                        "MP3 needs the 'lame' program ({e}); recording WAV instead"
                    ));
                    wav(&base.with_extension("wav"), self.rate, RecFormat::Wav16)
                }
            },
            f => wav(&base.with_extension("wav"), self.rate, f),
        };
        match sink {
            Ok((sink, path)) => {
                // Discard anything left from an earlier recording.
                let stale = self.rec.slots();
                if let Ok(c) = self.rec.read_chunk(stale) {
                    c.commit_all();
                }
                self.shared.tape.recorded_frames.store(0, Ordering::Relaxed);
                self.message(format!("Recording to {}", path.display()));
                self.recorder = Some(Recorder {
                    sink,
                    path,
                    paused: false,
                });
                self.shared.tape.recording.store(true, Ordering::Release);
            }
            Err(e) => self.message(format!("Cannot record: {e:#}")),
        }
    }

    fn drain_recording(&mut self) {
        let n = self.rec.slots();
        if n == 0 {
            return;
        }
        let Ok(chunk) = self.rec.read_chunk(n) else {
            return;
        };
        let Some(r) = &mut self.recorder else {
            chunk.commit_all();
            return;
        };
        let result: anyhow::Result<()> = match &mut r.sink {
            Sink::Wav(w, f) => chunk.into_iter().try_for_each(|v| {
                let v = v.clamp(-1.0, 1.0);
                match f {
                    RecFormat::Wav24 => w.write_sample((v * 8_388_607.0) as i32),
                    RecFormat::Wav32f => w.write_sample(v),
                    _ => w.write_sample((v * 32767.0) as i16),
                }
                .map_err(Into::into)
            }),
            Sink::Lame(child) => {
                let bytes: Vec<u8> = chunk
                    .into_iter()
                    .flat_map(|v| ((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
                    .collect();
                match child.stdin.as_mut() {
                    Some(stdin) => stdin.write_all(&bytes).map_err(Into::into),
                    None => Err(anyhow::anyhow!("lame closed its input")),
                }
            }
        };
        if let Err(e) = result {
            self.message(format!("Recording failed: {e:#}"));
            self.stop_recording();
        }
    }

    fn stop_recording(&mut self) {
        self.shared.tape.recording.store(false, Ordering::Release);
        self.drain_recording_tail();
        let Some(r) = self.recorder.take() else {
            return;
        };
        let result: anyhow::Result<()> = match r.sink {
            Sink::Wav(w, _) => w.finalize().map_err(Into::into),
            Sink::Lame(mut child) => {
                drop(child.stdin.take());
                child.wait().map(|_| ()).map_err(Into::into)
            }
        };
        match result {
            Ok(()) => self.message(format!("Saved {}", r.path.display())),
            Err(e) => self.message(format!("Could not finish {}: {e:#}", r.path.display())),
        }
    }

    fn drain_recording_tail(&mut self) {
        if self.recorder.is_some() {
            self.drain_recording();
        }
    }
}

fn wav(path: &Path, rate: u32, f: RecFormat) -> anyhow::Result<(Sink, PathBuf)> {
    let (bits, sample_format) = match f {
        RecFormat::Wav24 => (24, hound::SampleFormat::Int),
        RecFormat::Wav32f => (32, hound::SampleFormat::Float),
        _ => (16, hound::SampleFormat::Int),
    };
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: bits,
        sample_format,
    };
    let w = hound::WavWriter::create(path, spec)?;
    Ok((Sink::Wav(w, f), path.to_path_buf()))
}

fn spawn_lame(path: &Path, rate: u32, kbps: u32) -> std::io::Result<Child> {
    let khz = format!("{}", rate as f64 / 1000.0);
    Command::new("lame")
        .args([
            "-r",
            "-s",
            &khz,
            "--bitwidth",
            "16",
            "--signed",
            "--little-endian",
            "-m",
            "j",
            "-b",
            &kbps.to_string(),
            "--quiet",
            "-",
        ])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

fn open(path: &Path, rate: u32) -> anyhow::Result<(Player, u64)> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())
        .ok_or_else(|| anyhow::anyhow!("no audio track"))?;
    let file_rate = track.codec_params.sample_rate.unwrap_or(rate);
    let total = track
        .codec_params
        .n_frames
        .map_or(0, |n| n * rate as u64 / file_rate.max(1) as u64);
    let decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;
    let track_id = track.id;
    Ok((
        Player {
            format,
            decoder,
            track: track_id,
            resampler: Resampler::new(file_rate, rate),
        },
        total,
    ))
}

/// Decodes the next packet as stereo at the engine rate into `out`.
/// Returns Ok(false) at the end of the file.
fn next_frames(p: &mut Player, out: &mut Vec<f32>) -> Result<bool, SymError> {
    loop {
        let packet = match p.format.next_packet() {
            Ok(packet) => packet,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(false)
            }
            Err(SymError::ResetRequired) => return Ok(false),
            Err(e) => return Err(e),
        };
        if packet.track_id() != p.track {
            continue;
        }
        let decoded = match p.decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(e),
        };
        let spec = *decoded.spec();
        let ch = spec.channels.count().max(1);
        let mut sb = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        sb.copy_interleaved_ref(decoded);
        let frames: Vec<[f32; 2]> = sb
            .samples()
            .chunks_exact(ch)
            .map(|f| if ch == 1 { [f[0], f[0]] } else { [f[0], f[1]] })
            .collect();
        p.resampler.process(&frames, out);
        return Ok(true);
    }
}

/// Linear-interpolating stereo resampler. Plenty for monitoring and soundboard
/// playback; not a mastering-grade converter.
struct Resampler {
    step: f64,
    pos: f64,
    buf: Vec<[f32; 2]>,
}

impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        Self {
            step: from as f64 / to.max(1) as f64,
            pos: 0.0,
            buf: Vec::new(),
        }
    }

    fn reset(&mut self) {
        self.pos = 0.0;
        self.buf.clear();
    }

    fn process(&mut self, input: &[[f32; 2]], out: &mut Vec<f32>) {
        self.buf.extend_from_slice(input);
        while self.pos + 1.0 < self.buf.len() as f64 {
            let i = self.pos as usize;
            let t = (self.pos - i as f64) as f32;
            let (a, b) = (self.buf[i], self.buf[i + 1]);
            out.push(a[0] + (b[0] - a[0]) * t);
            out.push(a[1] + (b[1] - a[1]) * t);
            self.pos += self.step;
        }
        let used = (self.pos as usize).min(self.buf.len());
        self.buf.drain(..used);
        self.pos -= used as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::Resampler;

    #[test]
    fn resampler_changes_length_by_ratio() {
        let mut r = Resampler::new(44100, 48000);
        let input: Vec<[f32; 2]> = (0..44100).map(|i| [i as f32, 0.0]).collect();
        let mut out = Vec::new();
        r.process(&input, &mut out);
        let frames = out.len() / 2;
        assert!((frames as i64 - 48000).abs() < 4, "{frames}");
    }
}
