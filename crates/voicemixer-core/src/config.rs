//! Loading and saving settings as TOML.

use crate::params::MixerParams;
use anyhow::Context;
use std::path::{Path, PathBuf};

pub fn config_dir() -> PathBuf {
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("VoiceMixer");
        }
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(xdg).join("voicemixer");
    }
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(".config").join("voicemixer"),
        None => PathBuf::from(".voicemixer"),
    }
}

pub fn default_path() -> PathBuf {
    config_dir().join("settings.toml")
}

pub fn presets_dir() -> PathBuf {
    config_dir().join("presets")
}

/// Default folder for recordings: `~/Music/VoiceMixer`.
pub fn recordings_dir() -> PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    match home {
        Some(h) => PathBuf::from(h).join("Music").join("VoiceMixer"),
        None => PathBuf::from("recordings"),
    }
}

pub fn load(path: &Path) -> anyhow::Result<MixerParams> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut p: MixerParams =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    p.normalize();
    Ok(p)
}

/// Loads the file, or returns defaults when it does not exist. A corrupt
/// file is kept aside as `*.broken` so the user's settings are not lost.
pub fn load_or_default(path: &Path) -> MixerParams {
    if !path.exists() {
        return MixerParams::default();
    }
    match load(path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("voicemixer: {e:#}; starting with default settings");
            let _ = std::fs::rename(path, path.with_extension("toml.broken"));
            MixerParams::default()
        }
    }
}

/// Writes atomically (temporary file + rename).
pub fn save(path: &Path, p: &MixerParams) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let text = toml::to_string_pretty(p)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("voicemixer-test-{}", std::process::id()));
        let path = dir.join("settings.toml");
        let mut p = MixerParams::default();
        p.strips[2].label = "Guitar".into();
        p.buses[1].eq[4][2].gain_db = -3.5;
        save(&path, &p).unwrap();
        assert_eq!(load(&path).unwrap(), p);
        std::fs::remove_dir_all(dir).ok();
    }
}
