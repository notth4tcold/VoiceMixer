//! Silent backend: runs the engine on a timer without any audio device.
//! `VOICEMIXER_TEST_TONE=1` feeds a 440 Hz tone into the first hardware
//! strip and the first virtual strip, so meters and routing can be checked.

use super::BackendHandle;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use voicemixer_core::{Engine, Shared, NUM_BUSES, NUM_HW_STRIPS};

pub fn start(mut engine: Engine, shared: Arc<Shared>) -> anyhow::Result<BackendHandle> {
    let frames = shared.params.lock().settings.buffer_frames as usize;
    let rate = engine.sample_rate();
    let tone = std::env::var_os("VOICEMIXER_TEST_TONE").is_some();
    *shared.status.backend.lock() = if tone {
        "Null (test tone)"
    } else {
        "Null (no audio device)"
    }
    .into();
    shared.status.running.store(true, Ordering::Relaxed);
    std::thread::Builder::new()
        .name("null-audio".into())
        .spawn(move || {
            let period = Duration::from_secs_f64(frames as f64 / rate as f64);
            let mut out = vec![0.0f32; frames * 2];
            let mut tone_buf = vec![0.0f32; frames * 2];
            let step = 2.0 * std::f32::consts::PI * 440.0 / rate as f32;
            let mut phase = 0.0f32;
            let mut next = Instant::now();
            loop {
                if tone {
                    for f in 0..frames {
                        let v = 0.25 * phase.sin();
                        phase = (phase + step) % (2.0 * std::f32::consts::PI);
                        tone_buf[2 * f] = v;
                        tone_buf[2 * f + 1] = v;
                    }
                    engine.push_input(0, &tone_buf, 2);
                    engine.push_input(NUM_HW_STRIPS, &tone_buf, 2);
                }
                for b in 0..NUM_BUSES {
                    engine.pull_output(b, &mut out, 2);
                }
                next += period;
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    next = now;
                }
            }
        })?;
    Ok(BackendHandle::new(|| {}))
}
