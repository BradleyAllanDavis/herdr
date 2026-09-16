use std::io;

use tracing::{debug, warn};

use crate::protocol::NotifyKind;

use super::shell;

pub(super) fn handle_shell_notification_effects(
    effects: Vec<shell::ClientShellNotificationEffect>,
    sound_config: &crate::config::SoundConfig,
) {
    for effect in effects {
        match effect {
            shell::ClientShellNotificationEffect::Sound {
                sound,
                agent,
                volume,
            } => {
                let agent = agent.as_deref().and_then(crate::detect::parse_agent_label);
                if sound_config.allows(agent) {
                    crate::sound::play_at_volume(
                        sound,
                        sound_config,
                        resolve_notify_volume(volume, sound_config),
                    );
                }
            }
            shell::ClientShellNotificationEffect::Terminal { title, body } => {
                if let Err(err) = crate::terminal_notify::show_notification(&title, body.as_deref())
                {
                    warn!(err = %err, "failed to emit terminal notification");
                }
            }
            shell::ClientShellNotificationEffect::System { title, body } => {
                if let Err(err) =
                    crate::platform::show_desktop_notification(&title, body.as_deref())
                {
                    warn!(err = %err, "failed to emit system notification");
                }
            }
        }
    }
}

pub(super) fn handle_notify(
    kind: NotifyKind,
    message: &str,
    body: Option<&str>,
    sound_config: &crate::config::SoundConfig,
    volume: Option<f32>,
) {
    handle_notify_with_notifiers(
        kind,
        message,
        body,
        sound_config,
        volume,
        crate::terminal_notify::show_notification,
        crate::platform::show_desktop_notification,
    );
}

pub(super) fn handle_notify_with_notifiers(
    kind: NotifyKind,
    message: &str,
    body: Option<&str>,
    sound_config: &crate::config::SoundConfig,
    volume: Option<f32>,
    mut show_terminal_notification: impl FnMut(&str, Option<&str>) -> io::Result<bool>,
    mut show_system_notification: impl FnMut(&str, Option<&str>) -> io::Result<bool>,
) {
    match kind {
        NotifyKind::Sound => {
            let Some(sound) = sound_from_notify_message(message) else {
                warn!(
                    message = message,
                    "received unknown sound notification from server"
                );
                return;
            };
            if sound_config.enabled {
                crate::sound::play_at_volume(
                    sound,
                    sound_config,
                    resolve_notify_volume(volume, sound_config),
                );
            }
        }
        NotifyKind::Toast => {
            debug!(
                message = message,
                "received terminal toast notification from server"
            );
            if let Err(err) = show_terminal_notification(message, body) {
                warn!(err = %err, "failed to emit terminal notification");
            }
        }
        NotifyKind::SystemToast => {
            debug!(
                message = message,
                "received system toast notification from server"
            );
            if let Err(err) = show_system_notification(message, body) {
                warn!(err = %err, "failed to emit system notification");
            }
        }
    }
}

pub(super) fn sound_from_notify_message(message: &str) -> Option<crate::sound::Sound> {
    match message {
        "agent done" => Some(crate::sound::Sound::Done),
        "agent attention" => Some(crate::sound::Sound::Request),
        _ => None,
    }
}

/// Server-provided volume wins when present (remote mode, server-
/// authoritative); otherwise falls back to this client's own local config
/// (local mode, or a peer that predates the volume field entirely) —
/// matching pre-volume-field behavior exactly when there's nothing to fall
/// back from.
fn resolve_notify_volume(server_volume: Option<f32>, sound_config: &crate::config::SoundConfig) -> f32 {
    server_volume.unwrap_or_else(|| sound_config.effective_volume())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sound_config_with_volume(volume: f32) -> crate::config::SoundConfig {
        crate::config::SoundConfig {
            volume,
            ..crate::config::SoundConfig::default()
        }
    }

    #[test]
    fn notify_volume_prefers_server_value_when_present() {
        let sound_config = sound_config_with_volume(0.9);
        assert_eq!(resolve_notify_volume(Some(0.4), &sound_config), 0.4);
    }

    #[test]
    fn notify_volume_falls_back_to_local_config_when_absent() {
        let sound_config = sound_config_with_volume(0.6);
        assert_eq!(resolve_notify_volume(None, &sound_config), 0.6);
    }
}
