//! Sound notifications for agent state changes.
//!
//! Embeds mp3 files in the binary and plays them via system audio tools.
//! Uses afplay (macOS), Windows MediaPlayer, or decoder-capable Linux audio
//! players — no Rust audio dependencies.

use std::io::Write;
#[cfg(not(any(windows, target_os = "macos")))]
use std::io::{Read, Result as IoResult};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(any(windows, target_os = "macos")))]
use std::time::{Duration, Instant};

use tracing::warn;

const DISABLE_SOUND_ENV: &str = "HERDR_DISABLE_SOUND";
#[cfg(any(windows, test))]
const WINDOWS_SOUND_PATH_ENV: &str = "HERDR_SOUND_PATH";
#[cfg(any(windows, test))]
const WINDOWS_SOUND_VOLUME_ENV: &str = "HERDR_SOUND_VOLUME";
#[cfg(not(any(windows, target_os = "macos")))]
const AUDIO_PLAYER_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(not(any(windows, target_os = "macos")))]
const AUDIO_PLAYER_POLL_INTERVAL: Duration = Duration::from_millis(25);

static SOUND_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static SOUND_DONE: &[u8] = include_bytes!("../assets/sounds/done.mp3");
static SOUND_REQUEST: &[u8] = include_bytes!("../assets/sounds/request.mp3");

/// Which notification sound to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    /// Agent finished work (transitioned to Idle).
    Done,
    /// Agent needs input (transitioned to Blocked).
    Request,
}

/// Play a notification sound in a background thread, at the volume from
/// `config` (i.e. `config.effective_volume()`).
/// Silently does nothing if no audio player is available.
pub fn play(sound: Sound, config: &crate::config::SoundConfig) {
    play_at_volume(sound, config, config.effective_volume());
}

/// Play a notification sound in a background thread, at an explicit volume
/// (0.0..=1.0, clamped defensively) that overrides `config.volume` — used
/// when a peer (typically the server, in `--remote` mode) provided its own
/// volume alongside the notification. Falls back to `config`'s own volume
/// when the caller has nothing better (see call sites in `client::notifications`).
pub fn play_at_volume(sound: Sound, config: &crate::config::SoundConfig, volume: f32) {
    if sound_playback_disabled_by_env() {
        return;
    }

    let volume = volume.clamp(0.0, 1.0);
    let custom_path = config.path_for(sound);
    std::thread::spawn(move || {
        if let Some(path) = custom_path {
            match play_file(&path, volume) {
                Ok(()) => return,
                Err(err) => {
                    warn!(path = %path.display(), sound = ?sound, err = %err, "custom sound playback failed, falling back to built-in sound")
                }
            }
        }

        let data = match sound {
            Sound::Done => SOUND_DONE,
            Sound::Request => SOUND_REQUEST,
        };

        if let Err(err) = play_bytes(data, volume) {
            warn!(sound = ?sound, err = %err, "sound playback failed");
        }
    });
}

fn sound_playback_disabled_by_env() -> bool {
    std::env::var_os(DISABLE_SOUND_ENV).is_some() || std::env::var_os("NEXTEST").is_some()
}

fn play_file(path: &Path, volume: f32) -> Result<(), String> {
    match run_player(path, volume) {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(playback_error(&output)),
        Err(err) => Err(err),
    }
}

fn play_bytes(data: &[u8], volume: f32) -> Result<(), String> {
    // Write to a temp file because the supported audio players need a file path.
    let tmp = temp_sound_path();
    let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    file.write_all(data).map_err(|e| e.to_string())?;
    drop(file);

    let result = run_player(&tmp, volume);

    let _ = std::fs::remove_file(&tmp);

    match result {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(playback_error(&output)),
        Err(e) => Err(e),
    }
}

fn playback_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        format!("player exited with {}", output.status)
    } else {
        format!("player exited with {}: {stderr}", output.status)
    }
}

fn temp_sound_path() -> PathBuf {
    let id = SOUND_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("herdr-sound-{}-{id}.mp3", std::process::id()))
}

#[cfg(windows)]
fn run_player(path: &Path, volume: f32) -> Result<Output, String> {
    run_windows_player(path, volume)
}

#[cfg(any(target_os = "macos", test))]
fn afplay_command(path: &Path, volume: f32) -> Command {
    // afplay's -v takes a linear 0.0-1.0 float directly, matching herdr's own
    // volume range — no rescaling needed.
    let mut command = Command::new("afplay");
    command.arg("-v").arg(format!("{volume:.3}")).arg(path);
    command
}

#[cfg(target_os = "macos")]
fn run_player(path: &Path, volume: f32) -> Result<Output, String> {
    afplay_command(path, volume)
        .output()
        .map_err(|e| format!("no audio player available: {e}"))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn run_player(path: &Path, volume: f32) -> Result<Output, String> {
    run_linux_player(path, volume)
}

#[cfg(any(windows, test))]
fn windows_media_player_script() -> &'static str {
    r#"
$ErrorActionPreference = 'Stop'
$Path = [Environment]::GetEnvironmentVariable('HERDR_SOUND_PATH', 'Process')
if ([string]::IsNullOrWhiteSpace($Path)) { throw 'HERDR_SOUND_PATH is not set' }
$VolumeRaw = [Environment]::GetEnvironmentVariable('HERDR_SOUND_VOLUME', 'Process')
$Volume = 1.0
if (-not [string]::IsNullOrWhiteSpace($VolumeRaw)) { $Volume = [double]$VolumeRaw }
Add-Type -AssemblyName PresentationCore
Add-Type -AssemblyName WindowsBase
$resolved = (Resolve-Path -LiteralPath $Path).ProviderPath
$script:player = [System.Windows.Media.MediaPlayer]::new()
$script:player.Volume = $Volume
$script:frame = [System.Windows.Threading.DispatcherFrame]::new()
$script:timer = [System.Windows.Threading.DispatcherTimer]::new()
$script:timer.Interval = [TimeSpan]::FromSeconds(15)
$script:failed = $null
$script:timedOut = $false
$script:player.add_MediaOpened({ $script:player.Play() })
$script:player.add_MediaEnded({ $script:frame.Continue = $false })
$script:player.add_MediaFailed({
    param($sender, $eventArgs)
    $script:failed = $eventArgs.ErrorException
    $script:frame.Continue = $false
})
$script:timer.add_Tick({
    $script:timedOut = $true
    $script:frame.Continue = $false
})
try {
    $script:player.Open([Uri]::new($resolved))
    $script:timer.Start()
    [System.Windows.Threading.Dispatcher]::PushFrame($script:frame)
} finally {
    $script:timer.Stop()
    $script:player.Close()
}
if ($script:failed) { throw "sound media failed: $($script:failed.Message)" }
if ($script:timedOut) { throw 'sound playback timed out' }
"#
}

#[cfg(any(windows, test))]
fn windows_player_command(path: &Path, volume: f32) -> Command {
    let mut command = crate::noninteractive_process::command("powershell.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            windows_media_player_script(),
        ])
        .env(WINDOWS_SOUND_PATH_ENV, path)
        .env(WINDOWS_SOUND_VOLUME_ENV, format!("{volume:.3}"));
    command
}

#[cfg(windows)]
fn run_windows_player(path: &Path, volume: f32) -> Result<Output, String> {
    windows_player_command(path, volume)
        .output()
        .map_err(|e| format!("Windows MediaPlayer playback failed: {e}"))
}

#[cfg(not(any(windows, target_os = "macos")))]
#[derive(Debug, Clone)]
struct AudioPlayer {
    program: &'static str,
    // Owned, not `&'static [&'static str]`, because the volume-derived flag
    // is a runtime-formatted string per invocation (see `linux_audio_players`).
    args: Vec<String>,
}

#[cfg(not(any(windows, target_os = "macos")))]
impl AudioPlayer {
    fn output(&self, path: &Path) -> std::io::Result<Output> {
        self.output_with_timeout(path, AUDIO_PLAYER_TIMEOUT)
    }

    fn output_with_timeout(&self, path: &Path, timeout: Duration) -> std::io::Result<Output> {
        let mut child = Command::new(self.program)
            .args(&self.args)
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        let Some(stdout) = child.stdout.take() else {
            terminate_and_reap(&mut child)?;
            return Err(std::io::Error::other("audio player stdout was not piped"));
        };
        let Some(stderr) = child.stderr.take() else {
            terminate_and_reap(&mut child)?;
            return Err(std::io::Error::other("audio player stderr was not piped"));
        };
        let stdout_reader = read_output(stdout);
        let stderr_reader = read_output(stderr);
        let deadline = Instant::now() + timeout;

        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let (stdout, stderr) = finish_output(stdout_reader, stderr_reader)?;
                    return Ok(Output {
                        status,
                        stdout,
                        stderr,
                    });
                }
                Ok(None) => {}
                Err(wait_err) => {
                    let cleanup_result = terminate_and_reap(&mut child);
                    let _ = finish_output(stdout_reader, stderr_reader);
                    cleanup_result?;
                    return Err(wait_err);
                }
            }

            let now = Instant::now();
            if now >= deadline {
                let cleanup_result = terminate_and_reap(&mut child);
                let _ = finish_output(stdout_reader, stderr_reader);
                cleanup_result?;
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("{} playback timed out after {timeout:?}", self.program),
                ));
            }

            std::thread::sleep((deadline - now).min(AUDIO_PLAYER_POLL_INTERVAL));
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn read_output<R>(mut reader: R) -> std::thread::JoinHandle<IoResult<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut output = Vec::new();
        reader.read_to_end(&mut output)?;
        Ok(output)
    })
}

#[cfg(not(any(windows, target_os = "macos")))]
fn finish_output(
    stdout_reader: std::thread::JoinHandle<IoResult<Vec<u8>>>,
    stderr_reader: std::thread::JoinHandle<IoResult<Vec<u8>>>,
) -> IoResult<(Vec<u8>, Vec<u8>)> {
    let stdout = stdout_reader
        .join()
        .map_err(|_| std::io::Error::other("audio player stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| std::io::Error::other("audio player stderr reader panicked"))??;
    Ok((stdout, stderr))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn terminate_and_reap(child: &mut std::process::Child) -> std::io::Result<()> {
    if let Err(kill_err) = child.kill() {
        if child.try_wait()?.is_none() {
            return Err(kill_err);
        }
    }
    child.wait().map(|_| ())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn linux_audio_players(volume: f32) -> Vec<AudioPlayer> {
    // Do not add bare aplay here. It does not decode MP3 and plays MP3 bytes as raw PCM.
    vec![
        AudioPlayer {
            program: "paplay",
            // PulseAudio's volume unit is linear 0-65536.
            args: vec![format!("--volume={}", (volume * 65536.0).round() as u32)],
        },
        AudioPlayer {
            program: "pw-play",
            // PipeWire's pw-play takes a linear 0.0-1.0 float directly.
            args: vec!["--volume".to_string(), format!("{volume:.3}")],
        },
        AudioPlayer {
            program: "ffplay",
            // ffplay's -volume flag is an integer 0-100.
            args: vec![
                "-nodisp".to_string(),
                "-autoexit".to_string(),
                "-loglevel".to_string(),
                "quiet".to_string(),
                "-volume".to_string(),
                format!("{}", (volume * 100.0).round() as i64),
            ],
        },
        AudioPlayer {
            program: "mpg123",
            // mpg123's -f/--scale takes a linear integer scale factor, 32768 = unity gain.
            args: vec![
                "-q".to_string(),
                "-f".to_string(),
                format!("{}", (volume * 32768.0).round() as i64),
            ],
        },
        AudioPlayer {
            program: "mpv",
            // mpv's --volume is 0-100, 100 = unity.
            args: vec![
                "--no-video".to_string(),
                "--really-quiet".to_string(),
                format!("--volume={}", (volume * 100.0).round() as i64),
            ],
        },
    ]
}

#[cfg(not(any(windows, target_os = "macos")))]
fn run_linux_player(path: &Path, volume: f32) -> Result<Output, String> {
    let mut errors = Vec::new();

    for player in &linux_audio_players(volume) {
        match player.output(path) {
            Ok(output) if output.status.success() => return Ok(output),
            Ok(output) => errors.push(player_error(player, &output)),
            Err(err) => errors.push(format!("{} failed: {err}", player.program)),
        }
    }

    Err(format!(
        "no mp3-capable audio player available: {}",
        errors.join("; ")
    ))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn player_error(player: &AudioPlayer, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();

    if stderr.is_empty() {
        format!("{} exited with {}", player.program, output.status)
    } else {
        format!("{} exited with {}: {stderr}", player.program, output.status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_sound_paths_are_unique() {
        assert_ne!(temp_sound_path(), temp_sound_path());
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn linux_audio_players_are_mp3_capable() {
        let programs: Vec<&str> = linux_audio_players(1.0)
            .iter()
            .map(|player| player.program)
            .collect();

        assert_eq!(programs, ["paplay", "pw-play", "ffplay", "mpg123", "mpv"]);
        assert!(!programs.contains(&"aplay"));
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn linux_audio_players_carry_volume_args() {
        let players = linux_audio_players(0.5);
        let args = |program: &str| -> Vec<String> {
            players
                .iter()
                .find(|player| player.program == program)
                .unwrap()
                .args
                .clone()
        };

        // PulseAudio: 0-65536 linear.
        assert_eq!(args("paplay"), vec!["--volume=32768".to_string()]);
        // PipeWire: 0.0-1.0 linear float.
        assert_eq!(
            args("pw-play"),
            vec!["--volume".to_string(), "0.500".to_string()]
        );
        // ffplay: 0-100 integer.
        assert_eq!(
            args("ffplay"),
            vec![
                "-nodisp".to_string(),
                "-autoexit".to_string(),
                "-loglevel".to_string(),
                "quiet".to_string(),
                "-volume".to_string(),
                "50".to_string(),
            ]
        );
        // mpg123: 0-32768 integer scale, 32768 = unity.
        assert_eq!(
            args("mpg123"),
            vec!["-q".to_string(), "-f".to_string(), "16384".to_string()]
        );
        // mpv: 0-100, 100 = unity.
        assert_eq!(
            args("mpv"),
            vec![
                "--no-video".to_string(),
                "--really-quiet".to_string(),
                "--volume=50".to_string(),
            ]
        );
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn linux_audio_players_clamp_volume_extremes_cleanly() {
        let silent = linux_audio_players(0.0);
        assert_eq!(
            silent
                .iter()
                .find(|p| p.program == "paplay")
                .unwrap()
                .args,
            vec!["--volume=0".to_string()]
        );
        let full = linux_audio_players(1.0);
        assert_eq!(
            full.iter().find(|p| p.program == "paplay").unwrap().args,
            vec!["--volume=65536".to_string()]
        );
        assert_eq!(
            full.iter().find(|p| p.program == "mpg123").unwrap().args,
            vec!["-q".to_string(), "-f".to_string(), "32768".to_string()]
        );
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn linux_audio_player_does_not_wait_forever() {
        let pid_path = temp_sound_path().with_extension("pid");
        let player = AudioPlayer {
            program: "sh",
            args: vec![
                "-c".to_string(),
                "printf '%s' \"$$\" > \"$1\"; exec sleep 2".to_string(),
                "herdr-sound-timeout-test".to_string(),
            ],
        };
        let result = player.output_with_timeout(&pid_path, Duration::from_millis(100));
        let pid = std::fs::read_to_string(&pid_path)
            .expect("hanging test player should record its process ID");
        let _ = std::fs::remove_file(pid_path);

        let err = result.expect_err("hanging audio player should time out");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        let status = Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .expect("test should inspect the timed-out player PID");
        assert!(
            !status.success(),
            "timed-out audio player should be terminated and reaped"
        );
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn linux_audio_player_preserves_completed_output() {
        let player = AudioPlayer {
            program: "sh",
            args: vec![
                "-c".to_string(),
                "i=0; while [ \"$i\" -lt 8192 ]; do printf 0123456789abcdef; i=$((i + 1)); done; i=0; while [ \"$i\" -lt 8192 ]; do printf fedcba9876543210; i=$((i + 1)); done >&2; exit 7".to_string(),
                "herdr-sound-output-test".to_string(),
            ],
        };

        let output = player
            .output_with_timeout(Path::new("unused.mp3"), Duration::from_secs(5))
            .expect("completed audio player should return its output");

        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout.len(), 131_072);
        assert_eq!(output.stderr.len(), 131_072);
        assert!(output.stdout.starts_with(b"0123456789abcdef"));
        assert!(output.stderr.starts_with(b"fedcba9876543210"));
    }

    #[test]
    fn afplay_command_passes_volume_flag_directly() {
        let path = Path::new("/tmp/done.mp3");
        let command = afplay_command(path, 0.35);
        let args: Vec<&std::ffi::OsStr> = command.get_args().collect();
        assert_eq!(
            args,
            vec![
                std::ffi::OsStr::new("-v"),
                std::ffi::OsStr::new("0.350"),
                path.as_os_str(),
            ]
        );
    }

    #[test]
    fn windows_media_player_uses_process_environment_and_dispatcher() {
        let script = windows_media_player_script();
        let path = Path::new(r"C:\sound dir\döne.mp3");
        let command = windows_player_command(path, 0.35);
        let env_path = command.get_envs().find_map(|(key, value)| {
            (key == std::ffi::OsStr::new(WINDOWS_SOUND_PATH_ENV))
                .then_some(value)
                .flatten()
        });
        let env_volume = command.get_envs().find_map(|(key, value)| {
            (key == std::ffi::OsStr::new(WINDOWS_SOUND_VOLUME_ENV))
                .then_some(value)
                .flatten()
        });

        assert!(script.contains("GetEnvironmentVariable('HERDR_SOUND_PATH', 'Process')"));
        assert!(script.contains("GetEnvironmentVariable('HERDR_SOUND_VOLUME', 'Process')"));
        assert!(script.contains("$script:player.Volume = $Volume"));
        assert!(!script.contains("param([string]$Path)"));
        assert!(script.contains("Resolve-Path -LiteralPath $Path"));
        assert!(script.contains("Dispatcher]::PushFrame"));
        assert!(script.contains("add_MediaEnded"));
        assert!(script.contains("add_MediaFailed"));
        assert_eq!(env_path, Some(path.as_os_str()));
        assert_eq!(env_volume, Some(std::ffi::OsStr::new("0.350")));
        assert!(!command.get_args().any(|arg| arg == path.as_os_str()));
    }

    #[cfg(windows)]
    #[test]
    fn windows_media_player_reports_invalid_media_without_waiting_for_timeout() {
        let _lock = crate::integration::integration_env_lock();
        let path = temp_sound_path();
        std::fs::write(&path, b"not an mp3").unwrap();
        let output = run_windows_player(&path, 1.0).unwrap();
        let _ = std::fs::remove_file(path);

        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("sound media failed"),
            "stderr should identify a MediaFailed error"
        );
    }
}
