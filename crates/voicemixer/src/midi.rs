//! MIDI control surface input with learn mode.

use voicemixer_core::Controller;

#[cfg(feature = "midi")]
pub fn start(ctl: Controller) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("midi".into())
        .spawn(move || watch(ctl))?;
    Ok(())
}

#[cfg(not(feature = "midi"))]
pub fn start(_ctl: Controller) -> anyhow::Result<()> {
    Ok(())
}

/// Lists ports and (re)connects whenever the selected device changes.
#[cfg(feature = "midi")]
fn watch(ctl: Controller) {
    use midir::MidiInput;
    let mut connected: Option<(String, midir::MidiInputConnection<()>)> = None;
    loop {
        let wanted = ctl.shared.params.lock().midi.input_device.clone();
        if let Ok(input) = MidiInput::new("VoiceMixer") {
            let ports = input.ports();
            let names: Vec<String> = ports
                .iter()
                .filter_map(|p| input.port_name(p).ok())
                .collect();
            let still_there = connected.as_ref().is_some_and(|(n, _)| names.contains(n));
            let current_wanted = connected
                .as_ref()
                .is_some_and(|(n, _)| !wanted.is_empty() && n.contains(&wanted));
            *ctl.shared.midi_ports.lock() = names.clone();
            if !(still_there && current_wanted) {
                connected = None;
                if !wanted.is_empty() {
                    if let Some(port) = ports
                        .iter()
                        .find(|p| input.port_name(p).is_ok_and(|n| n.contains(&wanted)))
                    {
                        let name = input.port_name(port).unwrap_or_default();
                        let cb_ctl = ctl.clone();
                        match input.connect(
                            port,
                            "voicemixer-in",
                            move |_, msg, _| handle(&cb_ctl, msg),
                            (),
                        ) {
                            Ok(conn) => connected = Some((name, conn)),
                            Err(e) => *ctl.shared.status.message.lock() = format!("MIDI: {e}"),
                        }
                    }
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

#[cfg_attr(not(feature = "midi"), allow(dead_code))]
fn handle(ctl: &Controller, msg: &[u8]) {
    use voicemixer_core::params::{MidiBinding, MidiKind, MidiMode};
    use voicemixer_core::script::ParamKind;
    if msg.len() < 3 {
        return;
    }
    let status = msg[0] & 0xF0;
    let channel = msg[0] & 0x0F;
    let (number, value) = (msg[1], msg[2]);
    let (kind, pressed) = match status {
        0xB0 => (MidiKind::Cc, value > 0),
        0x90 => (MidiKind::Note, value > 0),
        0x80 => (MidiKind::Note, false),
        _ => return,
    };

    let learn = ctl.shared.midi_learn.lock().take();
    if let Some(target) = learn {
        let (min, max, mode) = match ctl.describe(&target) {
            Ok(ParamKind::Number { min, max }) => (
                min,
                max,
                if kind == MidiKind::Cc {
                    MidiMode::Absolute
                } else {
                    MidiMode::Toggle
                },
            ),
            _ => (
                0.0,
                1.0,
                if kind == MidiKind::Cc {
                    MidiMode::Absolute
                } else {
                    MidiMode::Toggle
                },
            ),
        };
        ctl.shared.edit(|p| {
            p.midi
                .bindings
                .retain(|b| !(b.kind == kind && b.channel == channel && b.number == number));
            p.midi.bindings.push(MidiBinding {
                kind,
                channel,
                number,
                target,
                min,
                max,
                mode,
            });
        });
        return;
    }

    let bindings: Vec<MidiBinding> = ctl
        .shared
        .params
        .lock()
        .midi
        .bindings
        .iter()
        .filter(|b| b.kind == kind && b.channel == channel && b.number == number)
        .cloned()
        .collect();
    for b in bindings {
        let new = match b.mode {
            MidiMode::Absolute => Some(b.min + (value as f32 / 127.0) * (b.max - b.min)),
            MidiMode::Momentary => Some(if pressed { b.max } else { b.min }),
            MidiMode::Toggle if pressed => {
                let cur = ctl
                    .get(&b.target)
                    .ok()
                    .and_then(|v| v.as_num().ok())
                    .unwrap_or(b.min);
                Some(if (cur - b.max).abs() < 1e-3 {
                    b.min
                } else {
                    b.max
                })
            }
            MidiMode::Toggle => None,
        };
        if let Some(v) = new {
            if let Err(e) = ctl.run_script(&format!("{} = {v}", b.target)) {
                log::warn!("MIDI binding {}: {e}", b.target);
            }
        }
    }
}
