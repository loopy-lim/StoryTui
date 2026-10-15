//! OpenAI TTS backend — synthesize mp3 via `/audio/speech`, play via `afplay`.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use thiserror::Error;

use crate::TtsConfig;

/// OpenAI default voice when `[tts] voice` is unset.
const DEFAULT_VOICE: &str = "alloy";

#[derive(Debug, Error)]
pub enum TtsError {
    #[error("tts http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("tts protocol error: {0}")]
    Protocol(String),
    #[error("tts io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Synthesizes narration through an OpenAI-compatible TTS endpoint.
#[derive(Clone, Debug)]
pub struct OpenAiTtsBackend {
    config: TtsConfig,
    client: reqwest::Client,
}

impl OpenAiTtsBackend {
    pub fn new(config: TtsConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::builder()
                // A scene of narration synthesizes in a few seconds; cap the
                // wait so a stuck endpoint only delays one background task.
                .timeout(Duration::from_secs(120))
                .build()
                .expect("reqwest client builds with valid config"),
        }
    }

    /// `POST {base_url}/audio/speech` → mp3 bytes.
    pub async fn synthesize(&self, text: &str) -> Result<Vec<u8>, TtsError> {
        let url = format!(
            "{}/audio/speech",
            self.config.base_url.trim_end_matches('/')
        );
        let voice = self
            .config
            .voice
            .clone()
            .unwrap_or_else(|| DEFAULT_VOICE.to_string());
        let mut request = self.client.post(url).json(&serde_json::json!({
            "model": self.config.openai_model,
            "input": text,
            "voice": voice,
            "response_format": "mp3",
        }));
        if let Some(api_key) = &self.config.api_key {
            request = request.bearer_auth(api_key);
        }
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(TtsError::Protocol(format!("HTTP {}", status.as_u16())));
        }
        Ok(response.bytes().await?.to_vec())
    }
}

/// Stage synthesized audio for playback: write it to a temp dir (kept alive by
/// the returned guard) and build the `afplay` command over that file.
pub fn stage_audio(audio: &[u8]) -> Result<(Command, tempfile::TempDir), TtsError> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("narration.mp3");
    std::fs::write(&path, audio)?;
    Ok((afplay_for(&path), dir))
}

/// The `afplay` invocation for a local audio file (macOS built-in).
pub fn afplay_for(path: &Path) -> Command {
    let mut command = Command::new("afplay");
    command.arg(path);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn afplay_command_targets_afplay_with_file_arg() {
        let command = afplay_for(Path::new("/tmp/a.mp3"));
        assert_eq!(command.get_program(), "afplay");
        let args: Vec<_> = command.get_args().collect::<Vec<_>>();
        assert_eq!(args, vec!["/tmp/a.mp3"]);
    }

    #[tokio::test]
    async fn synthesize_posts_json_with_auth_and_returns_bytes() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/audio/speech"))
            .and(wiremock::matchers::header(
                "authorization",
                "Bearer test-key",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_bytes(b"ID3-mp3-bytes".to_vec()),
            )
            .mount(&server)
            .await;

        let backend = OpenAiTtsBackend::new(TtsConfig {
            backend: crate::TtsBackendKind::OpenAi,
            base_url: server.uri(),
            openai_model: "tts-1".to_string(),
            voice: Some("nova".to_string()),
            api_key: Some("test-key".to_string()),
            ..TtsConfig::default()
        });

        let audio = backend.synthesize("안녕하세요").await.expect("ok");
        assert_eq!(audio, b"ID3-mp3-bytes");

        let requests = server.received_requests().await.expect("requests");
        assert_eq!(requests.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("json");
        assert_eq!(body["model"], "tts-1");
        assert_eq!(body["input"], "안녕하세요");
        assert_eq!(body["voice"], "nova");
        assert_eq!(body["response_format"], "mp3");
    }

    #[tokio::test]
    async fn synthesize_surfaces_http_errors_as_protocol() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/audio/speech"))
            .respond_with(wiremock::ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let backend = OpenAiTtsBackend::new(TtsConfig {
            backend: crate::TtsBackendKind::OpenAi,
            base_url: server.uri(),
            ..TtsConfig::default()
        });

        let error = backend.synthesize("x").await.expect_err("401 must fail");
        assert!(matches!(error, TtsError::Protocol(m) if m == "HTTP 401"));
    }

    #[test]
    fn stage_audio_writes_playable_file_with_guard() {
        let (command, guard) = stage_audio(b"fake-mp3").expect("stage");
        assert_eq!(command.get_program(), "afplay");
        let arg = command
            .get_args()
            .next()
            .expect("afplay takes the file path")
            .to_string_lossy()
            .to_string();
        let written = std::fs::read(&arg).expect("staged file exists");
        assert_eq!(written, b"fake-mp3");
        // The guard owns the temp dir; dropping it removes the file.
        let path = arg.clone();
        drop(guard);
        assert!(!Path::new(&path).exists(), "temp audio must be cleaned up");
    }
}
