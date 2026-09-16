use serde::{Deserialize, Serialize};

/// `sound.volume.set` params — sets the server's live notification-sound
/// volume directly. `volume` is clamped to `0.0..=1.0` server-side (the same
/// range as `[ui.sound] volume` in config.toml).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SoundVolumeSetParams {
    pub volume: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SoundVolumeStepDirection {
    Up,
    Down,
}

/// `sound.volume.step` params — nudges the server's live notification-sound
/// volume by one step (0.1), clamped to `0.05..=1.0` (a tighter floor than
/// `sound.volume.set`'s `0.0..=1.0`: the live up/down step never reaches
/// fully silent, only an explicit `set 0`/config edit can).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SoundVolumeStepParams {
    pub direction: SoundVolumeStepDirection,
}
