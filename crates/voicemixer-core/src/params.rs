//! Mixer parameter model. Everything the user can change lives here and is
//! saved to the config file; the audio engine works from a copy of it.

use serde::{Deserialize, Serialize};

pub const NUM_HW_STRIPS: usize = 3;
pub const NUM_VIRT_STRIPS: usize = 2;
pub const NUM_STRIPS: usize = NUM_HW_STRIPS + NUM_VIRT_STRIPS;
pub const NUM_HW_BUSES: usize = 3;
pub const NUM_VIRT_BUSES: usize = 2;
pub const NUM_BUSES: usize = NUM_HW_BUSES + NUM_VIRT_BUSES;
/// Channel count of buses and virtual strips (7.1 order: FL FR FC LFE RL RR SL SR).
pub const MAX_CHANNELS: usize = 8;
pub const PEQ_BANDS: usize = 6;
pub const NUM_VBAN_STREAMS: usize = 8;
pub const BUS_NAMES: [&str; NUM_BUSES] = ["A1", "A2", "A3", "B1", "B2"];
pub const GAIN_MIN_DB: f32 = -60.0;
pub const GAIN_MAX_DB: f32 = 12.0;
/// Limit value at or above which the strip limiter is bypassed.
pub const LIMIT_OFF_DB: f32 = 12.0;
/// Highest composite patch source code (see [`composite_source`]).
pub const COMPOSITE_SOURCES: u8 = (NUM_HW_STRIPS * 2 + NUM_VIRT_STRIPS * MAX_CHANNELS) as u8;
pub const PEQ_DEFAULT_FREQS: [f32; PEQ_BANDS] = [50.0, 200.0, 800.0, 2000.0, 8000.0, 12000.0];

pub fn is_virtual_strip(i: usize) -> bool {
    i >= NUM_HW_STRIPS
}

pub fn is_virtual_bus(i: usize) -> bool {
    i >= NUM_HW_BUSES
}

/// Hardware strips are stereo; virtual strips carry up to 7.1.
pub fn strip_channels(i: usize) -> usize {
    if is_virtual_strip(i) {
        MAX_CHANNELS
    } else {
        2
    }
}

pub fn bus_index(name: &str) -> Option<usize> {
    BUS_NAMES.iter().position(|b| b.eq_ignore_ascii_case(name))
}

pub fn default_strip_name(i: usize) -> String {
    match i {
        0..NUM_HW_STRIPS => format!("Hardware Input {}", i + 1),
        NUM_HW_STRIPS => "Virtual Input".into(),
        _ => "Virtual AUX".into(),
    }
}

/// Composite patch codes: 0 = silence, 1..=6 the hardware strip channels
/// (strip 1 L, strip 1 R, ...), 7..=22 the virtual strip channels.
pub fn composite_source(code: u8) -> Option<(usize, usize)> {
    let code = code as usize;
    if code == 0 || code > COMPOSITE_SOURCES as usize {
        return None;
    }
    let idx = code - 1;
    if idx < NUM_HW_STRIPS * 2 {
        Some((idx / 2, idx % 2))
    } else {
        let v = idx - NUM_HW_STRIPS * 2;
        Some((NUM_HW_STRIPS + v / MAX_CHANNELS, v % MAX_CHANNELS))
    }
}

pub fn composite_label(code: u8) -> String {
    const CH: [&str; MAX_CHANNELS] = ["FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR"];
    match composite_source(code) {
        None => "—".into(),
        Some((s, c)) if !is_virtual_strip(s) => {
            format!("In{} {}", s + 1, if c == 0 { "L" } else { "R" })
        }
        Some((s, c)) => format!("V{} {}", s - NUM_HW_STRIPS + 1, CH[c]),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PanelMode {
    #[default]
    Color,
    Position,
    Modulation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompParams {
    /// Front-panel knob, 0..10. Sets the ratio; 0 bypasses the compressor.
    pub amount: f32,
    pub input_gain_db: f32,
    pub threshold_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    /// Soft-knee amount 0..1.
    pub knee: f32,
    pub auto_makeup: bool,
    pub output_gain_db: f32,
}

impl Default for CompParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            input_gain_db: 0.0,
            threshold_db: -24.0,
            attack_ms: 10.0,
            release_ms: 50.0,
            knee: 0.5,
            auto_makeup: true,
            output_gain_db: 0.0,
        }
    }
}

impl CompParams {
    pub fn ratio(&self) -> f32 {
        1.0 + self.amount.clamp(0.0, 10.0) * 0.9
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GateParams {
    /// Front-panel knob, 0..10. Sets the threshold; 0 bypasses the gate.
    pub amount: f32,
    /// Attenuation applied while the gate is closed.
    pub damping_db: f32,
    /// High-pass frequency of the detector, so rumble does not open the gate.
    pub sidechain_hz: f32,
    pub attack_ms: f32,
    pub hold_ms: f32,
    pub release_ms: f32,
}

impl Default for GateParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            damping_db: -60.0,
            sidechain_hz: 100.0,
            attack_ms: 10.0,
            hold_ms: 200.0,
            release_ms: 1250.0,
        }
    }
}

impl GateParams {
    pub fn threshold_db(&self) -> f32 {
        -60.0 + self.amount.clamp(0.0, 10.0) * 5.0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StripParams {
    pub label: String,
    /// PipeWire node name of the capture device (hardware strips only).
    pub device: Option<String>,
    pub gain_db: f32,
    pub mute: bool,
    pub solo: bool,
    pub mono: bool,
    pub routes: [bool; NUM_BUSES],
    /// Position panel, -1..1: x = left/right, y = stereo width (stereo) or front/rear (7.1).
    pub pan_x: f32,
    pub pan_y: f32,
    /// Colour panel, -1..1: x = dark/bright tilt, y = warmth (-) / presence (+).
    pub color_x: f32,
    pub color_y: f32,
    /// Modulation panel: x = rate, y > 0 = chorus depth.
    pub fx_x: f32,
    pub fx_y: f32,
    pub panel: PanelMode,
    /// Virtual strips: bass, medium, treble in dB.
    pub eq_gain: [f32; 3],
    pub comp: CompParams,
    pub gate: GateParams,
    /// Brickwall limiter ceiling in dB; [`LIMIT_OFF_DB`] disables it.
    pub limit_db: f32,
    /// Voice-removal mode, 0 = off, 1..=4.
    pub karaoke: u8,
    /// Mute the centre channel of 5.1/7.1 material.
    pub mute_center: bool,
}

impl Default for StripParams {
    fn default() -> Self {
        Self {
            label: String::new(),
            device: None,
            gain_db: 0.0,
            mute: false,
            solo: false,
            mono: false,
            routes: [true, false, false, false, false],
            pan_x: 0.0,
            pan_y: 0.0,
            color_x: 0.0,
            color_y: 0.0,
            fx_x: 0.0,
            fx_y: 0.0,
            panel: PanelMode::Color,
            eq_gain: [0.0; 3],
            comp: CompParams::default(),
            gate: GateParams::default(),
            limit_db: LIMIT_OFF_DB,
            karaoke: 0,
            mute_center: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BusMode {
    #[default]
    Normal,
    Amix,
    Bmix,
    Repeat,
    Composite,
    TvMix,
    UpMix21,
    UpMix41,
    UpMix61,
    CenterOnly,
    LfeOnly,
    RearOnly,
}

impl BusMode {
    pub const ALL: [BusMode; 12] = [
        BusMode::Normal,
        BusMode::Amix,
        BusMode::Bmix,
        BusMode::Repeat,
        BusMode::Composite,
        BusMode::TvMix,
        BusMode::UpMix21,
        BusMode::UpMix41,
        BusMode::UpMix61,
        BusMode::CenterOnly,
        BusMode::LfeOnly,
        BusMode::RearOnly,
    ];

    /// Name used in scripts (`Bus[0].mode.Amix = 1`).
    pub fn name(self) -> &'static str {
        match self {
            BusMode::Normal => "normal",
            BusMode::Amix => "amix",
            BusMode::Bmix => "bmix",
            BusMode::Repeat => "repeat",
            BusMode::Composite => "composite",
            BusMode::TvMix => "tvmix",
            BusMode::UpMix21 => "upmix21",
            BusMode::UpMix41 => "upmix41",
            BusMode::UpMix61 => "upmix61",
            BusMode::CenterOnly => "centeronly",
            BusMode::LfeOnly => "lfeonly",
            BusMode::RearOnly => "rearonly",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BusMode::Normal => "Normal",
            BusMode::Amix => "Mix Down A",
            BusMode::Bmix => "Mix Down B",
            BusMode::Repeat => "Stereo Repeat",
            BusMode::Composite => "Composite",
            BusMode::TvMix => "TV Mix",
            BusMode::UpMix21 => "Up Mix 2.1",
            BusMode::UpMix41 => "Up Mix 4.1",
            BusMode::UpMix61 => "Up Mix 6.1",
            BusMode::CenterOnly => "Center Only",
            BusMode::LfeOnly => "LFE Only",
            BusMode::RearOnly => "Rear Only",
        }
    }

    pub fn from_name(s: &str) -> Option<BusMode> {
        Self::ALL
            .into_iter()
            .find(|m| m.name().eq_ignore_ascii_case(s))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PeqKind {
    #[default]
    Peak,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
    BandPass,
    Notch,
}

impl PeqKind {
    pub const ALL: [PeqKind; 7] = [
        PeqKind::Peak,
        PeqKind::LowShelf,
        PeqKind::HighShelf,
        PeqKind::LowPass,
        PeqKind::HighPass,
        PeqKind::BandPass,
        PeqKind::Notch,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PeqKind::Peak => "peak",
            PeqKind::LowShelf => "lowshelf",
            PeqKind::HighShelf => "highshelf",
            PeqKind::LowPass => "lowpass",
            PeqKind::HighPass => "highpass",
            PeqKind::BandPass => "bandpass",
            PeqKind::Notch => "notch",
        }
    }

    pub fn from_name(s: &str) -> Option<PeqKind> {
        Self::ALL
            .into_iter()
            .find(|k| k.name().eq_ignore_ascii_case(s))
    }

    /// Whether the gain parameter affects this filter type.
    pub fn uses_gain(self) -> bool {
        matches!(self, PeqKind::Peak | PeqKind::LowShelf | PeqKind::HighShelf)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PeqBand {
    pub on: bool,
    pub kind: PeqKind,
    pub gain_db: f32,
    pub freq: f32,
    pub q: f32,
}

impl Default for PeqBand {
    fn default() -> Self {
        Self {
            on: true,
            kind: PeqKind::Peak,
            gain_db: 0.0,
            freq: 1000.0,
            q: 3.0,
        }
    }
}

fn default_peq_channel() -> Vec<PeqBand> {
    PEQ_DEFAULT_FREQS
        .iter()
        .map(|&freq| PeqBand {
            freq,
            ..Default::default()
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BusParams {
    pub label: String,
    /// PipeWire node name of the playback device (A buses only).
    pub device: Option<String>,
    pub gain_db: f32,
    pub mute: bool,
    pub mono: bool,
    pub mode: BusMode,
    pub eq_on: bool,
    /// `eq[channel][band]`.
    pub eq: Vec<Vec<PeqBand>>,
}

impl Default for BusParams {
    fn default() -> Self {
        Self {
            label: String::new(),
            device: None,
            gain_db: 0.0,
            mute: false,
            mono: false,
            mode: BusMode::Normal,
            eq_on: false,
            eq: (0..MAX_CHANNELS).map(|_| default_peq_channel()).collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RecFormat {
    #[default]
    Wav16,
    Wav24,
    Wav32f,
    /// Encoded through the external `lame` program.
    Mp3,
}

impl RecFormat {
    pub const ALL: [RecFormat; 4] = [
        RecFormat::Wav16,
        RecFormat::Wav24,
        RecFormat::Wav32f,
        RecFormat::Mp3,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RecFormat::Wav16 => "WAV 16-bit",
            RecFormat::Wav24 => "WAV 24-bit",
            RecFormat::Wav32f => "WAV 32-bit float",
            RecFormat::Mp3 => "MP3",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RecFormat::Wav16 => "wav16",
            RecFormat::Wav24 => "wav24",
            RecFormat::Wav32f => "wav32f",
            RecFormat::Mp3 => "mp3",
        }
    }

    pub fn from_name(s: &str) -> Option<RecFormat> {
        Self::ALL
            .into_iter()
            .find(|f| f.name().eq_ignore_ascii_case(s))
    }
}

/// Tape recorder / player.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TapeParams {
    pub routes: [bool; NUM_BUSES],
    pub gain_db: f32,
    /// The recording is the sum of armed strips (post-fader) and armed buses.
    pub arm_strips: [bool; NUM_STRIPS],
    pub arm_buses: [bool; NUM_BUSES],
    pub format: RecFormat,
    pub mp3_kbps: u32,
    /// Empty = the default recordings folder.
    pub directory: String,
    pub loop_play: bool,
    pub last_file: String,
}

impl Default for TapeParams {
    fn default() -> Self {
        Self {
            routes: [true, false, false, false, false],
            gain_db: 0.0,
            arm_strips: [false; NUM_STRIPS],
            arm_buses: [true, false, false, false, false],
            format: RecFormat::Wav16,
            mp3_kbps: 192,
            directory: String::new(),
            loop_play: false,
            last_file: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VbanFormat {
    #[default]
    Int16,
    Int24,
    Float32,
}

impl VbanFormat {
    pub const ALL: [VbanFormat; 3] = [VbanFormat::Int16, VbanFormat::Int24, VbanFormat::Float32];

    pub fn label(self) -> &'static str {
        match self {
            VbanFormat::Int16 => "PCM 16",
            VbanFormat::Int24 => "PCM 24",
            VbanFormat::Float32 => "Float 32",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            VbanFormat::Int16 => "int16",
            VbanFormat::Int24 => "int24",
            VbanFormat::Float32 => "float32",
        }
    }

    pub fn from_name(s: &str) -> Option<VbanFormat> {
        Self::ALL
            .into_iter()
            .find(|f| f.name().eq_ignore_ascii_case(s))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VbanInStream {
    pub enabled: bool,
    pub name: String,
    /// Accept only from this address; empty accepts any sender.
    pub ip: String,
    /// Strip the stream is mixed into.
    pub strip: usize,
}

impl Default for VbanInStream {
    fn default() -> Self {
        Self {
            enabled: false,
            name: "Stream1".into(),
            ip: String::new(),
            strip: NUM_HW_STRIPS,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VbanOutStream {
    pub enabled: bool,
    pub name: String,
    pub ip: String,
    pub port: u16,
    /// Bus whose output is sent.
    pub bus: usize,
    pub channels: u8,
    pub format: VbanFormat,
}

impl Default for VbanOutStream {
    fn default() -> Self {
        Self {
            enabled: false,
            name: "Stream1".into(),
            ip: String::new(),
            port: crate::vban::DEFAULT_PORT,
            bus: 0,
            channels: 2,
            format: VbanFormat::Int16,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VbanParams {
    pub enabled: bool,
    pub listen_port: u16,
    pub incoming: Vec<VbanInStream>,
    pub outgoing: Vec<VbanOutStream>,
    /// Accept VBAN-TEXT packets as control scripts.
    pub text_enabled: bool,
    /// Accept text commands only from this stream name; empty accepts any.
    pub text_stream: String,
}

impl Default for VbanParams {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_port: crate::vban::DEFAULT_PORT,
            incoming: (0..NUM_VBAN_STREAMS)
                .map(|i| VbanInStream {
                    name: format!("Stream{}", i + 1),
                    ..Default::default()
                })
                .collect(),
            outgoing: (0..NUM_VBAN_STREAMS)
                .map(|i| VbanOutStream {
                    name: format!("Stream{}", i + 1),
                    ..Default::default()
                })
                .collect(),
            text_enabled: false,
            text_stream: "Command1".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MidiKind {
    #[default]
    Cc,
    Note,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MidiMode {
    /// Controller value 0..127 is scaled to min..max.
    #[default]
    Absolute,
    /// Each press flips between min and max.
    Toggle,
    /// Max while held, min when released.
    Momentary,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MidiBinding {
    pub kind: MidiKind,
    /// MIDI channel 0..15.
    pub channel: u8,
    /// Controller or note number.
    pub number: u8,
    /// Script path, e.g. `Strip[0].Gain`.
    pub target: String,
    pub min: f32,
    pub max: f32,
    pub mode: MidiMode,
}

impl Default for MidiBinding {
    fn default() -> Self {
        Self {
            kind: MidiKind::Cc,
            channel: 0,
            number: 0,
            target: String::new(),
            min: 0.0,
            max: 1.0,
            mode: MidiMode::Absolute,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MidiParams {
    /// Substring of the MIDI input port name; empty disables MIDI.
    pub input_device: String,
    pub bindings: Vec<MidiBinding>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MacroButton {
    pub label: String,
    pub on_press: String,
    pub on_release: String,
    /// Toggle buttons run `on_press` when switched on and `on_release` when switched off.
    pub toggle: bool,
    pub state: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineSettings {
    pub sample_rate: u32,
    /// Requested PipeWire quantum.
    pub buffer_frames: u32,
    /// Channels exposed by the virtual input devices (2 or 8).
    pub virtual_input_channels: u8,
    /// Channels exposed by the B1/B2 virtual outputs (2 or 8).
    pub virtual_output_channels: u8,
    /// Channels sent to A1..A3 hardware outputs (2 or 8).
    pub hw_output_channels: u8,
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            buffer_frames: 512,
            virtual_input_channels: 2,
            virtual_output_channels: 2,
            hw_output_channels: 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MixerParams {
    pub strips: Vec<StripParams>,
    pub buses: Vec<BusParams>,
    /// Source for each bus channel in Composite mode (see [`composite_source`]).
    pub composite: [u8; MAX_CHANNELS],
    pub tape: TapeParams,
    pub vban: VbanParams,
    pub midi: MidiParams,
    pub macros: Vec<MacroButton>,
    pub settings: EngineSettings,
    /// Lock faders and buttons in the GUI.
    pub gui_locked: bool,
}

impl Default for MixerParams {
    fn default() -> Self {
        let mut strips: Vec<StripParams> =
            (0..NUM_STRIPS).map(|_| StripParams::default()).collect();
        // A typical setup: the microphone goes to the virtual output B1, the rest to the speakers.
        strips[0].routes = [false, false, false, true, false];
        Self {
            strips,
            buses: (0..NUM_BUSES).map(|_| BusParams::default()).collect(),
            composite: [1, 2, 0, 0, 0, 0, 0, 0],
            tape: TapeParams::default(),
            vban: VbanParams::default(),
            midi: MidiParams::default(),
            macros: (0..8)
                .map(|i| MacroButton {
                    label: format!("Button {}", i + 1),
                    ..Default::default()
                })
                .collect(),
            settings: EngineSettings::default(),
            gui_locked: false,
        }
    }
}

impl MixerParams {
    /// Repairs sizes and ranges after loading a hand-edited or older file.
    pub fn normalize(&mut self) {
        self.strips.resize_with(NUM_STRIPS, StripParams::default);
        self.buses.resize_with(NUM_BUSES, BusParams::default);
        for (i, s) in self.strips.iter_mut().enumerate() {
            s.gain_db = s.gain_db.clamp(GAIN_MIN_DB, GAIN_MAX_DB);
            s.limit_db = s.limit_db.clamp(-40.0, LIMIT_OFF_DB);
            s.karaoke = s.karaoke.min(4);
            for v in [
                &mut s.pan_x,
                &mut s.pan_y,
                &mut s.color_x,
                &mut s.color_y,
                &mut s.fx_x,
                &mut s.fx_y,
            ] {
                *v = v.clamp(-1.0, 1.0);
            }
            if is_virtual_strip(i) {
                s.device = None;
            }
        }
        for (i, b) in self.buses.iter_mut().enumerate() {
            b.gain_db = b.gain_db.clamp(GAIN_MIN_DB, GAIN_MAX_DB);
            b.eq.resize_with(MAX_CHANNELS, default_peq_channel);
            for ch in &mut b.eq {
                ch.resize_with(PEQ_BANDS, PeqBand::default);
            }
            if is_virtual_bus(i) {
                b.device = None;
            }
        }
        for c in &mut self.composite {
            if *c > COMPOSITE_SOURCES {
                *c = 0;
            }
        }
        let d = VbanParams::default();
        self.vban
            .incoming
            .resize_with(NUM_VBAN_STREAMS, Default::default);
        self.vban
            .outgoing
            .resize_with(NUM_VBAN_STREAMS, Default::default);
        for s in &mut self.vban.incoming {
            s.strip = s.strip.min(NUM_STRIPS - 1);
        }
        for s in &mut self.vban.outgoing {
            s.bus = s.bus.min(NUM_BUSES - 1);
            s.channels = s.channels.clamp(1, MAX_CHANNELS as u8);
        }
        if self.vban.listen_port == 0 {
            self.vban.listen_port = d.listen_port;
        }
        let st = &mut self.settings;
        if !matches!(st.sample_rate, 44100 | 48000 | 88200 | 96000) {
            st.sample_rate = 48000;
        }
        st.buffer_frames = st.buffer_frames.clamp(64, 4096);
        for ch in [
            &mut st.virtual_input_channels,
            &mut st.virtual_output_channels,
            &mut st.hw_output_channels,
        ] {
            if *ch != 8 {
                *ch = 2;
            }
        }
    }

    pub fn strip_name(&self, i: usize) -> String {
        match self.strips.get(i) {
            Some(s) if !s.label.is_empty() => s.label.clone(),
            _ => default_strip_name(i),
        }
    }

    pub fn bus_name(&self, i: usize) -> String {
        match self.buses.get(i) {
            Some(b) if !b.label.is_empty() => format!("{} {}", BUS_NAMES[i], b.label),
            _ => BUS_NAMES[i].to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composite_codes_cover_every_input_channel() {
        assert_eq!(composite_source(0), None);
        assert_eq!(composite_source(1), Some((0, 0)));
        assert_eq!(composite_source(6), Some((2, 1)));
        assert_eq!(composite_source(7), Some((3, 0)));
        assert_eq!(composite_source(22), Some((4, 7)));
        assert_eq!(composite_source(23), None);
    }

    #[test]
    fn normalize_repairs_truncated_config() {
        let mut p = MixerParams {
            strips: vec![],
            buses: vec![],
            ..Default::default()
        };
        p.settings.sample_rate = 1234;
        p.normalize();
        assert_eq!(p.strips.len(), NUM_STRIPS);
        assert_eq!(p.buses[4].eq.len(), MAX_CHANNELS);
        assert_eq!(p.settings.sample_rate, 48000);
    }
}
