use std::io;

use tracing::{debug, warn};

use crate::protocol::NotifyKind;

use super::shell;

/// The platform helper blocks on an external program (`terminal-notifier` /
/// `osascript` / `notify-send`), so run it off the input/render thread —
/// same as sound playback — or the whole client freezes while it runs.
/// (Windows already queues its native delivery upstream, see
/// `queue_system_notification`.)
#[cfg(not(windows))]
fn show_untargeted_system_notification(title: &str, body: Option<&str>) -> io::Result<bool> {
    show_desktop_notification_in_background(title.to_owned(), body.map(str::to_owned));
    Ok(true)
}

#[cfg(not(windows))]
fn show_desktop_notification_in_background(title: String, body: Option<String>) {
    let spawned = std::thread::Builder::new()
        .name("system-notify".into())
        .spawn(move || {
            if let Err(err) = crate::platform::show_desktop_notification(&title, body.as_deref()) {
                warn!(err = %err, "failed to emit system notification");
            }
        });
    if let Err(err) = spawned {
        warn!(err = %err, "failed to spawn system notification thread");
    }
}

#[cfg(windows)]
fn queue_system_notification(
    task: impl FnOnce() -> io::Result<bool> + Send + 'static,
) -> io::Result<bool> {
    type Task = Box<dyn FnOnce() -> io::Result<bool> + Send>;
    static QUEUE: std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<Task>> =
        std::sync::OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<Task>();
        // Preserve replacement order while keeping native waits off the client loop.
        tokio::spawn(async move {
            while let Some(task) = receiver.recv().await {
                let result = tokio::task::spawn_blocking(task)
                    .await
                    .unwrap_or_else(|err| Err(io::Error::other(err)));
                if let Err(err) = result {
                    warn!(err = %err, "failed to emit system notification");
                }
            }
        });
        sender
    });
    queue.send(Box::new(task)).map_err(|_| {
        io::Error::new(
            io::ErrorKind::BrokenPipe,
            "notification delivery queue closed",
        )
    })?;
    Ok(true)
}

#[cfg(windows)]
fn show_untargeted_system_notification(title: &str, body: Option<&str>) -> io::Result<bool> {
    let title = title.to_owned();
    let body = body.map(str::to_owned);
    queue_system_notification(move || {
        crate::platform::show_desktop_notification(&title, body.as_deref())
    })
}

pub(super) fn handle_shell_notification_effects(
    effects: Vec<shell::ClientShellNotificationEffect>,
    sound_config: &crate::config::SoundConfig,
    #[cfg(windows)] event_tx: &tokio::sync::mpsc::Sender<super::events::ClientLoopEvent>,
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
            shell::ClientShellNotificationEffect::System {
                title,
                body,
                #[cfg(windows)]
                target,
            } => {
                #[cfg(windows)]
                let result = show_system_notification(&title, body.as_deref(), target, event_tx);
                #[cfg(not(windows))]
                let result = show_untargeted_system_notification(&title, body.as_deref());
                if let Err(err) = result {
                    warn!(err = %err, "failed to emit system notification");
                }
            }
        }
    }
}

#[cfg(windows)]
fn show_system_notification(
    title: &str,
    body: Option<&str>,
    target: Option<shell::ClientSystemNotificationTarget>,
    event_tx: &tokio::sync::mpsc::Sender<super::events::ClientLoopEvent>,
) -> io::Result<bool> {
    let Some(target) = target else {
        return show_untargeted_system_notification(title, body);
    };
    let key = serde_json::to_string(&(
        &target.endpoint_id.storage_key(),
        &target.boot_id,
        &target.pane_id,
    ))
    .map_err(io::Error::other)?;
    let event_tx = event_tx.clone();
    let runtime = tokio::runtime::Handle::current();
    let title = title.to_owned();
    let body = body.map(str::to_owned);
    queue_system_notification(move || {
        crate::platform::show_actionable_desktop_notification(
            &title,
            body.as_deref(),
            key,
            std::sync::Arc::new(move || {
                let event_tx = event_tx.clone();
                let target = target.clone();
                // Never block a native callback: WinRT can dispatch it while Show is awaited.
                runtime.spawn(async move {
                    let _ = event_tx
                        .send(super::events::ClientLoopEvent::NotificationActivated(
                            target,
                        ))
                        .await;
                });
            }),
        )
    })
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
        show_untargeted_system_notification,
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
/// authoritative); otherwise falls back to this client's own local config —
/// e.g. a notification with no sound at all has nothing meaningful to
/// provide here.
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

#[cfg(all(test, windows))]
mod windows_delivery_tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn native_delivery_keeps_client_responsive_and_preserves_replacement_order() {
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        queue_system_notification(move || {
            let _ = started.send(());
            wait.recv_timeout(std::time::Duration::from_secs(5))
                .map_err(io::Error::other)?;
            Ok(true)
        })
        .expect("queued first notification");
        ready.await.expect("notification worker started");
        let (finished, mut second) = tokio::sync::oneshot::channel();
        queue_system_notification(move || {
            let _ = finished.send(());
            Ok(true)
        })
        .expect("queued replacement");
        tokio::task::yield_now().await;
        assert_eq!(
            second.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        );
        // This send must run while native delivery is still waiting.
        release.send(()).expect("client loop remained responsive");
        second.await.expect("replacement delivered after original");
    }
}
