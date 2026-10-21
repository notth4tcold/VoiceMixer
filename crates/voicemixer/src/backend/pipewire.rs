//! PipeWire backend.
//!
//! Creates, as ordinary PipeWire streams owned by this process (no kernel
//! driver, no root):
//! - two virtual sinks, "VoiceMixer Input" and "VoiceMixer AUX", that
//!   applications play into (virtual strips 4 and 5);
//! - two virtual sources, "VoiceMixer B1" and "VoiceMixer B2", that
//!   applications record from (buses B1 and B2);
//! - one capture stream per hardware strip and one playback stream per A bus,
//!   connected to the devices chosen in the GUI.
//!
//! All callbacks run on the PipeWire main-loop thread (no RT_PROCESS), so
//! the engine lives in a plain `RefCell`.

use super::BackendHandle;
use parking_lot::Mutex;
use pipewire as pw;
use pw::properties::Properties;
use pw::spa::pod::Pod;
use pw::spa::utils::Direction;
use pw::stream::{Stream, StreamFlags, StreamListener};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use voicemixer_core::shared::Devices;
use voicemixer_core::*;

enum Msg {
    Reconnect,
}

#[derive(Clone, Copy)]
enum Port {
    Input(usize),
    Output(usize),
}

type StreamPair = (Stream, StreamListener<()>);

struct State {
    core: pw::core::Core,
    engine: Rc<RefCell<Engine>>,
    shared: Arc<Shared>,
    rate: u32,
    quantum: u32,
    virtual_streams: Vec<StreamPair>,
    hw_streams: Vec<StreamPair>,
}

pub fn start(engine: Engine, shared: Arc<Shared>) -> anyhow::Result<BackendHandle> {
    let (tx, rx) = pw::channel::channel::<Msg>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let thread_shared = shared.clone();
    std::thread::Builder::new()
        .name("pipewire".into())
        .spawn(move || {
            let shared = thread_shared;
            if let Err(e) = run(engine, shared.clone(), rx, &ready_tx) {
                let msg = format!("PipeWire: {e}");
                log::error!("{msg}");
                *shared.status.message.lock() = msg.clone();
                shared.status.running.store(false, Ordering::Relaxed);
                let _ = ready_tx.send(Err(msg));
            }
        })?;
    match ready_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => anyhow::bail!(e),
        Err(_) => anyhow::bail!("PipeWire thread exited during start-up"),
    }
    let tx = Mutex::new(tx);
    Ok(BackendHandle::new(move || {
        let _ = tx.lock().send(Msg::Reconnect);
    }))
}

fn run(
    engine: Engine,
    shared: Arc<Shared>,
    rx: pw::channel::Receiver<Msg>,
    ready: &std::sync::mpsc::Sender<Result<(), String>>,
) -> Result<(), pw::Error> {
    pw::init();
    let mainloop = pw::main_loop::MainLoop::new(None)?;
    let context = pw::context::Context::new(&mainloop)?;
    let core = context.connect(None)?;
    let registry = core.get_registry()?;

    let (rate, quantum) = {
        let p = shared.params.lock();
        (engine.sample_rate(), p.settings.buffer_frames)
    };
    let state = Rc::new(RefCell::new(State {
        core: core.clone(),
        engine: Rc::new(RefCell::new(engine)),
        shared: shared.clone(),
        rate,
        quantum,
        virtual_streams: Vec::new(),
        hw_streams: Vec::new(),
    }));

    // Device list for the GUI.
    let nodes: Rc<RefCell<HashMap<u32, (bool, DeviceInfo)>>> = Rc::default();
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let nodes = nodes.clone();
            let shared = shared.clone();
            move |global| {
                if global.type_ != pw::types::ObjectType::Node {
                    return;
                }
                let Some(props) = global.props else { return };
                let class = props.get("media.class").unwrap_or_default();
                let is_input = match class {
                    "Audio/Source" | "Audio/Source/Virtual" | "Audio/Duplex" => true,
                    "Audio/Sink" => false,
                    _ => return,
                };
                let Some(name) = props.get("node.name") else {
                    return;
                };
                if name.starts_with("voicemixer_") {
                    return;
                }
                let description = props
                    .get("node.description")
                    .or_else(|| props.get("node.nick"))
                    .unwrap_or(name);
                nodes.borrow_mut().insert(
                    global.id,
                    (
                        is_input,
                        DeviceInfo {
                            name: name.into(),
                            description: description.into(),
                        },
                    ),
                );
                publish_devices(&nodes.borrow(), &shared);
            }
        })
        .global_remove({
            let nodes = nodes.clone();
            let shared = shared.clone();
            move |id| {
                if nodes.borrow_mut().remove(&id).is_some() {
                    publish_devices(&nodes.borrow(), &shared);
                }
            }
        })
        .register();

    {
        let mut st = state.borrow_mut();
        let streams = build_virtual_streams(&st)?;
        st.virtual_streams = streams;
        let streams = build_hw_streams(&st);
        st.hw_streams = streams;
    }

    let _receiver = rx.attach(mainloop.loop_(), {
        let state = state.clone();
        move |msg| match msg {
            Msg::Reconnect => {
                let mut st = state.borrow_mut();
                st.hw_streams.clear();
                let streams = build_hw_streams(&st);
                st.hw_streams = streams;
            }
        }
    });

    *shared.status.backend.lock() = "PipeWire".into();
    shared.status.running.store(true, Ordering::Relaxed);
    let _ = ready.send(Ok(()));
    mainloop.run();
    Ok(())
}

fn publish_devices(nodes: &HashMap<u32, (bool, DeviceInfo)>, shared: &Shared) {
    let mut d = Devices::default();
    for (is_input, info) in nodes.values() {
        if *is_input {
            d.inputs.push(info.clone());
        } else {
            d.outputs.push(info.clone());
        }
    }
    d.inputs.sort_by(|a, b| a.description.cmp(&b.description));
    d.outputs.sort_by(|a, b| a.description.cmp(&b.description));
    *shared.devices.lock() = d;
}

fn build_virtual_streams(st: &State) -> Result<Vec<StreamPair>, pw::Error> {
    let s = st.shared.params.lock().settings.clone();
    let in_ch = s.virtual_input_channels as u32;
    let out_ch = s.virtual_output_channels as u32;
    let mut v = Vec::new();
    for (k, (node, desc)) in [
        ("voicemixer_input", "VoiceMixer Input"),
        ("voicemixer_aux", "VoiceMixer AUX"),
    ]
    .iter()
    .enumerate()
    {
        let mut props = base_props(node, desc, in_ch);
        props.insert("media.class", "Audio/Sink");
        v.push(make_stream(
            st,
            node,
            props,
            Port::Input(NUM_HW_STRIPS + k),
            in_ch,
            false,
        )?);
    }
    for (k, (node, desc)) in [
        ("voicemixer_b1", "VoiceMixer B1"),
        ("voicemixer_b2", "VoiceMixer B2"),
    ]
    .iter()
    .enumerate()
    {
        let mut props = base_props(node, desc, out_ch);
        props.insert("media.class", "Audio/Source");
        v.push(make_stream(
            st,
            node,
            props,
            Port::Output(NUM_HW_BUSES + k),
            out_ch,
            false,
        )?);
    }
    Ok(v)
}

/// Opens the hardware devices currently selected. A device that fails to
/// open is reported in the status line; the others keep working.
fn build_hw_streams(st: &State) -> Vec<StreamPair> {
    let p = st.shared.params.lock().clone();
    let hw_out_ch = p.settings.hw_output_channels as u32;
    let mut v = Vec::new();
    let mut errors = Vec::new();
    for i in 0..NUM_HW_STRIPS {
        let Some(dev) = p.strips[i].device.clone().filter(|d| !d.is_empty()) else {
            continue;
        };
        let node = format!("voicemixer_in{}", i + 1);
        let mut props = base_props(&node, &format!("VoiceMixer {}", p.strip_name(i)), 2);
        props.insert("media.category", "Capture");
        props.insert("media.role", "Communication");
        props.insert("target.object", dev.as_str());
        props.insert("node.dont-reconnect", "true");
        match make_stream(st, &node, props, Port::Input(i), 2, true) {
            Ok(s) => v.push(s),
            Err(e) => errors.push(format!("input {}: {e}", i + 1)),
        }
    }
    for b in 0..NUM_HW_BUSES {
        let Some(dev) = p.buses[b].device.clone().filter(|d| !d.is_empty()) else {
            continue;
        };
        let node = format!("voicemixer_{}", BUS_NAMES[b].to_ascii_lowercase());
        let mut props = base_props(&node, &format!("VoiceMixer {}", BUS_NAMES[b]), hw_out_ch);
        props.insert("media.category", "Playback");
        props.insert("media.role", "Music");
        props.insert("target.object", dev.as_str());
        props.insert("node.dont-reconnect", "true");
        match make_stream(st, &node, props, Port::Output(b), hw_out_ch, true) {
            Ok(s) => v.push(s),
            Err(e) => errors.push(format!("{}: {e}", BUS_NAMES[b])),
        }
    }
    *st.shared.status.message.lock() = if errors.is_empty() {
        String::new()
    } else {
        format!("Device error: {}", errors.join("; "))
    };
    v
}

fn base_props(node: &str, description: &str, channels: u32) -> Properties {
    let mut props = Properties::new();
    props.insert("media.type", "Audio");
    props.insert("node.name", node);
    props.insert("node.description", description);
    props.insert("audio.channels", channels.to_string());
    props.insert(
        "audio.position",
        if channels == 2 {
            "FL,FR"
        } else {
            "FL,FR,FC,LFE,RL,RR,SL,SR"
        },
    );
    props
}

fn make_stream(
    st: &State,
    name: &str,
    mut props: Properties,
    port: Port,
    channels: u32,
    autoconnect: bool,
) -> Result<StreamPair, pw::Error> {
    props.insert("node.latency", format!("{}/{}", st.quantum, st.rate));
    props.insert("node.rate", format!("1/{}", st.rate));
    let stream = Stream::new(&st.core, name, props)?;
    let engine = st.engine.clone();
    let ch = channels as usize;
    let quantum = st.quantum as usize;
    let mut scratch: Vec<f32> = Vec::with_capacity(8192 * ch);
    let listener = stream
        .add_local_listener_with_user_data(())
        .process(move |stream, _| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else {
                return;
            };
            match port {
                Port::Input(strip) => {
                    let offset = data.chunk().offset() as usize;
                    let size = data.chunk().size() as usize;
                    if let Some(bytes) = data.data() {
                        let end = (offset + size).min(bytes.len());
                        let bytes = &bytes[offset.min(end)..end];
                        scratch.clear();
                        scratch.extend(
                            bytes
                                .chunks_exact(4)
                                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                        );
                        engine.borrow_mut().push_input(strip, &scratch, ch);
                    }
                }
                Port::Output(bus) => {
                    let stride = 4 * ch;
                    let n_frames = if let Some(bytes) = data.data() {
                        // pipewire-rs 0.8 does not expose pw_buffer.requested. Our
                        // node.latency caps the graph quantum at `quantum`, so one
                        // quantum per callback is always enough.
                        let frames = quantum.min(bytes.len() / stride);
                        scratch.resize(frames * ch, 0.0);
                        engine.borrow_mut().pull_output(bus, &mut scratch, ch);
                        for (dst, v) in bytes.chunks_exact_mut(4).zip(&scratch) {
                            dst.copy_from_slice(&v.to_le_bytes());
                        }
                        frames
                    } else {
                        0
                    };
                    let chunk = data.chunk_mut();
                    *chunk.offset_mut() = 0;
                    *chunk.stride_mut() = stride as _;
                    *chunk.size_mut() = (stride * n_frames) as _;
                }
            }
        })
        .register()?;

    let values = format_pod(st.rate, channels);
    let mut params = [Pod::from_bytes(&values).expect("valid format pod")];
    let direction = match port {
        Port::Input(_) => Direction::Input,
        Port::Output(_) => Direction::Output,
    };
    let mut flags = StreamFlags::MAP_BUFFERS;
    if autoconnect {
        flags |= StreamFlags::AUTOCONNECT;
    }
    stream.connect(direction, None, flags, &mut params)?;
    Ok((stream, listener))
}

fn format_pod(rate: u32, channels: u32) -> Vec<u8> {
    use pw::spa::param::audio::{AudioFormat, AudioInfoRaw, MAX_CHANNELS as SPA_MAX};
    let mut info = AudioInfoRaw::new();
    info.set_format(AudioFormat::F32LE);
    info.set_rate(rate);
    info.set_channels(channels);
    let mut position = [0u32; SPA_MAX];
    let order = [
        libspa_sys::SPA_AUDIO_CHANNEL_FL,
        libspa_sys::SPA_AUDIO_CHANNEL_FR,
        libspa_sys::SPA_AUDIO_CHANNEL_FC,
        libspa_sys::SPA_AUDIO_CHANNEL_LFE,
        libspa_sys::SPA_AUDIO_CHANNEL_RL,
        libspa_sys::SPA_AUDIO_CHANNEL_RR,
        libspa_sys::SPA_AUDIO_CHANNEL_SL,
        libspa_sys::SPA_AUDIO_CHANNEL_SR,
    ];
    for (dst, src) in position
        .iter_mut()
        .zip(order.iter())
        .take(channels as usize)
    {
        *dst = *src;
    }
    info.set_position(position);
    pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(pw::spa::pod::Object {
            type_: libspa_sys::SPA_TYPE_OBJECT_Format,
            id: libspa_sys::SPA_PARAM_EnumFormat,
            properties: info.into(),
        }),
    )
    .expect("serialising an audio format cannot fail")
    .0
    .into_inner()
}
