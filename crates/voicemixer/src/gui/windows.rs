//! Dialog windows opened from the console.

use super::widgets::color;
use super::{PathKind, UiState};
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, RichText, Sense, Stroke};
use std::path::Path;
use std::sync::atomic::Ordering;
use voicemixer_core::dsp::Biquad;
use voicemixer_core::shared::{Devices, Shared};
use voicemixer_core::*;

pub fn show(
    ctx: &egui::Context,
    p: &mut MixerParams,
    st: &mut UiState,
    shared: &Shared,
    _devices: &Devices,
    midi_ports: &[String],
) {
    settings(ctx, p, st, shared);
    vban(ctx, p, st);
    midi(ctx, p, st, shared, midi_ports);
    macros(ctx, p, st);
    bus_eq(ctx, p, st, shared);
    dynamics(ctx, p, st);
    patch(ctx, p, st);
    path_dialog(ctx, st);
    about(ctx, st);
}

fn settings(ctx: &egui::Context, p: &mut MixerParams, st: &mut UiState, shared: &Shared) {
    let mut open = st.show_settings;
    egui::Window::new("Engine settings")
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            let s = &mut p.settings;
            egui::Grid::new("settings")
                .num_columns(2)
                .spacing(vec2(12.0, 6.0))
                .show(ui, |ui| {
                    ui.label("Sample rate");
                    egui::ComboBox::from_id_salt("sr")
                        .selected_text(format!("{} Hz", s.sample_rate))
                        .show_ui(ui, |ui| {
                            for r in [44100, 48000, 88200, 96000] {
                                ui.selectable_value(&mut s.sample_rate, r, format!("{r} Hz"));
                            }
                        });
                    ui.end_row();
                    ui.label("Buffer (PipeWire quantum)");
                    egui::ComboBox::from_id_salt("buf")
                        .selected_text(format!("{} samples", s.buffer_frames))
                        .show_ui(ui, |ui| {
                            for b in [64, 128, 256, 512, 1024, 2048] {
                                ui.selectable_value(
                                    &mut s.buffer_frames,
                                    b,
                                    format!("{b} samples"),
                                );
                            }
                        });
                    ui.end_row();
                    for (label, v) in [
                        ("Virtual input channels", &mut s.virtual_input_channels),
                        (
                            "Virtual output (B1/B2) channels",
                            &mut s.virtual_output_channels,
                        ),
                        ("Hardware output channels", &mut s.hw_output_channels),
                    ] {
                        ui.label(label);
                        ui.horizontal(|ui| {
                            ui.radio_value(v, 2, "Stereo");
                            ui.radio_value(v, 8, "7.1");
                        });
                        ui.end_row();
                    }
                });
            ui.label(
                RichText::new("These settings take effect after restarting VoiceMixer.")
                    .color(color::SOLO)
                    .size(11.0),
            );
            ui.separator();
            ui.label(format!("Backend: {}", shared.status.backend.lock()));
            ui.label(format!(
                "Input underruns: {}",
                shared.status.underruns.load(Ordering::Relaxed)
            ));
            ui.label(format!(
                "Settings file: {}",
                config::default_path().display()
            ));
        });
    st.show_settings = open;
}

fn vban(ctx: &egui::Context, p: &mut MixerParams, st: &mut UiState) {
    let mut open = st.show_vban;
    egui::Window::new("VBAN").open(&mut open).default_width(720.0).collapsible(false).show(ctx, |ui| {
        let v = &mut p.vban;
        ui.horizontal(|ui| {
            ui.checkbox(&mut v.enabled, RichText::new("VBAN enabled").strong());
            ui.label("Listen on UDP port");
            ui.add(egui::DragValue::new(&mut v.listen_port).range(1024..=65535));
        });
        ui.separator();
        ui.label(RichText::new("Incoming streams").strong());
        egui::Grid::new("vban-in").striped(true).num_columns(5).show(ui, |ui| {
            for h in ["On", "Stream name", "From IP (empty = any)", "Into strip"] {
                ui.label(RichText::new(h).color(color::DIM));
            }
            ui.end_row();
            for (i, s) in v.incoming.iter_mut().enumerate() {
                ui.checkbox(&mut s.enabled, "");
                ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(120.0).char_limit(16));
                ui.add(egui::TextEdit::singleline(&mut s.ip).desired_width(130.0));
                egui::ComboBox::from_id_salt(("vin", i)).selected_text(default_strip_name(s.strip)).show_ui(ui, |ui| {
                    for k in 0..NUM_STRIPS {
                        ui.selectable_value(&mut s.strip, k, default_strip_name(k));
                    }
                });
                ui.end_row();
            }
        });
        ui.separator();
        ui.label(RichText::new("Outgoing streams").strong());
        egui::Grid::new("vban-out").striped(true).num_columns(7).show(ui, |ui| {
            for h in ["On", "Stream name", "To IP", "Port", "From bus", "Channels", "Format"] {
                ui.label(RichText::new(h).color(color::DIM));
            }
            ui.end_row();
            for (i, s) in v.outgoing.iter_mut().enumerate() {
                ui.checkbox(&mut s.enabled, "");
                ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(110.0).char_limit(16));
                ui.add(egui::TextEdit::singleline(&mut s.ip).desired_width(120.0));
                ui.add(egui::DragValue::new(&mut s.port).range(1..=65535));
                egui::ComboBox::from_id_salt(("vout-bus", i)).width(50.0).selected_text(BUS_NAMES[s.bus]).show_ui(ui, |ui| {
                    for (b, name) in BUS_NAMES.iter().enumerate() {
                        ui.selectable_value(&mut s.bus, b, *name);
                    }
                });
                ui.add(egui::DragValue::new(&mut s.channels).range(1..=8));
                egui::ComboBox::from_id_salt(("vout-fmt", i)).width(70.0).selected_text(s.format.label()).show_ui(ui, |ui| {
                    for f in VbanFormat::ALL {
                        ui.selectable_value(&mut s.format, f, f.label());
                    }
                });
                ui.end_row();
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.checkbox(&mut v.text_enabled, "Accept VBAN-TEXT commands from stream");
            ui.add(egui::TextEdit::singleline(&mut v.text_stream).desired_width(110.0).hint_text("any"));
        });
        ui.label(
            RichText::new("Text commands use the same syntax as `voicemixer ctl`, e.g. Strip[0].Mute = 1. Anyone on your network can send them, so only enable this on a trusted network.")
                .size(10.5)
                .color(color::DIM),
        );
    });
    st.show_vban = open;
}

fn midi(
    ctx: &egui::Context,
    p: &mut MixerParams,
    st: &mut UiState,
    shared: &Shared,
    ports: &[String],
) {
    let mut open = st.show_midi;
    egui::Window::new("MIDI mapping")
        .open(&mut open)
        .default_width(640.0)
        .collapsible(false)
        .show(ctx, |ui| {
            let m = &mut p.midi;
            ui.horizontal(|ui| {
                ui.label("Input device");
                let shown = if m.input_device.is_empty() {
                    "— none —".to_string()
                } else {
                    m.input_device.clone()
                };
                egui::ComboBox::from_id_salt("midi-dev")
                    .width(320.0)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut m.input_device, String::new(), "— none —");
                        for port in ports {
                            ui.selectable_value(&mut m.input_device, port.clone(), port);
                        }
                    });
            });
            if cfg!(not(feature = "midi")) {
                ui.label(
                    RichText::new(
                        "This build has no MIDI support (built without the \"midi\" feature).",
                    )
                    .color(color::SOLO),
                );
            }
            ui.separator();
            let learning = shared.midi_learn.lock().clone();
            ui.horizontal(|ui| {
                ui.label("Learn:");
                ui.add(
                    egui::TextEdit::singleline(&mut st.learn_target)
                        .hint_text("Strip[0].Gain")
                        .desired_width(220.0),
                );
                if let Some(t) = &learning {
                    ui.label(
                        RichText::new(format!("move a control to bind {t}…")).color(color::SOLO),
                    );
                    if ui.button("Cancel").clicked() {
                        *shared.midi_learn.lock() = None;
                    }
                } else if ui
                    .add_enabled(
                        !st.learn_target.trim().is_empty(),
                        egui::Button::new("Learn"),
                    )
                    .clicked()
                {
                    *shared.midi_learn.lock() = Some(st.learn_target.trim().to_string());
                }
            });
            ui.separator();
            let mut remove = None;
            egui::ScrollArea::vertical()
                .max_height(300.0)
                .show(ui, |ui| {
                    egui::Grid::new("midi-map")
                        .striped(true)
                        .num_columns(7)
                        .show(ui, |ui| {
                            for h in ["Type", "Ch", "No.", "Parameter", "Min", "Max", "Mode", ""] {
                                ui.label(RichText::new(h).color(color::DIM));
                            }
                            ui.end_row();
                            for (k, b) in m.bindings.iter_mut().enumerate() {
                                egui::ComboBox::from_id_salt(("mk", k))
                                    .width(50.0)
                                    .selected_text(if b.kind == MidiKind::Cc {
                                        "CC"
                                    } else {
                                        "Note"
                                    })
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut b.kind, MidiKind::Cc, "CC");
                                        ui.selectable_value(&mut b.kind, MidiKind::Note, "Note");
                                    });
                                let mut ch = b.channel + 1;
                                ui.add(egui::DragValue::new(&mut ch).range(1..=16));
                                b.channel = ch - 1;
                                ui.add(egui::DragValue::new(&mut b.number).range(0..=127));
                                ui.add(
                                    egui::TextEdit::singleline(&mut b.target).desired_width(150.0),
                                );
                                ui.add(egui::DragValue::new(&mut b.min).speed(0.1));
                                ui.add(egui::DragValue::new(&mut b.max).speed(0.1));
                                egui::ComboBox::from_id_salt(("mm", k))
                                    .width(80.0)
                                    .selected_text(format!("{:?}", b.mode))
                                    .show_ui(ui, |ui| {
                                        for mode in [
                                            MidiMode::Absolute,
                                            MidiMode::Toggle,
                                            MidiMode::Momentary,
                                        ] {
                                            ui.selectable_value(
                                                &mut b.mode,
                                                mode,
                                                format!("{mode:?}"),
                                            );
                                        }
                                    });
                                if ui.button("x").on_hover_text("Remove").clicked() {
                                    remove = Some(k);
                                }
                                ui.end_row();
                            }
                        });
                });
            if let Some(k) = remove {
                m.bindings.remove(k);
            }
            if ui.button("Add binding").clicked() {
                m.bindings.push(MidiBinding {
                    target: "Strip[0].Gain".into(),
                    min: GAIN_MIN_DB,
                    max: GAIN_MAX_DB,
                    ..Default::default()
                });
            }
        });
    st.show_midi = open;
}

fn macros(ctx: &egui::Context, p: &mut MixerParams, st: &mut UiState) {
    let mut open = st.show_macros;
    egui::Window::new("Macro Buttons")
        .open(&mut open)
        .default_width(520.0)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut st.macros_edit, "Edit");
                ui.label(
                    RichText::new("Each button runs a script on press and on release.")
                        .size(10.5)
                        .color(color::DIM),
                );
            });
            st.macro_held.resize(p.macros.len(), false);
            if st.macros_edit {
                let mut remove = None;
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .show(ui, |ui| {
                        for (i, m) in p.macros.iter_mut().enumerate() {
                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(format!("#{}", i + 1));
                                    ui.add(
                                        egui::TextEdit::singleline(&mut m.label)
                                            .desired_width(160.0),
                                    );
                                    ui.checkbox(&mut m.toggle, "Toggle");
                                    if ui.button("Remove").clicked() {
                                        remove = Some(i);
                                    }
                                });
                                ui.label(
                                    RichText::new(if m.toggle {
                                        "When switched on:"
                                    } else {
                                        "On press:"
                                    })
                                    .size(10.5),
                                );
                                ui.add(
                                    egui::TextEdit::multiline(&mut m.on_press)
                                        .desired_rows(2)
                                        .desired_width(f32::INFINITY)
                                        .code_editor(),
                                );
                                ui.label(
                                    RichText::new(if m.toggle {
                                        "When switched off:"
                                    } else {
                                        "On release:"
                                    })
                                    .size(10.5),
                                );
                                ui.add(
                                    egui::TextEdit::multiline(&mut m.on_release)
                                        .desired_rows(2)
                                        .desired_width(f32::INFINITY)
                                        .code_editor(),
                                );
                            });
                        }
                    });
                if let Some(i) = remove {
                    p.macros.remove(i);
                }
                if ui.button("Add button").clicked() {
                    p.macros.push(MacroButton {
                        label: format!("Button {}", p.macros.len() + 1),
                        ..Default::default()
                    });
                }
            } else {
                egui::Grid::new("macro-grid")
                    .spacing(vec2(6.0, 6.0))
                    .show(ui, |ui| {
                        for (i, m) in p.macros.iter().enumerate() {
                            let lit = m.state || st.macro_held[i];
                            let fill = if lit { color::VIRT } else { color::OFF };
                            let fg = if lit { Color32::BLACK } else { color::TEXT };
                            let r = ui.add_sized(
                                [118.0, 48.0],
                                egui::Button::new(RichText::new(&m.label).color(fg).strong())
                                    .fill(fill)
                                    .sense(Sense::click_and_drag()),
                            );
                            let down = r.is_pointer_button_down_on();
                            if down && !st.macro_held[i] {
                                st.pending.push(Action::MacroDown(i));
                            } else if !down && st.macro_held[i] {
                                st.pending.push(Action::MacroUp(i));
                            }
                            st.macro_held[i] = down;
                            if (i + 1) % 4 == 0 {
                                ui.end_row();
                            }
                        }
                    });
            }
        });
    st.show_macros = open;
}

const EQ_MIN_F: f32 = 20.0;
const EQ_MAX_F: f32 = 20000.0;
const EQ_RANGE_DB: f32 = 18.0;

fn bus_eq(ctx: &egui::Context, p: &mut MixerParams, st: &mut UiState, shared: &Shared) {
    let Some(b) = st.eq_bus else { return };
    let mut open = true;
    let fs = (shared.status.sample_rate.load(Ordering::Relaxed).max(8000)) as f32;
    let title = format!("{} — parametric EQ", p.bus_name(b));
    egui::Window::new(title)
        .id(egui::Id::new("bus-eq"))
        .open(&mut open)
        .default_width(560.0)
        .collapsible(false)
        .show(ctx, |ui| {
            let bus = &mut p.buses[b];
            ui.horizontal(|ui| {
                ui.checkbox(&mut bus.eq_on, RichText::new("EQ on").strong());
                ui.separator();
                ui.label("Channel");
                const CH: [&str; MAX_CHANNELS] = ["FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR"];
                for (c, name) in CH.iter().enumerate() {
                    ui.selectable_value(&mut st.eq_channel, c, *name);
                }
                ui.separator();
                ui.checkbox(&mut st.eq_link, "Link all channels");
            });
            let c = st.eq_channel.min(MAX_CHANNELS - 1);
            let before = bus.eq[c].clone();

            // Response curve with draggable band handles.
            let (rect, resp) = ui.allocate_exact_size(
                vec2(ui.available_width().max(520.0), 170.0),
                Sense::click_and_drag(),
            );
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 4.0, color::TRACK);
            let fx = |f: f32| {
                rect.left() + rect.width() * ((f / EQ_MIN_F).ln() / (EQ_MAX_F / EQ_MIN_F).ln())
            };
            let fy = |db: f32| rect.center().y - db / EQ_RANGE_DB * rect.height() * 0.5;
            let x_to_f = |x: f32| {
                EQ_MIN_F
                    * (EQ_MAX_F / EQ_MIN_F).powf(((x - rect.left()) / rect.width()).clamp(0.0, 1.0))
            };
            let y_to_db = |y: f32| {
                ((rect.center().y - y) / (rect.height() * 0.5) * EQ_RANGE_DB)
                    .clamp(-EQ_RANGE_DB, EQ_RANGE_DB)
            };
            for f in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0] {
                let x = fx(f);
                painter.line_segment(
                    [pos2(x, rect.top()), pos2(x, rect.bottom())],
                    Stroke::new(1.0_f32, color::PANEL),
                );
                let label = if f >= 1000.0 {
                    format!("{}k", f / 1000.0)
                } else {
                    format!("{f}")
                };
                painter.text(
                    pos2(x + 2.0, rect.bottom() - 2.0),
                    Align2::LEFT_BOTTOM,
                    label,
                    FontId::proportional(9.0),
                    color::DIM,
                );
            }
            for db in [-12.0, -6.0, 0.0, 6.0, 12.0] {
                let y = fy(db);
                painter.line_segment(
                    [pos2(rect.left(), y), pos2(rect.right(), y)],
                    Stroke::new(
                        1.0_f32,
                        if db == 0.0 {
                            color::PANEL_HI
                        } else {
                            color::PANEL
                        },
                    ),
                );
                painter.text(
                    pos2(rect.left() + 2.0, y - 1.0),
                    Align2::LEFT_BOTTOM,
                    format!("{db:+}"),
                    FontId::proportional(9.0),
                    color::DIM,
                );
            }
            let filters: Vec<Biquad> = bus.eq[c]
                .iter()
                .filter(|band| band.on && (!band.kind.uses_gain() || band.gain_db.abs() > 0.01))
                .map(|band| Biquad::new(band.kind, fs, band.freq, band.q, band.gain_db))
                .collect();
            let points: Vec<egui::Pos2> = (0..=200)
                .map(|k| {
                    let f = EQ_MIN_F * (EQ_MAX_F / EQ_MIN_F).powf(k as f32 / 200.0);
                    let db: f32 = filters.iter().map(|b| b.magnitude_db(f, fs)).sum();
                    pos2(fx(f), fy(db.clamp(-EQ_RANGE_DB * 1.5, EQ_RANGE_DB * 1.5)))
                })
                .collect();
            let curve_color = if bus.eq_on { color::bus(b) } else { color::DIM };
            painter.add(egui::Shape::line(points, Stroke::new(2.0_f32, curve_color)));
            for (k, band) in bus.eq[c].iter().enumerate() {
                let pos = pos2(
                    fx(band.freq),
                    fy(if band.kind.uses_gain() {
                        band.gain_db
                    } else {
                        0.0
                    }),
                );
                painter.circle_filled(pos, 6.0, if band.on { curve_color } else { color::OFF });
                painter.text(
                    pos,
                    Align2::CENTER_CENTER,
                    format!("{}", k + 1),
                    FontId::proportional(9.0),
                    Color32::BLACK,
                );
            }
            if resp.drag_started() {
                if let Some(ptr) = resp.interact_pointer_pos() {
                    st.eq_drag = bus.eq[c]
                        .iter()
                        .enumerate()
                        .map(|(k, band)| {
                            (
                                k,
                                pos2(
                                    fx(band.freq),
                                    fy(if band.kind.uses_gain() {
                                        band.gain_db
                                    } else {
                                        0.0
                                    }),
                                )
                                .distance(ptr),
                            )
                        })
                        .filter(|(_, d)| *d < 14.0)
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(k, _)| k);
                }
            }
            if resp.dragged() {
                if let (Some(k), Some(ptr)) = (st.eq_drag, resp.interact_pointer_pos()) {
                    let band = &mut bus.eq[c][k];
                    band.freq = x_to_f(ptr.x).round();
                    if band.kind.uses_gain() {
                        band.gain_db = (y_to_db(ptr.y) * 10.0).round() / 10.0;
                    }
                }
            }
            if resp.drag_stopped() {
                st.eq_drag = None;
            }
            if resp.hovered() {
                let scroll = ui.input(|i| i.raw_scroll_delta.y);
                if scroll != 0.0 {
                    if let Some(k) = st
                        .eq_drag
                        .or_else(|| nearest_band(&bus.eq[c], resp.hover_pos(), &fx))
                    {
                        let q = &mut bus.eq[c][k].q;
                        *q = (*q * if scroll > 0.0 { 1.1 } else { 1.0 / 1.1 }).clamp(0.3, 100.0);
                    }
                }
            }

            egui::Grid::new("eq-bands")
                .striped(true)
                .num_columns(6)
                .show(ui, |ui| {
                    for h in ["Band", "On", "Type", "Frequency", "Gain", "Q"] {
                        ui.label(RichText::new(h).color(color::DIM));
                    }
                    ui.end_row();
                    for (k, band) in bus.eq[c].iter_mut().enumerate() {
                        ui.label(format!("{}", k + 1));
                        ui.checkbox(&mut band.on, "");
                        egui::ComboBox::from_id_salt(("eqk", k))
                            .width(90.0)
                            .selected_text(band.kind.name())
                            .show_ui(ui, |ui| {
                                for kind in PeqKind::ALL {
                                    ui.selectable_value(&mut band.kind, kind, kind.name());
                                }
                            });
                        let speed = band.freq * 0.01;
                        ui.add(
                            egui::DragValue::new(&mut band.freq)
                                .range(20.0..=20000.0)
                                .speed(speed)
                                .suffix(" Hz"),
                        );
                        ui.add_enabled(
                            band.kind.uses_gain(),
                            egui::DragValue::new(&mut band.gain_db)
                                .range(-36.0..=18.0)
                                .speed(0.1)
                                .suffix(" dB"),
                        );
                        ui.add(
                            egui::DragValue::new(&mut band.q)
                                .range(0.3..=100.0)
                                .speed(0.02),
                        );
                        ui.end_row();
                    }
                });
            ui.label(
                RichText::new(
                    "Drag a numbered point to set frequency and gain; scroll over it to change Q.",
                )
                .size(10.5)
                .color(color::DIM),
            );
            if st.eq_link && bus.eq[c] != before {
                let bands = bus.eq[c].clone();
                for ch in bus.eq.iter_mut() {
                    *ch = bands.clone();
                }
            }
        });
    if !open {
        st.eq_bus = None;
    }
}

fn nearest_band(
    bands: &[PeqBand],
    pos: Option<egui::Pos2>,
    fx: &impl Fn(f32) -> f32,
) -> Option<usize> {
    let x = pos?.x;
    bands
        .iter()
        .enumerate()
        .map(|(k, b)| (k, (fx(b.freq) - x).abs()))
        .filter(|(_, d)| *d < 20.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(k, _)| k)
}

fn dynamics(ctx: &egui::Context, p: &mut MixerParams, st: &mut UiState) {
    let Some(i) = st.dyn_strip else { return };
    let mut open = true;
    let title = format!("{} — compressor & gate", p.strip_name(i));
    egui::Window::new(title)
        .id(egui::Id::new("dynamics"))
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            let s = &mut p.strips[i];
            ui.columns(2, |cols| {
                let ui = &mut cols[0];
                ui.label(RichText::new("Compressor").strong().color(color::HW));
                ui.add(egui::Slider::new(&mut s.comp.amount, 0.0..=10.0).text("amount (ratio)"));
                ui.label(format!("ratio {:.1}:1", s.comp.ratio()));
                ui.add(
                    egui::Slider::new(&mut s.comp.input_gain_db, -24.0..=24.0)
                        .text("input gain dB"),
                );
                ui.add(
                    egui::Slider::new(&mut s.comp.threshold_db, -60.0..=0.0).text("threshold dB"),
                );
                ui.add(
                    egui::Slider::new(&mut s.comp.attack_ms, 0.1..=200.0)
                        .logarithmic(true)
                        .text("attack ms"),
                );
                ui.add(
                    egui::Slider::new(&mut s.comp.release_ms, 1.0..=5000.0)
                        .logarithmic(true)
                        .text("release ms"),
                );
                ui.add(egui::Slider::new(&mut s.comp.knee, 0.0..=1.0).text("knee"));
                ui.checkbox(&mut s.comp.auto_makeup, "automatic make-up gain");
                ui.add(
                    egui::Slider::new(&mut s.comp.output_gain_db, -24.0..=24.0)
                        .text("output gain dB"),
                );
                if ui.button("Reset compressor").clicked() {
                    s.comp = CompParams::default();
                }
                let ui = &mut cols[1];
                ui.label(RichText::new("Gate").strong().color(color::HW));
                ui.add(egui::Slider::new(&mut s.gate.amount, 0.0..=10.0).text("amount"));
                ui.label(format!("threshold {:.0} dB", s.gate.threshold_db()));
                ui.add(egui::Slider::new(&mut s.gate.damping_db, -60.0..=0.0).text("damping dB"));
                ui.add(
                    egui::Slider::new(&mut s.gate.sidechain_hz, 20.0..=4000.0)
                        .logarithmic(true)
                        .text("detector HPF Hz"),
                );
                ui.add(
                    egui::Slider::new(&mut s.gate.attack_ms, 0.1..=1000.0)
                        .logarithmic(true)
                        .text("attack ms"),
                );
                ui.add(egui::Slider::new(&mut s.gate.hold_ms, 0.0..=5000.0).text("hold ms"));
                ui.add(
                    egui::Slider::new(&mut s.gate.release_ms, 1.0..=5000.0)
                        .logarithmic(true)
                        .text("release ms"),
                );
                if ui.button("Reset gate").clicked() {
                    s.gate = GateParams::default();
                }
            });
        });
    if !open {
        st.dyn_strip = None;
    }
}

fn patch(ctx: &egui::Context, p: &mut MixerParams, st: &mut UiState) {
    let mut open = st.show_patch;
    egui::Window::new("Composite patch").open(&mut open).resizable(false).collapsible(false).show(ctx, |ui| {
        ui.label(RichText::new("A bus in Composite mode takes each of its channels from the input channel chosen here (post-fader).").size(10.5).color(color::DIM));
        const CH: [&str; MAX_CHANNELS] = ["FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR"];
        egui::Grid::new("patch").num_columns(2).show(ui, |ui| {
            for (k, name) in CH.iter().enumerate() {
                ui.label(format!("Bus channel {} ({name})", k + 1));
                egui::ComboBox::from_id_salt(("patch", k)).selected_text(composite_label(p.composite[k])).show_ui(ui, |ui| {
                    for code in 0..=COMPOSITE_SOURCES {
                        ui.selectable_value(&mut p.composite[k], code, composite_label(code));
                    }
                });
                ui.end_row();
            }
        });
    });
    st.show_patch = open;
}

fn wanted_extension(kind: PathKind, path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match kind {
        PathKind::SavePreset | PathKind::LoadPreset => ext == "toml",
        PathKind::ImportBanana => ext == "xml",
        PathKind::TapeLoad => {
            matches!(
                ext.as_str(),
                "wav"
                    | "flac"
                    | "mp3"
                    | "ogg"
                    | "oga"
                    | "m4a"
                    | "aac"
                    | "mp4"
                    | "mkv"
                    | "webm"
                    | "aiff"
                    | "aif"
                    | "caf"
            )
        }
    }
}

fn path_dialog(ctx: &egui::Context, st: &mut UiState) {
    let Some(dlg) = st.path_dialog.as_mut() else {
        return;
    };
    let title = match dlg.kind {
        PathKind::SavePreset => "Save preset",
        PathKind::LoadPreset => "Load preset",
        PathKind::ImportBanana => "Import Voicemeeter settings",
        PathKind::TapeLoad => "Load audio file",
    };
    let mut open = true;
    let mut confirm = false;
    egui::Window::new(title).id(egui::Id::new("path-dialog")).open(&mut open).default_width(520.0).collapsible(false).show(ctx, |ui| {
        ui.horizontal(|ui| {
            if ui.button("Up").clicked() {
                if let Some(parent) = dlg.dir.parent() {
                    dlg.dir = parent.to_path_buf();
                }
            }
            ui.label(RichText::new(dlg.dir.display().to_string()).monospace().size(11.0));
        });
        if dlg.kind == PathKind::ImportBanana {
            ui.label(RichText::new("Copy VoicemeeterBanana_LastSettings.xml (from Documents\\Voicemeeter on Windows) and pick it here.").size(10.5).color(color::DIM));
        }
        let mut entries: Vec<(bool, std::path::PathBuf)> = std::fs::read_dir(&dlg.dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
                    .map(|p| (p.is_dir(), p))
                    .filter(|(is_dir, p)| *is_dir || wanted_extension(dlg.kind, p))
                    .collect()
            })
            .unwrap_or_default();
        entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        egui::ScrollArea::vertical().max_height(280.0).auto_shrink([false, true]).show(ui, |ui| {
            for (is_dir, path) in entries {
                let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                let text = if is_dir { format!("{name}/") } else { name.clone() };
                let r = ui.selectable_label(!is_dir && dlg.name == name, text);
                if r.clicked() {
                    if is_dir {
                        dlg.dir = path.clone();
                    } else {
                        dlg.name = name.clone();
                    }
                }
                if r.double_clicked() && !is_dir {
                    dlg.name = name;
                    confirm = true;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Name:");
            let r = ui.add(egui::TextEdit::singleline(&mut dlg.name).desired_width(320.0));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                confirm = true;
            }
            if ui.add_enabled(!dlg.name.trim().is_empty(), egui::Button::new("OK")).clicked() {
                confirm = true;
            }
        });
    });
    if confirm {
        let dlg = st.path_dialog.take().expect("dialog is open");
        let mut path = dlg.dir.join(dlg.name.trim());
        if dlg.kind == PathKind::SavePreset && path.extension().is_none() {
            path.set_extension("toml");
        }
        let text = path.to_string_lossy().into_owned();
        st.pending.push(match dlg.kind {
            PathKind::SavePreset => Action::SavePreset(text),
            PathKind::LoadPreset | PathKind::ImportBanana => Action::LoadPreset(text),
            PathKind::TapeLoad => Action::TapeLoad(text),
        });
    } else if !open {
        st.path_dialog = None;
    }
}

fn about(ctx: &egui::Context, st: &mut UiState) {
    let mut open = st.show_about;
    egui::Window::new("About VoiceMixer").open(&mut open).default_width(560.0).collapsible(false).show(ctx, |ui| {
        ui.label(RichText::new(format!("VoiceMixer {}", env!("CARGO_PKG_VERSION"))).strong().size(16.0).color(color::HW));
        ui.label("Virtual mixing console for Linux and PipeWire: 3 hardware inputs, 2 virtual inputs, 3 hardware outputs (A1–A3) and 2 virtual outputs (B1, B2).");
        ui.separator();
        ui.label(RichText::new("Getting started").strong());
        ui.label("1. Pick your microphone in Hardware Input 1 and your headphones/speakers in A1.");
        ui.label("2. In your desktop's sound settings, choose \"VoiceMixer Input\" as the output device.");
        ui.label("3. In Discord, OBS, etc. choose \"VoiceMixer B1\" as the microphone.");
        ui.label("4. Use the A/B buttons on each strip to decide who hears what.");
        ui.separator();
        ui.label(RichText::new("Remote control").strong());
        ui.label(RichText::new("voicemixer ctl 'Strip[0].Gain = -6; Bus[3].Mute = 1'\nvoicemixer ctl get Strip[0].Gain\nvoicemixer ctl 'Recorder.Record = 1'").monospace());
        ui.label("The same commands work from macro buttons, MIDI bindings and VBAN-TEXT.");
    });
    st.show_about = open;
}
