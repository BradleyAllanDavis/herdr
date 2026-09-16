//! Small persisted store for the server's live-adjusted notification sound
//! volume (`sound_volume_up`/`sound_volume_down`), so an operator's in-session
//! adjustment survives a full server restart the same way it already survives
//! `herdr server reload-config` (see `app::apply_live_config`'s sticky-volume
//! handling). Mirrors `product_announcements`'s small-JSON-file pattern.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const SOUND_VOLUME_STATE_PATH: &str = "sound-volume.json";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct StoredSoundVolume {
    volume: f32,
}

pub fn store_path() -> PathBuf {
    crate::config::state_dir().join(SOUND_VOLUME_STATE_PATH)
}

/// Load the last-persisted live volume, if any. Returns `None` when nothing
/// has ever been saved (fresh install, or the file was removed) or the file
/// is unreadable/corrupt — callers should fall back to the config's volume.
pub fn load() -> Option<f32> {
    load_from_path(&store_path())
}

fn load_from_path(path: &Path) -> Option<f32> {
    let content = fs::read_to_string(path).ok()?;
    let stored: StoredSoundVolume = serde_json::from_str(&content).ok()?;
    (0.0..=1.0).contains(&stored.volume).then_some(stored.volume)
}

/// Persist the current live volume so it survives a full server restart.
pub fn save(volume: f32) -> io::Result<()> {
    save_to_path(&store_path(), volume)
}

fn save_to_path(path: &Path, volume: f32) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let stored = StoredSoundVolume {
        volume: volume.clamp(0.0, 1.0),
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
    fn round_trips_a_saved_volume() {
        let path = temp_store_path("round-trip");
        save_to_path(&path, 0.42).unwrap();
        assert_eq!(load_from_path(&path), Some(0.42));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn clamps_on_save() {
        let path = temp_store_path("clamp-on-save");
        save_to_path(&path, 1.7).unwrap();
        assert_eq!(load_from_path(&path), Some(1.0));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn missing_file_loads_as_none() {
        let path = temp_store_path("missing");
        assert_eq!(load_from_path(&path), None);
    }

    #[test]
    fn corrupt_file_loads_as_none() {
        let path = temp_store_path("corrupt");
        fs::write(&path, b"not json").unwrap();
        assert_eq!(load_from_path(&path), None);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn out_of_range_stored_value_loads_as_none() {
        let path = temp_store_path("out-of-range");
        fs::write(&path, br#"{"volume":1.5}"#).unwrap();
        assert_eq!(load_from_path(&path), None);
        let _ = fs::remove_file(path);
    }
}
