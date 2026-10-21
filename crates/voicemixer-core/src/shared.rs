//! State shared between the GUI, control interfaces and the audio thread.

use crate::params::{MixerParams, MAX_CHANNELS, NUM_BUSES, NUM_STRIPS};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

/// How often [`Meters::tick`] drains the per-block peaks into the decaying
/// display values. One ticker thread runs this (see `main.rs`), whether or
/// not a GUI is attached, so meters behave the same in `--headless` mode.
pub const METER_TICK: Duration = Duration::from_millis(33);
/// Per-tick falloff applied to the displayed peak (tuned for `METER_TICK`).
const METER_DECAY: f32 = 0.82;

/// Peak levels written by the engine every audio block, plus the decaying
/// values actually shown to the user. `strips`/`buses`/`tape` only ever grow
/// (via `fetch_max`) until [`Meters::tick`] drains them; `*_display` holds
/// the ballistics-decayed value that the GUI and `voicemixer ctl levels`
/// both just read, so the two never fight over who gets to reset a peak.
pub struct Meters {
    pub strips: [[AtomicU32; MAX_CHANNELS]; NUM_STRIPS],
    pub buses: [[AtomicU32; MAX_CHANNELS]; NUM_BUSES],
    pub tape: [AtomicU32; 2],
    strips_display: [[AtomicU32; MAX_CHANNELS]; NUM_STRIPS],
    buses_display: [[AtomicU32; MAX_CHANNELS]; NUM_BUSES],
    tape_display: [AtomicU32; 2],
    /// Compressor gain reduction per strip (dB, stored negated).
    pub comp_gr: [AtomicU32; NUM_STRIPS],
    pub gate_open: [AtomicBool; NUM_STRIPS],
}

impl Default for Meters {
    fn default() -> Self {
        Self {
            strips: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU32::new(0))),
            buses: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU32::new(0))),
            tape: std::array::from_fn(|_| AtomicU32::new(0)),
            strips_display: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU32::new(0))),
            buses_display: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU32::new(0))),
            tape_display: std::array::from_fn(|_| AtomicU32::new(0)),
            comp_gr: std::array::from_fn(|_| AtomicU32::new(0)),
            gate_open: std::array::from_fn(|_| AtomicBool::new(false)),
        }
    }
}

impl Meters {
    #[inline]
    pub fn put(slot: &AtomicU32, peak: f32) {
        slot.fetch_max(peak.abs().to_bits(), Ordering::Relaxed);
    }

    /// Reads and resets the peak collected since the last call.
    pub fn take(slot: &AtomicU32) -> f32 {
        f32::from_bits(slot.swap(0, Ordering::Relaxed))
    }

    /// Reads a decaying display value without resetting it; any number of
    /// readers (the GUI, `voicemixer ctl levels`) can call this at once.
    #[inline]
    fn load(slot: &AtomicU32) -> f32 {
        f32::from_bits(slot.load(Ordering::Relaxed))
    }

    fn decay_one(accum: &AtomicU32, display: &AtomicU32) {
        let peak = Self::take(accum);
        let prev = Self::load(display);
        display.store(peak.max(prev * METER_DECAY).to_bits(), Ordering::Relaxed);
    }

    /// Drains the peaks accumulated since the last tick into the decaying
    /// display values, applying meter ballistics once centrally. Call this
    /// every [`METER_TICK`] from a single thread (started in `main.rs` for
    /// both the GUI and `--headless` modes) so meters fall back to silence
    /// over time instead of latching at their all-time peak.
    pub fn tick(&self) {
        for s in 0..NUM_STRIPS {
            for c in 0..MAX_CHANNELS {
                Self::decay_one(&self.strips[s][c], &self.strips_display[s][c]);
            }
        }
        for b in 0..NUM_BUSES {
            for c in 0..MAX_CHANNELS {
                Self::decay_one(&self.buses[b][c], &self.buses_display[b][c]);
            }
        }
        for c in 0..2 {
            Self::decay_one(&self.tape[c], &self.tape_display[c]);
        }
    }

    /// Current displayed peak for a strip (both channels combined).
    pub fn strip_level(&self, strip: usize) -> f32 {
        self.strips_display[strip]
            .iter()
            .fold(0.0, |m, s| m.max(Self::load(s)))
    }

    /// Current displayed peak for a bus (all channels combined).
    pub fn bus_level(&self, bus: usize) -> f32 {
        self.buses_display[bus]
            .iter()
            .fold(0.0, |m, s| m.max(Self::load(s)))
    }

    /// Current displayed peak for a strip, per channel (for GUI meters).
    pub fn strip_channel(&self, strip: usize, channel: usize) -> f32 {
        Self::load(&self.strips_display[strip][channel])
    }

    /// Current displayed peak for a bus, per channel (for GUI meters).
    pub fn bus_channel(&self, bus: usize, channel: usize) -> f32 {
        Self::load(&self.buses_display[bus][channel])
    }

    /// Current displayed peak for a tape channel.
    pub fn tape_channel(&self, channel: usize) -> f32 {
        Self::load(&self.tape_display[channel])
    }
}

#[derive(Default)]
pub struct TapeStatus {
    pub playing: AtomicBool,
    pub recording: AtomicBool,
    /// Set by the player to make the engine drop buffered playback audio.
    pub flush: AtomicBool,
    /// Playback position in engine frames, advanced by the engine.
    pub played_frames: AtomicU64,
    pub total_frames: AtomicU64,
    pub recorded_frames: AtomicU64,
    pub file_name: Mutex<String>,
    pub message: Mutex<String>,
}

#[derive(Default)]
pub struct EngineStatus {
    pub running: AtomicBool,
    pub sample_rate: AtomicU32,
    pub remote_clients: AtomicU32,
    /// Engine runs that found an input FIFO short of data.
    pub underruns: AtomicU64,
    pub backend: Mutex<String>,
    pub message: Mutex<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Stable identifier (PipeWire `node.name`).
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Devices {
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
}

impl Devices {
    pub fn describe(list: &[DeviceInfo], name: &str) -> String {
        list.iter()
            .find(|d| d.name == name)
            .map_or_else(|| name.to_string(), |d| d.description.clone())
    }
}

pub struct Shared {
    pub params: Mutex<MixerParams>,
    generation: AtomicU64,
    pub meters: Meters,
    pub tape: TapeStatus,
    pub status: EngineStatus,
    pub devices: Mutex<Devices>,
    pub midi_ports: Mutex<Vec<String>>,
    /// Parameter path waiting for the next MIDI message to bind to.
    pub midi_learn: Mutex<Option<String>>,
}

impl Shared {
    pub fn new(mut params: MixerParams) -> Self {
        params.normalize();
        Self {
            params: Mutex::new(params),
            generation: AtomicU64::new(1),
            meters: Meters::default(),
            tape: TapeStatus::default(),
            status: EngineStatus::default(),
            devices: Mutex::new(Devices::default()),
            midi_ports: Mutex::new(Vec::new()),
            midi_learn: Mutex::new(None),
        }
    }

    /// Increases on every parameter change; the engine reloads when it moves.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn bump(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn edit<R>(&self, f: impl FnOnce(&mut MixerParams) -> R) -> R {
        let r = f(&mut self.params.lock());
        self.bump();
        r
    }

    pub fn snapshot(&self) -> MixerParams {
        self.params.lock().clone()
    }

    pub fn replace(&self, mut params: MixerParams) {
        params.normalize();
        *self.params.lock() = params;
        self.bump();
    }
}
