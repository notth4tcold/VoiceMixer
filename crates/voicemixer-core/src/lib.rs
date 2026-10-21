//! VoiceMixer core: parameter model, DSP, mixing engine, control scripts and
//! the VBAN network protocol. Nothing in here depends on an audio API, so it
//! builds and tests on any platform.

// DSP loops index several parallel buffers by frame and channel.
#![allow(clippy::needless_range_loop)]

pub mod banana;
pub mod config;
pub mod controller;
pub mod dsp;
pub mod engine;
pub mod fifo;
pub mod params;
pub mod script;
pub mod shared;
pub mod vban;

pub use controller::Controller;
pub use engine::{Engine, Taps};
pub use params::*;
pub use script::{Action, Value};
pub use shared::{DeviceInfo, Shared};
