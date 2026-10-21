//! Signal processing building blocks. All processors work on planar buffers
//! (`buf[channel][frame]`) and never allocate after construction.

use crate::params::{BusMode, CompParams, GateParams, PeqKind, MAX_CHANNELS};
use std::f32::consts::PI;

pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

pub fn lin_to_db(x: f32) -> f32 {
    if x <= 1e-9 {
        -180.0
    } else {
        20.0 * x.log10()
    }
}

/// One-pole coefficient reaching ~63% of a step after `ms` milliseconds.
pub fn time_coef(ms: f32, fs: f32) -> f32 {
    let samples = (ms.max(0.01) * 0.001 * fs).max(1.0);
    1.0 - (-1.0 / samples).exp()
}

/// RBJ "Audio EQ Cookbook" biquad, transposed direct form II.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Default for Biquad {
    fn default() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }
}

impl Biquad {
    pub fn new(kind: PeqKind, fs: f32, freq: f32, q: f32, gain_db: f32) -> Self {
        let mut b = Self::default();
        b.set(kind, fs, freq, q, gain_db);
        b
    }

    /// Updates the coefficients and keeps the filter state, so changes are click-free.
    pub fn set(&mut self, kind: PeqKind, fs: f32, freq: f32, q: f32, gain_db: f32) {
        let f = freq.clamp(10.0, fs * 0.49);
        let w0 = 2.0 * PI * f / fs;
        let (sn, cs) = w0.sin_cos();
        let q = q.max(0.05);
        let alpha = sn / (2.0 * q);
        let a = 10f32.powf(gain_db / 40.0);
        let sq = 2.0 * a.sqrt() * alpha;
        let (b0, b1, b2, a0, a1, a2) = match kind {
            PeqKind::Peak => (
                1.0 + alpha * a,
                -2.0 * cs,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cs,
                1.0 - alpha / a,
            ),
            PeqKind::LowShelf => (
                a * ((a + 1.0) - (a - 1.0) * cs + sq),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                a * ((a + 1.0) - (a - 1.0) * cs - sq),
                (a + 1.0) + (a - 1.0) * cs + sq,
                -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                (a + 1.0) + (a - 1.0) * cs - sq,
            ),
            PeqKind::HighShelf => (
                a * ((a + 1.0) + (a - 1.0) * cs + sq),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                a * ((a + 1.0) + (a - 1.0) * cs - sq),
                (a + 1.0) - (a - 1.0) * cs + sq,
                2.0 * ((a - 1.0) - (a + 1.0) * cs),
                (a + 1.0) - (a - 1.0) * cs - sq,
            ),
            PeqKind::LowPass => (
                (1.0 - cs) / 2.0,
                1.0 - cs,
                (1.0 - cs) / 2.0,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            PeqKind::HighPass => (
                (1.0 + cs) / 2.0,
                -(1.0 + cs),
                (1.0 + cs) / 2.0,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            PeqKind::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            PeqKind::Notch => (1.0, -2.0 * cs, 1.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
        };
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    pub fn process_slice(&mut self, buf: &mut [f32]) {
        for x in buf {
            *x = self.process(*x);
        }
        // Flush denormals that build up in the state during silence.
        if self.z1.abs() < 1e-25 {
            self.z1 = 0.0;
        }
        if self.z2.abs() < 1e-25 {
            self.z2 = 0.0;
        }
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// Magnitude response in dB at `freq`, used to draw EQ curves.
    pub fn magnitude_db(&self, freq: f32, fs: f32) -> f32 {
        let w = 2.0 * PI * freq / fs;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        let num = nr * nr + ni * ni;
        let den = (dr * dr + di * di).max(1e-20);
        10.0 * (num / den).max(1e-20).log10()
    }
}

/// Exponential parameter smoother to avoid zipper noise.
#[derive(Clone, Copy, Debug)]
pub struct Smooth {
    pub current: f32,
    pub target: f32,
    coef: f32,
}

impl Smooth {
    pub fn new(value: f32, ms: f32, fs: f32) -> Self {
        Self {
            current: value,
            target: value,
            coef: time_coef(ms, fs),
        }
    }

    #[inline]
    pub fn tick(&mut self) -> f32 {
        self.current += (self.target - self.current) * self.coef;
        self.current
    }

    pub fn is_settled(&self) -> bool {
        (self.current - self.target).abs() < 1e-5
    }
}

/// Stereo-linked feed-forward compressor with soft knee.
#[derive(Clone, Debug)]
pub struct Compressor {
    active: bool,
    threshold: f32,
    ratio: f32,
    knee_db: f32,
    attack: f32,
    release: f32,
    in_gain: f32,
    out_gain: f32,
    /// Peak envelope of the input (linear), with attack/release ballistics.
    env: f32,
    gr_db: f32,
}

impl Default for Compressor {
    fn default() -> Self {
        Self {
            active: false,
            threshold: 0.0,
            ratio: 1.0,
            knee_db: 0.0,
            attack: 1.0,
            release: 1.0,
            in_gain: 1.0,
            out_gain: 1.0,
            env: 0.0,
            gr_db: 0.0,
        }
    }
}

impl Compressor {
    pub fn configure(&mut self, p: &CompParams, fs: f32) {
        self.active = p.amount > 0.0;
        self.threshold = p.threshold_db;
        self.ratio = p.ratio();
        self.knee_db = p.knee.clamp(0.0, 1.0) * 20.0;
        self.attack = time_coef(p.attack_ms, fs);
        self.release = time_coef(p.release_ms, fs);
        self.in_gain = db_to_lin(p.input_gain_db);
        let makeup = if p.auto_makeup {
            -self.static_curve(0.0) * 0.5
        } else {
            0.0
        };
        self.out_gain = db_to_lin(p.output_gain_db + makeup);
    }

    /// Gain reduction in dB (<= 0) for an input level in dB.
    fn static_curve(&self, level_db: f32) -> f32 {
        let over = level_db - self.threshold;
        let slope = 1.0 / self.ratio - 1.0;
        let w = self.knee_db;
        if w > 0.0 && 2.0 * over.abs() <= w {
            slope * (over + w / 2.0).powi(2) / (2.0 * w)
        } else if over > 0.0 {
            slope * over
        } else {
            0.0
        }
    }

    pub fn process(&mut self, buf: &mut [Vec<f32>], channels: usize, n: usize) {
        if !self.active {
            return;
        }
        for f in 0..n {
            let mut peak = 0f32;
            for ch in buf.iter_mut().take(channels) {
                ch[f] *= self.in_gain;
                peak = peak.max(ch[f].abs());
            }
            let coef = if peak > self.env {
                self.attack
            } else {
                self.release
            };
            self.env += (peak - self.env) * coef;
            self.gr_db = self.static_curve(lin_to_db(self.env));
            let g = db_to_lin(self.gr_db) * self.out_gain;
            for ch in buf.iter_mut().take(channels) {
                ch[f] *= g;
            }
        }
    }

    pub fn gain_reduction_db(&self) -> f32 {
        self.gr_db
    }
}

/// Noise gate with hold time and a high-passed detector.
#[derive(Clone, Debug)]
pub struct Gate {
    active: bool,
    threshold: f32,
    floor: f32,
    attack: f32,
    release: f32,
    hold: usize,
    hold_left: usize,
    env: f32,
    env_release: f32,
    gain: f32,
    sidechain: [Biquad; 2],
}

impl Default for Gate {
    fn default() -> Self {
        Self {
            active: false,
            threshold: 0.0,
            floor: 0.0,
            attack: 1.0,
            release: 1.0,
            hold: 0,
            hold_left: 0,
            env: 0.0,
            env_release: 1.0,
            gain: 1.0,
            sidechain: [Biquad::default(); 2],
        }
    }
}

impl Gate {
    pub fn configure(&mut self, p: &GateParams, fs: f32) {
        self.active = p.amount > 0.0;
        self.threshold = db_to_lin(p.threshold_db());
        self.floor = db_to_lin(p.damping_db.min(0.0));
        self.attack = time_coef(p.attack_ms, fs);
        self.release = time_coef(p.release_ms, fs);
        self.hold = (p.hold_ms.max(0.0) * 0.001 * fs) as usize;
        self.env_release = time_coef(5.0, fs);
        for b in &mut self.sidechain {
            b.set(
                PeqKind::HighPass,
                fs,
                p.sidechain_hz.clamp(20.0, 4000.0),
                0.707,
                0.0,
            );
        }
    }

    pub fn process(&mut self, buf: &mut [Vec<f32>], channels: usize, n: usize) {
        if !self.active {
            self.gain = 1.0;
            return;
        }
        for f in 0..n {
            let mut peak = 0f32;
            for (c, ch) in buf.iter().take(channels.min(2)).enumerate() {
                peak = peak.max(self.sidechain[c].process(ch[f]).abs());
            }
            self.env = if peak > self.env {
                peak
            } else {
                self.env + (peak - self.env) * self.env_release
            };
            let open = if self.env >= self.threshold {
                self.hold_left = self.hold;
                true
            } else if self.hold_left > 0 {
                self.hold_left -= 1;
                true
            } else {
                false
            };
            let (target, coef) = if open {
                (1.0, self.attack)
            } else {
                (self.floor, self.release)
            };
            self.gain += (target - self.gain) * coef;
            for ch in buf.iter_mut().take(channels) {
                ch[f] *= self.gain;
            }
        }
    }

    pub fn is_open(&self) -> bool {
        self.gain > 0.5
    }
}

/// Peak limiter: instant attack, smooth release, hard ceiling.
#[derive(Clone, Debug)]
pub struct Limiter {
    active: bool,
    ceiling: f32,
    release: f32,
    gain: f32,
}

impl Default for Limiter {
    fn default() -> Self {
        Self {
            active: false,
            ceiling: 1.0,
            release: 1.0,
            gain: 1.0,
        }
    }
}

impl Limiter {
    pub fn configure(&mut self, limit_db: f32, fs: f32) {
        self.active = limit_db < crate::params::LIMIT_OFF_DB;
        self.ceiling = db_to_lin(limit_db);
        self.release = time_coef(80.0, fs);
    }

    pub fn process(&mut self, buf: &mut [Vec<f32>], channels: usize, n: usize) {
        if !self.active {
            return;
        }
        for f in 0..n {
            let peak = buf
                .iter()
                .take(channels)
                .fold(0f32, |m, ch| m.max(ch[f].abs()));
            self.gain += (1.0 - self.gain) * self.release;
            if peak * self.gain > self.ceiling {
                self.gain = self.ceiling / peak;
            }
            for ch in buf.iter_mut().take(channels) {
                ch[f] *= self.gain;
            }
        }
    }
}

/// Two-voice stereo chorus driven by the modulation panel.
#[derive(Clone, Debug)]
pub struct Chorus {
    active: bool,
    lines: [Vec<f32>; 2],
    write: usize,
    phase: f32,
    phase_inc: f32,
    mix: f32,
    base: f32,
    depth: f32,
}

impl Chorus {
    pub fn new(fs: f32) -> Self {
        let len = (fs * 0.05) as usize + 4;
        Self {
            active: false,
            lines: [vec![0.0; len], vec![0.0; len]],
            write: 0,
            phase: 0.0,
            phase_inc: 0.0,
            mix: 0.0,
            base: 0.012 * fs,
            depth: 0.004 * fs,
        }
    }

    pub fn configure(&mut self, x: f32, y: f32, fs: f32) {
        self.active = y > 0.001;
        let rate_hz = 0.1 + (x.clamp(-1.0, 1.0) + 1.0) * 1.5;
        self.phase_inc = 2.0 * PI * rate_hz / fs;
        self.mix = y.clamp(0.0, 1.0);
        self.base = 0.012 * fs;
        self.depth = (0.002 + 0.006 * self.mix) * fs;
        if !self.active {
            for l in &mut self.lines {
                l.fill(0.0);
            }
        }
    }

    pub fn process(&mut self, buf: &mut [Vec<f32>], n: usize) {
        if !self.active {
            return;
        }
        let len = self.lines[0].len();
        for f in 0..n {
            for c in 0..2 {
                let x = buf[c][f];
                self.lines[c][self.write] = x;
                let lfo = (self.phase + c as f32 * PI * 0.5).sin();
                let delay = self.base + self.depth * lfo;
                let pos = self.write as f32 - delay + len as f32;
                let i = pos as usize;
                let t = pos - i as f32;
                let a = self.lines[c][i % len];
                let b = self.lines[c][(i + 1) % len];
                let wet = a + (b - a) * t;
                buf[c][f] = x * (1.0 - 0.5 * self.mix) + wet * 0.5 * self.mix;
            }
            self.write = (self.write + 1) % len;
            self.phase += self.phase_inc;
            if self.phase > 2.0 * PI {
                self.phase -= 2.0 * PI;
            }
        }
    }
}

/// Centre-voice removal for stereo material.
#[derive(Clone, Debug, Default)]
pub struct Karaoke {
    mode: u8,
    low: Biquad,
    high: Biquad,
    band: Biquad,
}

impl Karaoke {
    pub fn configure(&mut self, mode: u8, fs: f32) {
        if mode != self.mode {
            self.low.reset();
            self.high.reset();
            self.band.reset();
        }
        self.mode = mode.min(4);
        self.low.set(PeqKind::LowPass, fs, 180.0, 0.707, 0.0);
        self.high.set(PeqKind::HighPass, fs, 8000.0, 0.707, 0.0);
        self.band.set(PeqKind::BandPass, fs, 1000.0, 0.6, 0.0);
    }

    pub fn process(&mut self, buf: &mut [Vec<f32>], n: usize) {
        if self.mode == 0 {
            return;
        }
        let (l, r) = buf.split_at_mut(1);
        let (l, r) = (&mut l[0], &mut r[0]);
        for f in 0..n {
            let (a, b) = (l[f], r[f]);
            let mid = 0.5 * (a + b);
            let side = 0.5 * (a - b);
            let (ol, or) = match self.mode {
                1 => (side, -side),
                2 => {
                    let lo = self.low.process(mid);
                    (side + lo, -side + lo)
                }
                3 => {
                    let keep = self.low.process(mid) + self.high.process(mid);
                    (side + keep, -side + keep)
                }
                _ => {
                    let voice = self.band.process(mid);
                    (a - voice, b - voice)
                }
            };
            l[f] = ol;
            r[f] = or;
        }
    }
}

/// Rewrites a bus buffer according to its multichannel mode. `Composite` is
/// handled by the mixer, because it reads inputs rather than the bus mix.
pub fn apply_bus_mode(mode: BusMode, buf: &mut [Vec<f32>], n: usize) {
    debug_assert!(buf.len() >= MAX_CHANNELS);
    if matches!(mode, BusMode::Normal | BusMode::Composite) {
        return;
    }
    let mut frame = [0f32; MAX_CHANNELS];
    for f in 0..n {
        for (c, v) in frame.iter_mut().enumerate() {
            *v = buf[c][f];
        }
        let [fl, fr, fc, lfe, rl, rr, sl, sr] = frame;
        let out: [f32; MAX_CHANNELS] = match mode {
            BusMode::Normal | BusMode::Composite => frame,
            BusMode::Amix => [
                fl + 0.7 * fc + lfe + rl - sl,
                fr + 0.7 * fc + lfe - rr + sr,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
            ],
            BusMode::Bmix => [
                fl + 0.7 * fc + lfe + rl + sl,
                fr + 0.7 * fc + lfe + rr + sr,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
            ],
            BusMode::Repeat => [fl, fr, fl, fr, fl, fr, fl, fr],
            BusMode::TvMix => {
                let (s, d) = (fl + fr, fl - fr);
                [
                    fl,
                    fr,
                    0.2 * s,
                    0.5 * s,
                    0.7 * d,
                    -0.7 * d,
                    0.7 * d,
                    -0.7 * d,
                ]
            }
            BusMode::UpMix21 => [fl, fr, 0.0, 0.5 * (fl + fr), 0.0, 0.0, 0.0, 0.0],
            BusMode::UpMix41 => [fl, fr, 0.0, 0.5 * (fl + fr), fl, fr, 0.0, 0.0],
            BusMode::UpMix61 => [fl, fr, 0.0, 0.5 * (fl + fr), fl, fr, fl, fr],
            BusMode::CenterOnly => [fc, fc, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            BusMode::LfeOnly => [lfe, lfe, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            BusMode::RearOnly => [rl, rr, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        };
        for (c, v) in out.iter().enumerate() {
            buf[c][f] = *v;
        }
    }
}

/// Averages each left/right pair (front, rear, side) into mono; centre and LFE are untouched.
pub fn mono_pairs(buf: &mut [Vec<f32>], channels: usize, n: usize) {
    for (l, r) in [(0, 1), (4, 5), (6, 7)] {
        if r >= channels {
            break;
        }
        let (a, b) = buf.split_at_mut(r);
        for f in 0..n {
            let m = 0.5 * (a[l][f] + b[0][f]);
            a[l][f] = m;
            b[0][f] = m;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, fs: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI * freq * i as f32 / fs).sin())
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn peak_filter_boosts_at_centre_frequency() {
        let b = Biquad::new(PeqKind::Peak, 48000.0, 1000.0, 1.0, 6.0);
        assert!((b.magnitude_db(1000.0, 48000.0) - 6.0).abs() < 0.05);
        assert!(b.magnitude_db(50.0, 48000.0).abs() < 0.3);
    }

    #[test]
    fn highpass_removes_low_frequencies() {
        let mut b = Biquad::new(PeqKind::HighPass, 48000.0, 1000.0, 0.707, 0.0);
        let mut x = sine(50.0, 48000.0, 48000);
        b.process_slice(&mut x);
        assert!(rms(&x[24000..]) < 0.01);
    }

    #[test]
    fn compressor_reduces_loud_signal() {
        let fs = 48000.0;
        let mut c = Compressor::default();
        c.configure(
            &CompParams {
                amount: 10.0,
                threshold_db: -20.0,
                auto_makeup: false,
                ..Default::default()
            },
            fs,
        );
        let mut buf = vec![sine(440.0, fs, 48000), sine(440.0, fs, 48000)];
        c.process(&mut buf, 2, 48000);
        assert!(rms(&buf[0][24000..]) < 0.25);
    }

    #[test]
    fn compressor_with_makeup_never_exceeds_input_peak_on_steady_tone() {
        // Regression: an instantaneous detector released between zero
        // crossings and the make-up gain pushed a -12 dBFS tone above 0 dBFS.
        let fs = 48000.0;
        let mut c = Compressor::default();
        c.configure(
            &CompParams {
                amount: 4.0,
                threshold_db: -40.0,
                auto_makeup: true,
                ..Default::default()
            },
            fs,
        );
        let tone: Vec<f32> = sine(440.0, fs, 48000).iter().map(|v| v * 0.25).collect();
        let mut buf = vec![tone.clone(), tone];
        c.process(&mut buf, 2, 48000);
        let peak = buf[0][24000..].iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak < 0.25, "peak {peak}");
        assert!(peak > 0.05, "peak {peak}");
    }

    #[test]
    fn gate_closes_on_quiet_signal() {
        let fs = 48000.0;
        let mut g = Gate::default();
        g.configure(
            &GateParams {
                amount: 6.0,
                hold_ms: 10.0,
                release_ms: 20.0,
                ..Default::default()
            },
            fs,
        );
        let quiet: Vec<f32> = sine(440.0, fs, 48000).iter().map(|v| v * 0.001).collect();
        let mut buf = vec![quiet.clone(), quiet];
        g.process(&mut buf, 2, 48000);
        assert!(!g.is_open());
        assert!(rms(&buf[0][40000..]) < 1e-5);
    }

    #[test]
    fn limiter_holds_ceiling() {
        let fs = 48000.0;
        let mut l = Limiter::default();
        l.configure(-6.0, fs);
        let loud: Vec<f32> = sine(100.0, fs, 4800).iter().map(|v| v * 2.0).collect();
        let mut buf = vec![loud.clone(), loud];
        l.process(&mut buf, 2, 4800);
        let ceiling = db_to_lin(-6.0) + 1e-4;
        assert!(buf[0].iter().all(|v| v.abs() <= ceiling));
    }

    #[test]
    fn karaoke_cancels_centre() {
        let fs = 48000.0;
        let mut k = Karaoke::default();
        k.configure(1, fs);
        let s = sine(440.0, fs, 4800);
        let mut buf = vec![s.clone(), s];
        k.process(&mut buf, 4800);
        assert!(rms(&buf[0]) < 1e-6);
    }

    #[test]
    fn mixdown_folds_centre_into_stereo() {
        let mut buf: Vec<Vec<f32>> = (0..8).map(|_| vec![0.0; 4]).collect();
        buf[2].fill(1.0);
        apply_bus_mode(BusMode::Amix, &mut buf, 4);
        assert!((buf[0][0] - 0.7).abs() < 1e-6 && (buf[1][0] - 0.7).abs() < 1e-6);
        assert_eq!(buf[2][0], 0.0);
    }
}
