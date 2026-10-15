//! Text-to-speech narration for StoryTUI (독립 크레이트).
//!
//! 두 백엔드를 제공한다:
//! - [`say::SayBackend`] — macOS 내장 `say`(무료·오프라인). 텍스트는 stdin으로
//!   전달해 인자 길이 제한을 피한다.
//! - [`openai::OpenAiTtsBackend`] — OpenAI TTS API(`/audio/speech`, mp3)를
//!   내려받아 `afplay`로 재생.
//!
//! [`TtsService`]는 UI 루프를 막지 않는 것이 계약이다. `speak`는 항상 즉시
//! 반환하고, 실제 합성·재생은 백그라운드에서 진행된다. 새 요청이 오면 이전
//! 재생 자식 프로세스를 kill하고 뒤늦게 끝난 오래된 합성 요청은 폐기한다
//! (sequence 번호로 판정). 어떤 오류도 앱을 죽이지 않는다 — 로그만 남긴다.

pub mod openai;
pub mod say;
pub mod text;

use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

/// Which synthesis backend narrates the story.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TtsBackendKind {
    /// macOS built-in `say` (default; free, offline).
    #[default]
    Say,
    /// OpenAI TTS API (`/audio/speech`) played back through `afplay`.
    OpenAi,
}

impl TtsBackendKind {
    /// Lowercase config-style name (also used in the Settings screen).
    pub fn as_str(&self) -> &'static str {
        match self {
            TtsBackendKind::Say => "say",
            TtsBackendKind::OpenAi => "openai",
        }
    }
}

/// Read-only snapshot for the Settings screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TtsStatus {
    pub enabled: bool,
    pub backend: TtsBackendKind,
    pub voice: Option<String>,
}

/// `[tts]` configuration block. Every field has a default so an absent block
/// means "say backend, disabled".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TtsConfig {
    /// Master switch. The `v` toggle overrides this per session.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub backend: TtsBackendKind,
    /// Backend voice. `None` uses the backend default; the say backend picks
    /// the first installed Korean voice when this is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// OpenAI TTS model id.
    #[serde(default = "default_openai_model")]
    pub openai_model: String,
    /// OpenAI-compatible TTS base URL (ends in `/v1`).
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// Environment variable holding the Bearer token for the OpenAI backend.
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
    /// Optional file holding the Bearer token, mirroring `[provider]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_file: Option<String>,
    /// Resolved secret (env or file). Never serialized — same discipline as
    /// `ProviderConfig::api_key`.
    #[serde(skip)]
    pub api_key: Option<String>,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: TtsBackendKind::Say,
            voice: None,
            openai_model: default_openai_model(),
            base_url: default_base_url(),
            api_key_env: default_api_key_env(),
            api_key_file: None,
            api_key: None,
        }
    }
}

fn default_openai_model() -> String {
    "gpt-4o-mini-tts".to_string()
}
fn default_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}
fn default_api_key_env() -> String {
    "OPENAI_API_KEY".to_string()
}

/// One playing (or pending) child process, tagged with its speech sequence.
/// `_temp` keeps an OpenAI mp3 alive until playback ends or is superseded.
struct Playing {
    seq: u64,
    child: Option<Child>,
    _temp: Option<tempfile::TempDir>,
}

/// Owns the current playback child. Killing is synchronous and cheap; a killed
/// `say`/`afplay` stops within milliseconds.
#[derive(Default)]
pub struct TtsPlayer {
    current: Option<Playing>,
}

impl TtsPlayer {
    /// Stop the current playback (if any) and start `command`, tagged `seq`.
    /// Spawn failures are logged, never fatal.
    pub fn start(&mut self, command: Command, seq: u64) {
        self.start_guarded(command, seq, None);
    }

    /// [`Self::start`] plus a guard (e.g. the mp3 temp directory) kept alive
    /// until this playback is superseded or stopped.
    pub fn start_guarded(&mut self, command: Command, seq: u64, guard: Option<tempfile::TempDir>) {
        self.stop();
        match spawn(command) {
            Ok(child) => {
                self.current = Some(Playing {
                    seq,
                    child: Some(child),
                    _temp: guard,
                });
            }
            Err(error) => {
                tracing::warn!(%error, seq, "TTS playback failed to start");
            }
        }
    }

    /// Stop the current playback and mark `seq` as the newest request while
    /// its synthesis is still in flight (no child yet).
    pub fn arm(&mut self, seq: u64) {
        self.stop();
        self.current = Some(Playing {
            seq,
            child: None,
            _temp: None,
        });
    }

    /// Attach stdin-fed speech (`say`): spawn `command` with a piped stdin,
    /// write `text`, and close the pipe so the child starts speaking.
    pub fn speak_stdin(&mut self, mut command: Command, text: &str, seq: u64) {
        self.stop();
        let mut command = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match &mut command {
            Ok(child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = std::io::Write::write_all(&mut stdin, text.as_bytes());
                } // dropped → pipe closed → `say` speaks
                let child = command.expect("matched Ok above");
                self.current = Some(Playing {
                    seq,
                    child: Some(child),
                    _temp: None,
                });
            }
            Err(error) => {
                tracing::warn!(%error, seq, "TTS speech failed to start");
            }
        }
    }

    /// Kill the current playback, if any.
    pub fn stop(&mut self) {
        if let Some(mut playing) = self.current.take()
            && let Some(mut child) = playing.child.take()
        {
            let _ = child.kill();
            let _ = child.wait();
            // `_temp` drops with `playing`: the killed child no longer needs
            // its file.
        }
    }

    /// Whether `seq` is still the newest speech request.
    pub fn is_current(&self, seq: u64) -> bool {
        self.current
            .as_ref()
            .is_some_and(|playing| playing.seq == seq)
    }
}

impl Drop for TtsPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Spawn with all output muted: a TTS child must never write to the TUI's
/// terminal or the redirected log stream. (stdin is left as configured.)
fn spawn(mut command: Command) -> std::io::Result<Child> {
    command.stdout(Stdio::null()).stderr(Stdio::null());
    command.spawn()
}

/// Shared narration entry point. Cloneable so the host can hand handles to
/// background tasks; all state is behind one mutex.
#[derive(Clone)]
pub struct TtsService {
    inner: Arc<Mutex<ServiceState>>,
}

struct ServiceState {
    config: TtsConfig,
    player: TtsPlayer,
    next_seq: u64,
}

impl TtsService {
    /// Build a service for `config`. No IO happens here.
    pub fn new(config: TtsConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ServiceState {
                config,
                player: TtsPlayer::default(),
                next_seq: 1,
            })),
        }
    }

    /// Current enabled flag (mirrors the `v` toggle).
    pub async fn enabled(&self) -> bool {
        self.inner.lock().await.config.enabled
    }

    /// Read-only status snapshot for display.
    pub async fn status(&self) -> TtsStatus {
        let state = self.inner.lock().await;
        TtsStatus {
            enabled: state.config.enabled,
            backend: state.config.backend,
            voice: state.config.voice.clone(),
        }
    }

    /// Flip the master switch. Disabling also stops current playback.
    pub async fn set_enabled(&self, enabled: bool) {
        let mut state = self.inner.lock().await;
        state.config.enabled = enabled;
        if !enabled {
            state.player.stop();
        }
    }

    /// Narrate `text` if enabled. Returns immediately — synthesis and playback
    /// happen in the background. A disabled service is a no-op.
    pub async fn speak(&self, text: String) {
        let (config, seq) = {
            let mut state = self.inner.lock().await;
            if !state.config.enabled {
                return;
            }
            let seq = state.next_seq;
            state.next_seq += 1;
            state.player.arm(seq);
            (state.config.clone(), seq)
        };
        let speakable = text::speakable_text(&text);
        if speakable.trim().is_empty() {
            return;
        }
        match config.backend {
            TtsBackendKind::Say => {
                let command = say::say_command(say::resolve_voice(&config.voice));
                let mut state = self.inner.lock().await;
                if state.player.is_current(seq) {
                    state.player.speak_stdin(command, &speakable, seq);
                }
            }
            TtsBackendKind::OpenAi => {
                let backend = openai::OpenAiTtsBackend::new(config.clone());
                let service = self.clone();
                tokio::spawn(async move {
                    match backend.synthesize(&speakable).await {
                        Ok(audio) => match openai::stage_audio(&audio) {
                            Ok((command, guard)) => {
                                let mut state = service.inner.lock().await;
                                if state.player.is_current(seq) {
                                    state.player.start_guarded(command, seq, Some(guard));
                                }
                            }
                            Err(error) => {
                                tracing::warn!(%error, seq, "TTS audio staging failed");
                            }
                        },
                        Err(error) => {
                            tracing::warn!(%error, seq, "OpenAI TTS synthesis failed");
                        }
                    }
                });
            }
        }
    }

    /// Stop current playback; the still-speaking text is discarded.
    pub async fn stop(&self) {
        self.inner.lock().await.player.stop();
    }

    /// Update the config (e.g. after a backend/voice change) and stop playback.
    pub async fn replace_config(&self, config: TtsConfig) {
        let mut state = self.inner.lock().await;
        state.player.stop();
        state.config = config;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tts_config_defaults_are_say_disabled() {
        let config = TtsConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.backend, TtsBackendKind::Say);
        assert_eq!(config.base_url, "https://api.openai.com/v1");
        assert_eq!(config.openai_model, "gpt-4o-mini-tts");
        assert_eq!(config.api_key_env, "OPENAI_API_KEY");
    }

    #[test]
    fn tts_config_round_trips_through_toml() {
        let toml_str = r#"
enabled = true
backend = "openai"
voice = "alloy"
openai_model = "tts-1"
base_url = "https://example.com/v1"
api_key_env = "MY_KEY"
"#;
        let config: TtsConfig = toml::from_str(toml_str).expect("parse");
        assert_eq!(config.backend, TtsBackendKind::OpenAi);
        assert_eq!(config.voice.as_deref(), Some("alloy"));
        assert_eq!(config.openai_model, "tts-1");

        let text = toml::to_string_pretty(&config).expect("serialize");
        assert!(
            !text.contains("api_key ="),
            "the resolved secret field must never serialize: {text}"
        );
        // `api_key_env` is a variable NAME, not a secret; only the resolved
        // `api_key` value must stay off disk.
        assert!(text.contains("api_key_env"));
    }

    #[test]
    fn partial_tts_table_fills_defaults() {
        let config: TtsConfig = toml::from_str("enabled = true").expect("parse");
        assert!(config.enabled);
        assert_eq!(config.backend, TtsBackendKind::Say);
        assert_eq!(config.base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn player_arm_then_start_keeps_sequence() {
        let mut player = TtsPlayer::default();
        player.arm(7);
        assert!(player.is_current(7));
        assert!(!player.is_current(6));

        player.start(std::process::Command::new("true"), 7);
        assert!(player.is_current(7));
    }

    #[test]
    fn player_start_supersedes_previous_child() {
        let mut player = TtsPlayer::default();
        player.start(std::process::Command::new("sleep"), 1);
        player.start(std::process::Command::new("true"), 2);
        assert!(player.is_current(2));
        assert!(!player.is_current(1), "previous child must be killed");
    }

    #[test]
    fn player_stop_clears_currency() {
        let mut player = TtsPlayer::default();
        player.start(std::process::Command::new("true"), 1);
        player.stop();
        assert!(!player.is_current(1));
    }

    #[test]
    fn speak_stdin_feeds_text_and_keeps_child() {
        let mut player = TtsPlayer::default();
        // `/bin/cat` echoes stdin back (to /dev/null here) and exits on EOF —
        // the same lifecycle `say` has.
        player.speak_stdin(std::process::Command::new("cat"), "안녕", 3);
        assert!(player.is_current(3));
        player.stop();
        assert!(!player.is_current(3));
    }

    #[test]
    fn speak_stdin_failure_is_not_fatal() {
        let mut player = TtsPlayer::default();
        let mut command = std::process::Command::new("cat");
        command.current_dir("/definitely/not/a/dir");
        player.speak_stdin(command, "x", 1);
        // A failed spawn leaves no playback; the service's arm() owns currency
        // semantics, so a failed child simply means "nothing is playing".
        assert!(!player.is_current(1));
    }
}
