//! Audio backends. On Linux, PipeWire provides the hardware devices and
//! creates the virtual ones. Elsewhere, and with `VOICEMIXER_NULL_BACKEND=1`,
//! a silent timer-driven backend keeps the engine running for GUI work.

mod null;
#[cfg(target_os = "linux")]
mod pipewire;

use std::sync::Arc;
use voicemixer_core::{Engine, Shared};

pub struct BackendHandle {
    reconnect: Box<dyn Fn() + Send + Sync>,
}

impl BackendHandle {
    fn new(reconnect: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            reconnect: Box::new(reconnect),
        }
    }

    /// Re-opens hardware devices after the device selection changed.
    pub fn reconnect(&self) {
        (self.reconnect)()
    }
}

pub fn start(engine: Engine, shared: Arc<Shared>) -> anyhow::Result<BackendHandle> {
    #[cfg(target_os = "linux")]
    if std::env::var_os("VOICEMIXER_NULL_BACKEND").is_none() {
        return pipewire::start(engine, shared);
    }
    null::start(engine, shared)
}
