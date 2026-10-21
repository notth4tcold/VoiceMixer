//! VoiceMixer: virtual audio mixing console for Linux.

#![allow(clippy::needless_range_loop)]

mod backend;
mod gui;
mod midi;
mod remote;
mod tape;
mod vban_net;

use crossbeam_channel::Receiver;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use voicemixer_core::*;

const HELP: &str = "\
VoiceMixer - virtual audio mixing console

USAGE:
    voicemixer [--headless] [--config FILE] [--import-banana FILE]
    voicemixer ctl <script>          e.g. voicemixer ctl 'Strip[0].Gain = -6'
    voicemixer ctl get <parameter>   e.g. voicemixer ctl get Bus[0].Mute

OPTIONS:
    --headless             Run without a window (control with `voicemixer ctl`)
    --config FILE          Settings file (default: ~/.config/voicemixer/settings.toml)
    --import-banana FILE   Import a Voicemeeter Banana settings XML at start-up

ENVIRONMENT:
    VOICEMIXER_NULL_BACKEND=1   Run without PipeWire (no audio; for testing)
    VOICEMIXER_TEST_TONE=1      With the null backend, feed a test tone into inputs 1 and 4
    RUST_LOG=debug              More log output
";

fn main() -> anyhow::Result<()> {
    // "info" for our own crates only: left at the env_logger default, the
    // GUI's D-Bus and Wayland dependencies log multi-kilobyte "info" lines
    // per frame (full struct dumps), drowning out anything useful. RUST_LOG
    // still overrides this entirely, so `RUST_LOG=debug` keeps working as
    // documented below.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,voicemixer=info,voicemixer_core=info"),
    )
    .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("ctl") {
        return remote::client(&args[1..]);
    }
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{HELP}");
        return Ok(());
    }
    let headless = args.iter().any(|a| a == "--headless");
    let config_path = arg_value(&args, "--config")
        .map(PathBuf::from)
        .unwrap_or_else(config::default_path);
    let mut params = config::load_or_default(&config_path);
    if let Some(xml) = arg_value(&args, "--import-banana") {
        params = banana::import_file(Path::new(&xml), params)?;
        log::info!("imported Voicemeeter settings from {xml}");
    }
    run(params, config_path, headless)
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn run(params: MixerParams, config_path: PathBuf, headless: bool) -> anyhow::Result<()> {
    let rate = params.settings.sample_rate;
    let shared = Arc::new(Shared::new(params));
    let (action_tx, action_rx) = crossbeam_channel::unbounded();
    let ctl = Controller::new(shared.clone(), action_tx);

    // First, so a second instance stops here instead of fighting over devices.
    remote::start(ctl.clone())?;

    let frames = rate as usize;
    let (rec_tx, rec_rx) = rtrb::RingBuffer::new(frames * 2 * 2);
    let (tape_tx, tape_rx) = rtrb::RingBuffer::new(frames / 2 * 2);
    let (mut vban_in_tx, mut vban_in_rx, mut vban_out_tx, mut vban_out_rx) =
        (vec![], vec![], vec![], vec![]);
    for _ in 0..NUM_VBAN_STREAMS {
        let (p, c) = rtrb::RingBuffer::new(frames / 4 * MAX_CHANNELS);
        vban_in_tx.push(p);
        vban_in_rx.push(c);
        let (p, c) = rtrb::RingBuffer::new(frames / 4 * MAX_CHANNELS);
        vban_out_tx.push(p);
        vban_out_rx.push(c);
    }
    let taps = Taps {
        rec: Some(rec_tx),
        tape: Some(tape_rx),
        vban_in: vban_in_rx,
        vban_out: vban_out_tx,
    };
    let engine = Engine::new(shared.clone(), rate, taps);

    let backend = backend::start(engine, shared.clone())?;
    let tape = tape::Tape::start(shared.clone(), tape_tx, rec_rx, rate)?;
    vban_net::start(ctl.clone(), vban_in_tx, vban_out_rx, rate)?;
    midi::start(ctl.clone())?;

    {
        let ctl = ctl.clone();
        std::thread::Builder::new()
            .name("actions".into())
            .spawn(move || dispatch(action_rx, ctl, tape, backend))?;
    }
    {
        let shared = shared.clone();
        let path = config_path.clone();
        std::thread::Builder::new()
            .name("autosave".into())
            .spawn(move || autosave(shared, path))?;
    }
    {
        // Meters only ever accumulate (`fetch_max`) as the engine writes
        // them; something has to drain and decay them on a clock, or a
        // peak latches forever instead of falling back to silence. The GUI
        // used to be the only thing doing that, so `voicemixer ctl levels`
        // in `--headless` mode (no GUI ever running) showed the loudest
        // peak since start-up, never moving. Run it here instead, for both
        // modes, so the GUI and the remote socket just read an already
        // up-to-date value.
        let shared = shared.clone();
        std::thread::Builder::new()
            .name("meters".into())
            .spawn(move || loop {
                std::thread::sleep(shared::METER_TICK);
                shared.meters.tick();
            })?;
    }

    if headless {
        log::info!("running headless; control with `voicemixer ctl` (Ctrl+C to quit)");
        loop {
            std::thread::park();
        }
    }
    gui::run(ctl)?;
    config::save(&config_path, &shared.snapshot())?;
    Ok(())
}

/// Carries out actions requested by the GUI, scripts, MIDI and macros.
fn dispatch(
    rx: Receiver<Action>,
    ctl: Controller,
    tape: tape::Tape,
    backend: backend::BackendHandle,
) {
    let say = |msg: String| {
        log::info!("{msg}");
        *ctl.shared.status.message.lock() = msg;
    };
    for action in rx {
        match action {
            Action::TapePlay
            | Action::TapeStop
            | Action::TapeRecord
            | Action::TapePause
            | Action::TapeRewind
            | Action::TapeForward
            | Action::TapeLoad(_)
            | Action::TapeEject => tape.command(action),
            Action::SavePreset(name) => {
                let path = preset_path(&name);
                match config::save(&path, &ctl.shared.snapshot()) {
                    Ok(()) => say(format!("Saved preset {}", path.display())),
                    Err(e) => say(format!("Could not save preset: {e:#}")),
                }
            }
            Action::LoadPreset(name) => {
                let path = preset_path(&name);
                let is_xml = path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("xml"));
                let loaded = if is_xml {
                    banana::import_file(&path, ctl.shared.snapshot())
                } else {
                    config::load(&path)
                };
                match loaded {
                    Ok(p) => {
                        ctl.shared.replace(p);
                        backend.reconnect();
                        say(format!("Loaded {}", path.display()));
                    }
                    Err(e) => say(format!("Could not load {}: {e:#}", path.display())),
                }
            }
            Action::Reset => {
                let old = ctl.shared.snapshot();
                let mut fresh = MixerParams {
                    settings: old.settings.clone(),
                    ..Default::default()
                };
                for (n, o) in fresh.strips.iter_mut().zip(&old.strips) {
                    n.device = o.device.clone();
                }
                for (n, o) in fresh.buses.iter_mut().zip(&old.buses) {
                    n.device = o.device.clone();
                }
                ctl.shared.replace(fresh);
                say("All settings reset (devices kept)".into());
            }
            Action::ReconnectDevices => backend.reconnect(),
            Action::MacroDown(i) => run_macro(&ctl, i, true),
            Action::MacroUp(i) => run_macro(&ctl, i, false),
        }
    }
}

fn preset_path(name: &str) -> PathBuf {
    let p = PathBuf::from(name);
    if p.is_absolute() || p.components().count() > 1 {
        p
    } else {
        let mut p = config::presets_dir().join(name);
        if p.extension().is_none() {
            p.set_extension("toml");
        }
        p
    }
}

fn run_macro(ctl: &Controller, i: usize, down: bool) {
    let script = {
        let mut p = ctl.shared.params.lock();
        let Some(m) = p.macros.get_mut(i) else { return };
        if m.toggle {
            if !down {
                return;
            }
            m.state = !m.state;
            if m.state {
                m.on_press.clone()
            } else {
                m.on_release.clone()
            }
        } else {
            m.state = down;
            if down {
                m.on_press.clone()
            } else {
                m.on_release.clone()
            }
        }
    };
    ctl.shared.bump();
    if script.trim().is_empty() {
        return;
    }
    if let Err(e) = ctl.run_script(&script) {
        *ctl.shared.status.message.lock() = format!("Macro {}: {e}", i + 1);
    }
}

fn autosave(shared: Arc<Shared>, path: PathBuf) {
    let mut saved = shared.generation();
    loop {
        std::thread::sleep(Duration::from_secs(3));
        let g = shared.generation();
        if g != saved {
            if let Err(e) = config::save(&path, &shared.snapshot()) {
                log::warn!("autosave: {e:#}");
            }
            saved = g;
        }
    }
}
