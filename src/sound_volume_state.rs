//! Small persisted store for the server's live-adjusted notification sound
//! volume (`sound_volume_up`/`sound_volume_down`), so an operator's in-session
//! adjustment survives a full server restart the same way it already survives
//! `herdr server reload-config` (see `app::apply_live_config`'s sticky-volume
//! handling). Mirrors `product_announcements`'s small-JSON-file pattern.
//!
//! Also stores `config_baseline` — the config's own `ui.sound.volume` value
//! as of the last save — so `load()` can apply the exact same "did the
//! operator actually edit the config file" comparison at startup that
//! `apply_live_config` already applies on every `reload-config`. Without
//! this, an operator who edits `[ui.sound] volume` in config.toml while the
//! server is stopped would find that edit silently overridden by whatever
//! live value was persisted before the restart, forever, until they either
//! trigger a live reload or clear this file — a real footgun caught in
//! review (round 1 of this branch only carried the bare volume).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const SOUND_VOLUME_STATE_PATH: &str = "sound-volume.json";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct StoredSoundVolume {
    volume: f32,
    /// config.toml's `ui.sound.volume` as of this save. Startup only trusts
    /// `volume` when the config's *current* value still matches this — same
    /// comparison `apply_live_config` does on reload, applied once at boot.
    config_baseline: f32,
}

pub fn store_path() -> PathBuf {
    crate::config::state_dir().join(SOUND_VOLUME_STATE_PATH)
}

/// Load the last-persisted live volume, if the config's *current* volume
/// still matches what it was when that value was saved (i.e. the operator
/// hasn't edited `[ui.sound] volume` in config.toml since) — same rule
/// `apply_live_config` applies on `reload-config`, applied once at startup.
/// Returns `None` when nothing has ever been saved, the file is unreadable/
/// corrupt, or the config was edited since — callers fall back to the
/// config's volume in every `None` case.
pub fn load(current_config_volume: f32) -> Option<f32> {
    load_from_path(&store_path(), current_config_volume)
}

fn load_from_path(path: &Path, current_config_volume: f32) -> Option<f32> {
    let content = fs::read_to_string(path).ok()?;
    let stored: StoredSoundVolume = serde_json::from_str(&content).ok()?;
    if !(0.0..=1.0).contains(&stored.volume) {
        return None;
    }
    (stored.config_baseline == current_config_volume).then_some(stored.volume)
}

/// Persist the current live volume, tagged with the config volume it was
/// live-adjusted from, so it survives a full server restart.
pub fn save(volume: f32, config_baseline: f32) -> io::Result<()> {
    save_to_path(&store_path(), volume, config_baseline)
}

fn save_to_path(path: &Path, volume: f32, config_baseline: f32) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let stored = StoredSoundVolume {
        volume: volume.clamp(0.0, 1.0),
        config_baseline,
    };
    let json = serde_json::to_string_pretty(&stored).map_err(io::Error::other)?;
    let tmp_path = path.with_extension(format!("json.tmp.{}", std::process::id()));
    fs::write(&tmp_path, json)?;
    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "herdr-sound-volume-state-test-{name}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn round_trips_a_saved_volume_when_config_is_unchanged() {
        let path = temp_store_path("round-trip");
        save_to_path(&path, 0.42, 1.0).unwrap();
        assert_eq!(load_from_path(&path, 1.0), Some(0.42));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn ignores_a_saved_volume_when_config_baseline_changed() {
        let path = temp_store_path("baseline-changed");
        // Live-adjusted to 0.42 while config said 1.0 (the default).
        save_to_path(&path, 0.42, 1.0).unwrap();
        // Operator edited config.toml to 0.6 while the server was stopped —
        // the config's own new value should win at startup, not the stale
        // persisted live adjustment.
        assert_eq!(load_from_path(&path, 0.6), None);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn clamps_on_save() {
        let path = temp_store_path("clamp-on-save");
        save_to_path(&path, 1.7, 1.0).unwrap();
        assert_eq!(load_from_path(&path, 1.0), Some(1.0));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn missing_file_loads_as_none() {
        let path = temp_store_path("missing");
        assert_eq!(load_from_path(&path, 1.0), None);
    }

    #[test]
    fn corrupt_file_loads_as_none() {
        let path = temp_store_path("corrupt");
        fs::write(&path, b"not json").unwrap();
        assert_eq!(load_from_path(&path, 1.0), None);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn out_of_range_stored_value_loads_as_none() {
        let path = temp_store_path("out-of-range");
        fs::write(&path, br#"{"volume":1.5,"config_baseline":1.0}"#).unwrap();
        assert_eq!(load_from_path(&path, 1.0), None);
        let _ = fs::remove_file(path);
    }
}
