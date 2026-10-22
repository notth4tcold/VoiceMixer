//! Console sections: input strips, hardware-out selectors, tape deck, master.

use super::widgets::{self, color, PadKind};
use super::{MeterState, PathKind, UiState};
use eframe::egui::{self, vec2, Color32, RichText};
use std::sync::atomic::Ordering;
use voicemixer_core::shared::{Devices, Shared};
use voicemixer_core::*;

const STRIP_W: f32 = 128.0;

fn panel_frame(accent: Color32) -> egui::Frame {
    egui::Frame::NONE
        .fill(color::PANEL)
        .corner_radius(6.0)
        .inner_margin(6.0)
        .stroke(egui::Stroke::new(1.0_f32, accent.gamma_multiply(0.35)))
}

fn section_title(ui: &mut egui::Ui, text: &str, accent: Color32) {
    ui.label(RichText::new(text).size(11.0).strong().color(accent));
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

fn device_combo(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    dev: &mut Option<String>,
    list: &[DeviceInfo],
    width: f32,
) -> egui::Response {
    let text = match dev.as_deref() {
        None => "— select device —".to_string(),
        Some(n) if list.iter().any(|d| d.name == n) => Devices::describe(list, n),
        Some(n) => format!("(missing) {n}"),
    };
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .selected_text(RichText::new(truncate(&text, (width / 6.5) as usize)).size(11.0))
        .show_ui(ui, |ui| {
            ui.selectable_value(dev, None, "— none —");
            for d in list {
                ui.selectable_value(dev, Some(d.name.clone()), &d.description);
            }
        })
        .response
}

#[allow(clippy::too_many_arguments)]
pub fn strip(
    ui: &mut egui::Ui,
    i: usize,
    s: &mut StripParams,
    name: &str,
    devices: &Devices,
    meters: &MeterState,
    locked: bool,
    st: &mut UiState,
) {
    let virt = is_virtual_strip(i);
    let accent = if virt { color::VIRT } else { color::HW };
    panel_frame(accent).show(ui, |ui| {
        ui.set_width(STRIP_W);
        ui.vertical(|ui| {
            let title = ui.add_sized(
                [STRIP_W, 22.0],
                egui::Button::new(RichText::new(truncate(name, 18)).strong().color(accent)).fill(color::PANEL_HI),
            );
            st.help(
                &title,
                if virt {
                    "Virtual input: set this device as the output of your apps. Right-click for options."
                } else {
                    "Hardware input: pick a microphone or line input below. Right-click for options."
                },
            );
            title.context_menu(|ui| strip_menu(ui, i, s, st));

            if virt {
                let dev = if i == NUM_HW_STRIPS { "VoiceMixer Input" } else { "VoiceMixer AUX" };
                ui.label(RichText::new(format!("device: {dev}")).size(10.5).color(color::DIM));
            } else {
                let r = device_combo(ui, ("strip-dev", i), &mut s.device, &devices.inputs, STRIP_W - 8.0);
                st.help(&r, "Capture device feeding this strip.");
            }

            ui.add_enabled_ui(!locked, |ui| {
                strip_panel(ui, i, s, accent, st);
                strip_knobs(ui, i, s, meters, accent, st);
                ui.add_space(2.0);
                ui.horizontal_top(|ui| {
                    egui::Grid::new(("routes", i)).spacing(vec2(3.0, 3.0)).show(ui, |ui| {
                        for (row, pair) in [(0, Some(3)), (1, Some(4)), (2, None)] {
                            route_button(ui, s, row, st);
                            if let Some(b) = pair {
                                route_button(ui, s, b, st);
                            }
                            ui.end_row();
                        }
                    });
                    let ch = strip_channels(i);
                    let fr = widgets::vfader(ui, &mut s.gain_db, &meters.strips[i][..ch], Some(&mut s.limit_db), 150.0, accent);
                    st.help(&fr.fader, "Strip gain. Drag (Shift = fine), scroll, double-click for 0 dB. Right-click for more.");
                    st.help(&fr.meter, "Meter. Click or drag to set the limiter ceiling; double-click to remove it.");
                    fr.fader.context_menu(|ui| {
                        if ui.button("Reset to 0 dB").clicked() {
                            s.gain_db = 0.0;
                            ui.close();
                        }
                        if ui.button("Remove limiter").clicked() {
                            s.limit_db = LIMIT_OFF_DB;
                            ui.close();
                        }
                        if ui.button("MIDI learn: gain").clicked() {
                            st.midi_learn(format!("Strip[{i}].Gain"));
                            ui.close();
                        }
                    });
                });
                let limit = if s.limit_db < LIMIT_OFF_DB { format!("  lim {:.0}", s.limit_db) } else { String::new() };
                ui.label(RichText::new(format!("{}{limit}", widgets::fmt_db(s.gain_db))).size(10.5).color(color::TEXT));
                ui.horizontal(|ui| {
                    let size = vec2(38.0, 22.0);
                    let r = widgets::toggle(ui, &mut s.mono, "MONO", color::MONO, size);
                    st.help(&r, "Mono: sums left and right.");
                    let r = widgets::toggle(ui, &mut s.solo, "SOLO", color::SOLO, size);
                    st.help(&r, "Solo: hear only soloed strips.");
                    let r = widgets::toggle(ui, &mut s.mute, "MUTE", color::MUTE, size);
                    st.help(&r, "Mute this strip.");
                    r.context_menu(|ui| {
                        if ui.button("MIDI learn: mute").clicked() {
                            st.midi_learn(format!("Strip[{i}].Mute"));
                            ui.close();
                        }
                    });
                });
                if virt {
                    ui.horizontal(|ui| {
                        let text = if s.karaoke == 0 { "KARAOKE".to_string() } else { format!("KARAOKE {}", s.karaoke) };
                        let fill = if s.karaoke > 0 { color::VIRT } else { color::OFF };
                        let fg = if s.karaoke > 0 { Color32::BLACK } else { color::TEXT };
                        let r = ui.add_sized([78.0, 20.0], egui::Button::new(RichText::new(text).size(10.5).color(fg)).fill(fill));
                        if r.clicked() {
                            s.karaoke = (s.karaoke + 1) % 5;
                        }
                        st.help(&r, "Voice removal. Click to cycle through four methods; pick the one that works on your material.");
                        let r = widgets::toggle(ui, &mut s.mute_center, "MC", color::MUTE, vec2(38.0, 20.0));
                        st.help(&r, "Mute the centre channel of 5.1/7.1 audio (usually the dialogue).");
                    });
                }
            });
        });
    });
}

fn route_button(ui: &mut egui::Ui, s: &mut StripParams, b: usize, st: &mut UiState) {
    let r = widgets::toggle(
        ui,
        &mut s.routes[b],
        BUS_NAMES[b],
        color::bus(b),
        vec2(30.0, 22.0),
    );
    st.help(
        &r,
        if is_virtual_bus(b) {
            "Send this strip to a virtual output (B bus)."
        } else {
            "Send this strip to a hardware output (A bus)."
        },
    );
}

fn strip_panel(
    ui: &mut egui::Ui,
    i: usize,
    s: &mut StripParams,
    accent: Color32,
    st: &mut UiState,
) {
    let size = vec2(STRIP_W, 72.0);
    if is_virtual_strip(i) {
        let r = widgets::pad(
            ui,
            &mut s.pan_x,
            &mut s.pan_y,
            size,
            PadKind::Surround,
            accent,
        );
        st.help(
            &r,
            "Position: left/right balance and front/rear placement. Double-click to centre.",
        );
        return;
    }
    ui.horizontal(|ui| {
        for (mode, text) in [
            (PanelMode::Color, "Color"),
            (PanelMode::Position, "3D"),
            (PanelMode::Modulation, "FX"),
        ] {
            ui.selectable_value(&mut s.panel, mode, RichText::new(text).size(10.5));
        }
    });
    let (x, y, kind, help) = match s.panel {
        PanelMode::Color => (
            &mut s.color_x,
            &mut s.color_y,
            PadKind::Color,
            "Colour: left darker, right brighter; up adds presence, down adds warmth.",
        ),
        PanelMode::Position => (
            &mut s.pan_x,
            &mut s.pan_y,
            PadKind::Position,
            "3D: left/right pan; up widens the stereo image, down narrows it.",
        ),
        PanelMode::Modulation => (
            &mut s.fx_x,
            &mut s.fx_y,
            PadKind::Modulation,
            "Modulation: move up to add chorus; left/right sets its speed.",
        ),
    };
    let r = widgets::pad(ui, x, y, size, kind, accent);
    st.help(&r, help);
}

fn strip_knobs(
    ui: &mut egui::Ui,
    i: usize,
    s: &mut StripParams,
    meters: &MeterState,
    accent: Color32,
    st: &mut UiState,
) {
    ui.horizontal(|ui| {
        if is_virtual_strip(i) {
            ui.add_space(2.0);
            let fmt = |v: f32| format!("{v:+.1}");
            let r = widgets::knob(ui, &mut s.eq_gain[2], (-12.0, 12.0), 0.0, "Treble", accent, fmt);
            st.help(&r, "Treble (6 kHz shelf).");
            let r = widgets::knob(ui, &mut s.eq_gain[1], (-12.0, 12.0), 0.0, "Medium", accent, fmt);
            st.help(&r, "Medium (1 kHz).");
            let r = widgets::knob(ui, &mut s.eq_gain[0], (-12.0, 12.0), 0.0, "Bass", accent, fmt);
            st.help(&r, "Bass (120 Hz shelf).");
        } else {
            ui.add_space(14.0);
            let r = widgets::knob(ui, &mut s.comp.amount, (0.0, 10.0), 0.0, "Comp", accent, |v| format!("{v:.1}"));
            st.help(&r, "Compressor with automatic make-up gain. Right-click for detailed settings.");
            if r.secondary_clicked() {
                st.dyn_strip = Some(i);
            }
            let r = widgets::knob(ui, &mut s.gate.amount, (0.0, 10.0), 0.0, "Gate", accent, |v| format!("{v:.1}"));
            st.help(&r, "Noise gate: raise until background noise disappears. Right-click for detailed settings.");
            if r.secondary_clicked() {
                st.dyn_strip = Some(i);
            }
            ui.vertical(|ui| {
                ui.add_space(6.0);
                let gr = meters.comp_gr[i];
                let gr_text = if s.comp.amount > 0.0 { format!("-{gr:.0}") } else { "  ".into() };
                ui.label(RichText::new(gr_text).size(9.5).color(color::SOLO));
                let open = s.gate.amount <= 0.0 || meters.gate_open[i];
                let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, if open { color::METER_LO } else { color::OFF });
            });
        }
    });
}

fn strip_menu(ui: &mut egui::Ui, i: usize, s: &mut StripParams, st: &mut UiState) {
    ui.label("Name:");
    ui.text_edit_singleline(&mut s.label);
    ui.separator();
    if ui.button("Copy strip settings").clicked() {
        st.clipboard = Some(s.clone());
        ui.close();
    }
    let can_paste = st.clipboard.is_some();
    if ui
        .add_enabled(can_paste, egui::Button::new("Paste strip settings"))
        .clicked()
    {
        if let Some(c) = &st.clipboard {
            let (label, device) = (s.label.clone(), s.device.clone());
            *s = c.clone();
            s.label = label;
            s.device = device;
        }
        ui.close();
    }
    if ui.button("Reset strip").clicked() {
        let (label, device) = (s.label.clone(), s.device.clone());
        *s = StripParams {
            label,
            device,
            ..Default::default()
        };
        ui.close();
    }
    if !is_virtual_strip(i) && ui.button("Compressor / gate settings…").clicked() {
        st.dyn_strip = Some(i);
        ui.close();
    }
}

pub fn hardware_out(ui: &mut egui::Ui, p: &mut MixerParams, devices: &Devices, st: &mut UiState) {
    panel_frame(color::BUS_A).show(ui, |ui| {
        ui.set_width(430.0);
        section_title(ui, "HARDWARE OUT", color::BUS_A);
        egui::Grid::new("hw-out").spacing(vec2(6.0, 4.0)).show(ui, |ui| {
            for b in 0..NUM_HW_BUSES {
                ui.label(RichText::new(BUS_NAMES[b]).strong().color(color::BUS_A));
                let r = device_combo(ui, ("bus-dev", b), &mut p.buses[b].device, &devices.outputs, 360.0);
                st.help(&r, "Playback device for this A bus.");
                ui.end_row();
            }
        });
        ui.label(RichText::new("B1 and B2 appear to other apps as the recording devices \"VoiceMixer B1\" / \"VoiceMixer B2\".").size(10.0).color(color::DIM));
    });
}

fn fmt_time(frames: u64, rate: u32) -> String {
    let secs = frames / rate.max(1) as u64;
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

pub fn tape(
    ui: &mut egui::Ui,
    p: &mut MixerParams,
    shared: &Shared,
    meters: &MeterState,
    st: &mut UiState,
) {
    let t = &shared.tape;
    let rate = shared.status.sample_rate.load(Ordering::Relaxed);
    let recording = t.recording.load(Ordering::Relaxed);
    let playing = t.playing.load(Ordering::Relaxed);
    panel_frame(color::REC).show(ui, |ui| {
        ui.set_width(430.0);
        ui.horizontal(|ui| {
            section_title(ui, "TAPE RECORDER", color::REC);
            let file = t.file_name.lock().clone();
            ui.label(
                RichText::new(if file.is_empty() {
                    "no file".into()
                } else {
                    truncate(&file, 34)
                })
                .size(11.0)
                .color(color::TEXT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let time = if recording {
                    format!(
                        "REC {}",
                        fmt_time(t.recorded_frames.load(Ordering::Relaxed), rate)
                    )
                } else {
                    let total = t.total_frames.load(Ordering::Relaxed);
                    format!(
                        "{} / {}",
                        fmt_time(t.played_frames.load(Ordering::Relaxed), rate),
                        fmt_time(total, rate)
                    )
                };
                ui.label(RichText::new(time).monospace().color(if recording {
                    color::REC
                } else {
                    color::TEXT
                }));
            });
        });
        ui.horizontal(|ui| {
            let b = |ui: &mut egui::Ui, text: &str, on: bool, c: Color32| {
                ui.add_sized(
                    [46.0, 24.0],
                    egui::Button::new(RichText::new(text).strong().color(if on {
                        Color32::BLACK
                    } else {
                        color::TEXT
                    }))
                    .fill(if on { c } else { color::OFF }),
                )
            };
            if b(ui, "REW", false, color::TEXT).clicked() {
                st.pending.push(Action::TapeRewind);
            }
            if b(ui, "FF", false, color::TEXT).clicked() {
                st.pending.push(Action::TapeForward);
            }
            if b(ui, "PLAY", playing, color::BUS_A).clicked() {
                st.pending.push(if playing {
                    Action::TapePause
                } else {
                    Action::TapePlay
                });
            }
            let r = b(ui, "STOP", false, color::TEXT);
            st.help(&r, "Stop. Click again to go back to the start.");
            if r.clicked() {
                st.pending.push(Action::TapeStop);
            }
            let r = b(ui, "REC", recording, color::REC);
            st.help(
                &r,
                "Record a new file. Click again to pause/resume; STOP finishes the file.",
            );
            if r.clicked() {
                st.pending.push(Action::TapeRecord);
            }
            if ui
                .add_sized([70.0, 24.0], egui::Button::new("Load file…"))
                .clicked()
            {
                st.open_path(PathKind::TapeLoad);
            }
        });
        ui.horizontal(|ui| {
            for b in 0..NUM_BUSES {
                let r = widgets::toggle(
                    ui,
                    &mut p.tape.routes[b],
                    BUS_NAMES[b],
                    color::bus(b),
                    vec2(30.0, 20.0),
                );
                st.help(&r, "Send playback to this bus.");
            }
            ui.add_space(6.0);
            let r = widgets::hfader(ui, &mut p.tape.gain_db, &meters.tape, 150.0, color::REC);
            st.help(&r, "Playback gain.");
            ui.label(RichText::new(widgets::fmt_db(p.tape.gain_db)).size(10.5));
        });
        ui.collapsing(RichText::new("Recorder options").size(11.0), |ui| {
            ui.horizontal(|ui| {
                ui.label("Record:");
                for s in 0..NUM_STRIPS {
                    ui.checkbox(&mut p.tape.arm_strips[s], format!("In{}", s + 1));
                }
            });
            ui.horizontal(|ui| {
                ui.label("        ");
                for b in 0..NUM_BUSES {
                    ui.checkbox(&mut p.tape.arm_buses[b], BUS_NAMES[b]);
                }
            });
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("rec-format")
                    .selected_text(p.tape.format.label())
                    .show_ui(ui, |ui| {
                        for f in RecFormat::ALL {
                            ui.selectable_value(&mut p.tape.format, f, f.label());
                        }
                    });
                if p.tape.format == RecFormat::Mp3 {
                    ui.add(
                        egui::DragValue::new(&mut p.tape.mp3_kbps)
                            .range(32..=320)
                            .suffix(" kbps"),
                    );
                }
                ui.checkbox(&mut p.tape.loop_play, "Loop playback");
            });
            ui.horizontal(|ui| {
                ui.label("Folder:");
                let hint = config::recordings_dir().display().to_string();
                ui.add(
                    egui::TextEdit::singleline(&mut p.tape.directory)
                        .hint_text(hint)
                        .desired_width(330.0),
                );
            });
        });
    });
}

pub fn master(ui: &mut egui::Ui, p: &mut MixerParams, meters: &MeterState, st: &mut UiState) {
    let locked = p.gui_locked;
    panel_frame(color::BUS_B).show(ui, |ui| {
        ui.set_width(430.0);
        section_title(ui, "MASTER SECTION", color::BUS_B);
        ui.add_enabled_ui(!locked, |ui| {
            for b in 0..NUM_BUSES {
                bus_row(ui, b, p, meters, st);
            }
        });
    });
}

fn bus_row(
    ui: &mut egui::Ui,
    b: usize,
    p: &mut MixerParams,
    meters: &MeterState,
    st: &mut UiState,
) {
    let name = p.bus_name(b);
    let bus = &mut p.buses[b];
    let accent = color::bus(b);
    ui.horizontal(|ui| {
        let r = ui.add_sized(
            [62.0, 24.0],
            egui::Button::new(RichText::new(truncate(&name, 9)).strong().color(accent))
                .fill(color::PANEL_HI),
        );
        st.help(&r, "Output bus. Right-click to rename or map to MIDI.");
        r.context_menu(|ui| {
            ui.label("Name:");
            ui.text_edit_singleline(&mut bus.label);
            if ui.button("Parametric EQ…").clicked() {
                st.eq_bus = Some(b);
                ui.close();
            }
            if ui.button("MIDI learn: gain").clicked() {
                st.midi_learn(format!("Bus[{b}].Gain"));
                ui.close();
            }
            if ui.button("MIDI learn: mute").clicked() {
                st.midi_learn(format!("Bus[{b}].Mute"));
                ui.close();
            }
        });
        let mode_resp = ui.menu_button(
            RichText::new(truncate(bus.mode.label(), 11)).size(10.5),
            |ui| {
                for m in BusMode::ALL {
                    if ui.selectable_label(bus.mode == m, m.label()).clicked() {
                        bus.mode = m;
                        ui.close();
                    }
                }
            },
        );
        st.help(
            &mode_resp.response,
            "Multichannel mode: mix-downs, up-mixes, channel extraction, composite.",
        );
        let r = widgets::toggle(ui, &mut bus.eq_on, "EQ", accent, vec2(28.0, 22.0));
        st.help(&r, "6-band parametric EQ on/off. Right-click to edit it.");
        if r.secondary_clicked() {
            st.eq_bus = Some(b);
        }
        let r = widgets::toggle(ui, &mut bus.mono, "M", color::MONO, vec2(22.0, 22.0));
        st.help(&r, "Mono bus.");
        let r = widgets::toggle(ui, &mut bus.mute, "MUTE", color::MUTE, vec2(40.0, 22.0));
        st.help(&r, "Mute this bus.");
        let r = widgets::hfader(ui, &mut bus.gain_db, &meters.buses[b], 150.0, accent);
        st.help(
            &r,
            "Bus gain. Drag (Shift = fine), scroll, double-click for 0 dB.",
        );
        ui.label(RichText::new(widgets::fmt_db(bus.gain_db)).size(10.5));
    });
}
