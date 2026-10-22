//! VoiceMixer window, laid out like a hardware console: hardware inputs,
//! virtual inputs, then outputs, tape deck and the master section.

mod panels;
mod widgets;
mod windows;

use eframe::egui::{self, Color32, RichText};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;
use voicemixer_core::*;
use widgets::color;

use crate::tray::{Request, WindowControl};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::platform::run_on_demand::EventLoopExtRunOnDemand;
use winit::window::WindowId;

/// The window. winit allows one event loop per process, so it outlives each
/// window and is reused whenever the window comes back from the tray.
pub struct Gui {
    event_loop: EventLoop<eframe::UserEvent>,
}

impl Gui {
    pub fn new() -> anyhow::Result<Self> {
        let event_loop = EventLoop::with_user_event().build()?;
        Ok(Self { event_loop })
    }

    /// Shows the window until it is closed. Closing does not stop the
    /// audio; `main` decides whether that was a quit or a hide to the tray.
    pub fn run(&mut self, ctl: Controller, window: &WindowControl) -> anyhow::Result<()> {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title("VoiceMixer")
                .with_app_id("voicemixer")
                .with_icon(crate::app_icon())
                .with_inner_size([1240.0, 690.0])
                .with_min_inner_size([1040.0, 600.0]),
            ..Default::default()
        };
        let win = window.clone();
        let mut app = eframe::create_native(
            "VoiceMixer",
            options,
            Box::new(move |cc| Ok(Box::new(App::new(cc, ctl, win)))),
            &self.event_loop,
        );
        let result = self.event_loop.run_app_on_demand(&mut app);
        window.attach(None);
        drop(app);
        // Dropping a window only flags it; winit destroys it on its next
        // loop iteration. Without one, the closed window would stay on
        // screen (Wayland) while VoiceMixer sits in the tray.
        self.event_loop.run_app_on_demand(&mut OneIteration)?;
        result.map_err(|e| anyhow::anyhow!("GUI: {e}"))
    }
}

struct OneIteration;

impl ApplicationHandler<eframe::UserEvent> for OneIteration {
    fn resumed(&mut self, _: &ActiveEventLoop) {}

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.exit();
    }
}

#[derive(Default)]
struct MeterState {
    strips: [[f32; MAX_CHANNELS]; NUM_STRIPS],
    buses: [[f32; MAX_CHANNELS]; NUM_BUSES],
    tape: [f32; 2],
    comp_gr: [f32; NUM_STRIPS],
    gate_open: [bool; NUM_STRIPS],
}

impl MeterState {
    /// Pulls the current decaying display values. The actual ballistics run
    /// centrally (see `shared::Meters::tick`, driven by a background thread
    /// started in `main.rs`), so this is a plain read: any number of GUI
    /// frames or `ctl levels` calls can look at the same values without
    /// stepping on each other or needing the GUI to be open at all.
    fn update(&mut self, m: &shared::Meters) {
        for s in 0..NUM_STRIPS {
            for c in 0..MAX_CHANNELS {
                self.strips[s][c] = m.strip_channel(s, c);
            }
            self.comp_gr[s] = f32::from_bits(m.comp_gr[s].load(Ordering::Relaxed));
            self.gate_open[s] = m.gate_open[s].load(Ordering::Relaxed);
        }
        for b in 0..NUM_BUSES {
            for c in 0..MAX_CHANNELS {
                self.buses[b][c] = m.bus_channel(b, c);
            }
        }
        for c in 0..2 {
            self.tape[c] = m.tape_channel(c);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PathKind {
    SavePreset,
    LoadPreset,
    ImportBanana,
    TapeLoad,
}

struct PathDialog {
    kind: PathKind,
    dir: PathBuf,
    name: String,
}

#[derive(Default)]
pub struct UiState {
    show_settings: bool,
    show_vban: bool,
    show_midi: bool,
    show_macros: bool,
    macros_edit: bool,
    show_patch: bool,
    show_about: bool,
    eq_bus: Option<usize>,
    eq_channel: usize,
    eq_link: bool,
    /// Band being dragged in the EQ curve.
    eq_drag: Option<usize>,
    dyn_strip: Option<usize>,
    clipboard: Option<StripParams>,
    path_dialog: Option<PathDialog>,
    learn_target: String,
    macro_held: Vec<bool>,
    hover_help: Option<&'static str>,
    /// Help text collected during the previous frame, shown in the status bar.
    last_help: Option<&'static str>,
    pending: Vec<Action>,
}

impl UiState {
    fn help(&mut self, resp: &egui::Response, text: &'static str) {
        if resp.hovered() {
            self.hover_help = Some(text);
        }
    }

    fn open_path(&mut self, kind: PathKind) {
        let dir = match kind {
            PathKind::SavePreset | PathKind::LoadPreset => config::presets_dir(),
            PathKind::ImportBanana | PathKind::TapeLoad => home_dir(),
        };
        let _ = std::fs::create_dir_all(config::presets_dir());
        self.path_dialog = Some(PathDialog {
            kind,
            dir,
            name: String::new(),
        });
    }

    fn midi_learn(&mut self, target: String) {
        self.learn_target = target;
        self.show_midi = true;
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

struct App {
    ctl: Controller,
    window: WindowControl,
    meters: MeterState,
    ui: UiState,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, ctl: Controller, window: WindowControl) -> Self {
        window.attach(Some(cc.egui_ctx.clone()));
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = color::BG;
        visuals.window_fill = color::PANEL;
        visuals.extreme_bg_color = color::TRACK;
        visuals.widgets.inactive.weak_bg_fill = color::OFF;
        visuals.selection.bg_fill = color::HW.gamma_multiply(0.6);
        cc.egui_ctx.set_visuals(visuals);
        cc.egui_ctx.style_mut(|s| {
            s.spacing.item_spacing = egui::vec2(4.0, 4.0);
            s.spacing.button_padding = egui::vec2(4.0, 2.0);
        });
        Self {
            ctl,
            window,
            meters: MeterState::default(),
            ui: UiState::default(),
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(33));
        while let Some(r) = self.window.try_recv() {
            match r {
                // Raises (and un-minimises) the window where the compositor
                // allows it; Wayland may only flash it in the taskbar.
                Request::Show => ctx.send_viewport_cmd(egui::ViewportCommand::Focus),
                Request::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
        let shared = self.ctl.shared.clone();
        self.meters.update(&shared.meters);
        self.ui.last_help = self.ui.hover_help.take();
        let devices = shared.devices.lock().clone();
        let midi_ports = shared.midi_ports.lock().clone();

        // The GUI edits the parameters in place while holding the lock; the
        // audio thread only ever try-locks, so it is never blocked by this.
        let mut p = shared.params.lock();
        let before = p.clone();

        egui::TopBottomPanel::top("top")
            .frame(egui::Frame::NONE.fill(color::PANEL).inner_margin(6.0))
            .show(ctx, |ui| {
                self.top_bar(ui, &mut p);
            });
        egui::TopBottomPanel::bottom("status")
            .frame(egui::Frame::NONE.fill(color::PANEL).inner_margin(4.0))
            .show(ctx, |ui| {
                self.status_bar(ui);
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(color::BG).inner_margin(8.0))
            .show(ctx, |ui| {
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            let locked = p.gui_locked;
                            for i in 0..NUM_STRIPS {
                                if i == NUM_HW_STRIPS {
                                    ui.add_space(6.0);
                                }
                                let name = p.strip_name(i);
                                panels::strip(
                                    ui,
                                    i,
                                    &mut p.strips[i],
                                    &name,
                                    &devices,
                                    &self.meters,
                                    locked,
                                    &mut self.ui,
                                );
                            }
                            ui.add_space(6.0);
                            ui.vertical(|ui| {
                                panels::hardware_out(ui, &mut p, &devices, &mut self.ui);
                                panels::tape(ui, &mut p, &shared, &self.meters, &mut self.ui);
                                panels::master(ui, &mut p, &self.meters, &mut self.ui);
                            });
                        });
                    });
            });

        windows::show(ctx, &mut p, &mut self.ui, &shared, &devices, &midi_ports);

        let changed = *p != before;
        let devices_changed = p
            .strips
            .iter()
            .zip(&before.strips)
            .any(|(a, b)| a.device != b.device)
            || p.buses
                .iter()
                .zip(&before.buses)
                .any(|(a, b)| a.device != b.device);
        drop(p);
        if changed {
            shared.bump();
        }
        if devices_changed {
            self.ctl.send(Action::ReconnectDevices);
        }
        for a in self.ui.pending.drain(..) {
            self.ctl.send(a);
        }
    }
}

impl App {
    fn top_bar(&mut self, ui: &mut egui::Ui, p: &mut MixerParams) {
        let shared = self.ctl.shared.clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new("VoiceMixer").size(20.0).strong().color(color::HW));
            ui.label(RichText::new("virtual mixing console").size(11.0).color(color::DIM));
            ui.add_space(12.0);
            ui.menu_button(RichText::new("Menu").strong(), |ui| {
                if ui.button("Save preset…").clicked() {
                    self.ui.open_path(PathKind::SavePreset);
                    ui.close();
                }
                if ui.button("Load preset…").clicked() {
                    self.ui.open_path(PathKind::LoadPreset);
                    ui.close();
                }
                if ui.button("Import Voicemeeter settings (XML)…").clicked() {
                    self.ui.open_path(PathKind::ImportBanana);
                    ui.close();
                }
                ui.separator();
                if ui.button("Reconnect audio devices").clicked() {
                    self.ui.pending.push(Action::ReconnectDevices);
                    ui.close();
                }
                if ui.button("Reset all settings").clicked() {
                    self.ui.pending.push(Action::Reset);
                    ui.close();
                }
                ui.separator();
                if ui.button("Composite patch…").clicked() {
                    self.ui.show_patch = true;
                    ui.close();
                }
                if ui.button("MIDI mapping…").clicked() {
                    self.ui.show_midi = true;
                    ui.close();
                }
                if ui.button("Engine settings…").clicked() {
                    self.ui.show_settings = true;
                    ui.close();
                }
                ui.checkbox(&mut p.gui_locked, "Lock controls");
                ui.separator();
                if ui.button("Help / About").clicked() {
                    self.ui.show_about = true;
                    ui.close();
                }
                ui.separator();
                if self.window.in_tray() {
                    let r = ui.button("Hide to tray");
                    self.ui.help(&r, "Close the window; audio keeps running. Click the tray icon to bring it back.");
                    if r.clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        ui.close();
                    }
                }
                if ui.button("Quit").clicked() {
                    self.window.request(Request::Quit);
                    ui.close();
                }
            });
            let vban_on = p.vban.enabled;
            let r = ui.add(egui::Button::new(RichText::new("VBAN").strong().color(if vban_on { Color32::BLACK } else { color::TEXT }))
                .fill(if vban_on { color::BUS_B } else { color::OFF }));
            if r.clicked() {
                self.ui.show_vban = true;
            }
            self.ui.help(&r, "VBAN: send and receive audio over the network, and accept text commands.");
            if ui.button("Macro Buttons").clicked() {
                self.ui.show_macros = true;
            }
            if ui.button("Settings").clicked() {
                self.ui.show_settings = true;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let running = shared.status.running.load(Ordering::Relaxed);
                let backend = shared.status.backend.lock().clone();
                let r = widgets::chip(ui, &format!("AUDIO {backend}"), running, color::BUS_A);
                self.ui.help(&r, "Audio engine status and backend.");
                let rate = shared.status.sample_rate.load(Ordering::Relaxed);
                ui.label(RichText::new(format!("{} Hz · {} smp", rate, p.settings.buffer_frames)).size(11.0).color(color::DIM));
                let clients = shared.status.remote_clients.load(Ordering::Relaxed);
                let r = widgets::chip(ui, &format!("REMOTE {clients}"), clients > 0, color::VIRT);
                self.ui.help(&r, "Number of programs connected through the remote-control socket (voicemixer ctl).");
                if p.gui_locked {
                    widgets::chip(ui, "LOCKED", true, color::MUTE);
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let shared = &self.ctl.shared;
        let status = shared.status.message.lock().clone();
        let tape = shared.tape.message.lock().clone();
        ui.horizontal(|ui| {
            if let Some(h) = self.ui.last_help {
                ui.label(RichText::new(h).color(color::TEXT).size(11.0));
            } else if !status.is_empty() {
                ui.label(RichText::new(status).color(color::SOLO).size(11.0));
            } else if !tape.is_empty() {
                ui.label(RichText::new(tape).color(color::DIM).size(11.0));
            } else {
                ui.label(RichText::new("Right-click a strip or fader for more options. Double-click a control to reset it.").color(color::DIM).size(11.0));
            }
        });
    }
}
