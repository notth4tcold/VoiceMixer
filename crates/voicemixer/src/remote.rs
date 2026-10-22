//! Remote control over a local socket, one command per line:
//!
//! ```text
//! Strip[0].Gain = -6; Bus[3].Mute = 1     -> OK
//! get Strip[0].Gain                        -> OK -6
//! levels                                   -> OK <strip and bus peaks in dB>
//! show                                     -> OK (opens the window from the tray)
//! ```
//!
//! Linux: Unix socket `$XDG_RUNTIME_DIR/voicemixer.sock`. Other systems:
//! TCP `127.0.0.1:47650`. `voicemixer ctl ...` is the matching client.

use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::Ordering;
use voicemixer_core::{dsp, Controller, NUM_BUSES, NUM_STRIPS};

use crate::tray::{Request, WindowControl};

#[cfg(unix)]
pub fn socket_path() -> std::path::PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join("voicemixer.sock")
}

#[cfg(not(unix))]
const TCP_ADDR: &str = "127.0.0.1:47650";

pub fn start(ctl: Controller, window: Option<WindowControl>) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixListener;
        let path = socket_path();
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            anyhow::bail!("another VoiceMixer is already running ({})", path.display());
        }
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        std::thread::Builder::new()
            .name("remote".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let ctl = ctl.clone();
                    let window = window.clone();
                    std::thread::spawn(move || {
                        if let Ok(reader) = stream.try_clone() {
                            serve(&ctl, window.as_ref(), BufReader::new(reader), stream);
                        }
                    });
                }
            })?;
    }
    #[cfg(not(unix))]
    {
        let listener = std::net::TcpListener::bind(TCP_ADDR)?;
        std::thread::Builder::new()
            .name("remote".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let ctl = ctl.clone();
                    let window = window.clone();
                    std::thread::spawn(move || {
                        if let Ok(reader) = stream.try_clone() {
                            serve(&ctl, window.as_ref(), BufReader::new(reader), stream);
                        }
                    });
                }
            })?;
    }
    Ok(())
}

fn serve(
    ctl: &Controller,
    window: Option<&WindowControl>,
    reader: impl BufRead,
    mut writer: impl Write,
) {
    let clients = &ctl.shared.status.remote_clients;
    clients.fetch_add(1, Ordering::Relaxed);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let reply = answer(ctl, window, line.trim());
        if writeln!(writer, "{reply}")
            .and_then(|_| writer.flush())
            .is_err()
        {
            break;
        }
    }
    clients.fetch_sub(1, Ordering::Relaxed);
}

fn answer(ctl: &Controller, window: Option<&WindowControl>, line: &str) -> String {
    if line.is_empty() || line.eq_ignore_ascii_case("ping") {
        return "OK pong".into();
    }
    if line.eq_ignore_ascii_case("show") {
        return match window {
            Some(w) => {
                w.request(Request::Show);
                "OK".into()
            }
            None => "ERR running headless, there is no window to show".into(),
        };
    }
    if let Some(path) = line
        .strip_prefix("get ")
        .or_else(|| line.strip_prefix("GET "))
    {
        return match ctl.get(path) {
            Ok(v) => format!("OK {}", v.as_text()),
            Err(e) => format!("ERR {e}"),
        };
    }
    if line.eq_ignore_ascii_case("levels") {
        // Reads the decaying display peaks (see `shared::Meters::tick`, run
        // by a background thread regardless of whether a GUI is attached),
        // so this falls back towards silence over time in `--headless` mode
        // too, instead of latching at the loudest peak ever seen.
        let m = &ctl.shared.meters;
        let mut out = Vec::new();
        for s in 0..NUM_STRIPS {
            out.push(format!("{:.1}", dsp::lin_to_db(m.strip_level(s))));
        }
        for b in 0..NUM_BUSES {
            out.push(format!("{:.1}", dsp::lin_to_db(m.bus_level(b))));
        }
        return format!("OK {}", out.join(" "));
    }
    match ctl.run_script(line) {
        Ok(()) => "OK".into(),
        Err(e) => format!("ERR {e}"),
    }
}

/// Asks an already running VoiceMixer to show its window. False if none is
/// running (or it is headless).
pub fn show_running() -> bool {
    matches!(send("show"), Ok(reply) if reply.starts_with("OK"))
}

/// `voicemixer ctl <script>` / `voicemixer ctl get <param>`.
pub fn client(args: &[String]) -> anyhow::Result<()> {
    if args.is_empty() {
        anyhow::bail!(
            "usage: voicemixer ctl 'Strip[0].Gain = -6'  |  voicemixer ctl get Strip[0].Gain"
        );
    }
    let reply = send(&args.join(" "))?;
    match reply.strip_prefix("ERR ") {
        Some(e) => anyhow::bail!("{e}"),
        None => {
            let value = reply.strip_prefix("OK").unwrap_or(&reply).trim();
            if !value.is_empty() && value != "pong" {
                println!("{value}");
            }
            Ok(())
        }
    }
}

/// Sends one command line to the running instance and returns its reply.
fn send(line: &str) -> anyhow::Result<String> {
    #[cfg(unix)]
    let stream = std::os::unix::net::UnixStream::connect(socket_path()).map_err(|e| {
        anyhow::anyhow!(
            "VoiceMixer is not running ({}: {e})",
            socket_path().display()
        )
    })?;
    #[cfg(not(unix))]
    let stream = std::net::TcpStream::connect(TCP_ADDR)
        .map_err(|e| anyhow::anyhow!("VoiceMixer is not running ({TCP_ADDR}: {e})"))?;
    let mut writer = stream.try_clone()?;
    writeln!(writer, "{line}")?;
    writer.flush()?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    Ok(reply.trim_end().to_string())
}
