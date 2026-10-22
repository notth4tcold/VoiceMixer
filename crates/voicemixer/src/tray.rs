//! System tray icon, and the show/quit requests that drive the window.
//!
//! The audio engine never depended on the window, so "minimise to tray" just
//! means closing the window and opening a fresh one on request: no hidden
//! window lingering around (Wayland cannot hide one anyway), and no GPU
//! resources held while VoiceMixer sits in the tray. Requests come from the
//! tray menu, from `voicemixer ctl show`, and from starting `voicemixer`
//! again while it is already running.

use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Show,
    Quit,
}

#[derive(Clone)]
pub struct WindowControl(Arc<Inner>);

struct Inner {
    tx: Sender<Request>,
    rx: Receiver<Request>,
    /// The open window's context, to wake it up when a request arrives.
    ctx: Mutex<Option<egui::Context>>,
    /// A tray icon is currently shown, so closing the window can hide it
    /// there instead of quitting.
    tray: AtomicBool,
    quitting: AtomicBool,
}

impl WindowControl {
    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Self(Arc::new(Inner {
            tx,
            rx,
            ctx: Mutex::new(None),
            tray: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
        }))
    }

    pub fn request(&self, r: Request) {
        if r == Request::Quit {
            self.0.quitting.store(true, Ordering::Relaxed);
        }
        let _ = self.0.tx.send(r);
        if let Some(ctx) = &*self.0.ctx.lock() {
            ctx.request_repaint();
        }
    }

    /// Requests made while the window is open.
    pub fn try_recv(&self) -> Option<Request> {
        self.0.rx.try_recv().ok()
    }

    /// Blocks until something asks for the window back (or to quit).
    pub fn wait(&self) -> Request {
        if self.quitting() {
            return Request::Quit;
        }
        self.0.rx.recv().unwrap_or(Request::Quit)
    }

    pub fn attach(&self, ctx: Option<egui::Context>) {
        *self.0.ctx.lock() = ctx;
    }

    pub fn in_tray(&self) -> bool {
        self.0.tray.load(Ordering::Relaxed)
    }

    pub fn quitting(&self) -> bool {
        self.0.quitting.load(Ordering::Relaxed)
    }
}

/// Shows the tray icon. Returns false if the desktop has no tray (no
/// StatusNotifierWatcher, e.g. GNOME without the AppIndicator extension).
#[cfg(target_os = "linux")]
pub fn start(win: &WindowControl) -> bool {
    use ksni::blocking::TrayMethods;
    match (Tray { win: win.clone() }).spawn() {
        // The service keeps running on its own thread; the handle is only
        // needed to change the icon later, which we never do.
        Ok(_handle) => {
            win.0.tray.store(true, Ordering::Relaxed);
            true
        }
        Err(e) => {
            log::warn!("no system tray ({e}); closing the window will quit");
            false
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub fn start(_win: &WindowControl) -> bool {
    false
}

#[cfg(target_os = "linux")]
struct Tray {
    win: WindowControl,
}

#[cfg(target_os = "linux")]
impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "voicemixer".into()
    }

    fn title(&self) -> String {
        "VoiceMixer".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        // A pixmap rather than an icon name, so it also shows when running
        // from the tarball with nothing installed in the icon theme.
        let icon = crate::app_icon();
        let mut data = icon.rgba;
        for px in data.chunks_exact_mut(4) {
            px.rotate_right(1); // RGBA -> ARGB
        }
        vec![ksni::Icon {
            width: icon.width as i32,
            height: icon.height as i32,
            data,
        }]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "VoiceMixer".into(),
            description: "Virtual mixing console (running)".into(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.win.request(Request::Show);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Show VoiceMixer".into(),
                icon_name: "window-new".into(),
                activate: Box::new(|t: &mut Self| t.win.request(Request::Show)),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| t.win.request(Request::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn watcher_online(&self) {
        self.win.0.tray.store(true, Ordering::Relaxed);
    }

    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        // The panel went away (crashed, restarted, session change): until it
        // comes back, closing the window must quit, or VoiceMixer would be
        // left running with no way to reach it.
        self.win.0.tray.store(false, Ordering::Relaxed);
        true
    }
}
