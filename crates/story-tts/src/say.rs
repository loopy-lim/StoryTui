//! macOS `say` backend — free, offline, ships with Korean voices.

use std::process::Command;
use std::sync::OnceLock;

use crate::TtsBackendKind;

/// Marker type so `[tts] backend = "say"` has a named owner, mirroring the
/// OpenAI backend struct.
#[derive(Clone, Debug, Default)]
pub struct SayBackend;

impl SayBackend {
    /// The [`TtsBackendKind`] discriminator for this backend.
    pub fn kind(&self) -> crate::TtsBackendKind {
        TtsBackendKind::Say
    }
}

/// Build the `say` invocation for narration. The text itself is fed through
/// stdin by [`crate::TtsPlayer::speak_stdin`] so narration length never hits
/// argument-size limits.
pub fn say_command(voice: Option<String>) -> Command {
    let mut command = Command::new("say");
    if let Some(voice) = voice {
        command.arg("-v").arg(voice);
    }
    // No input file → `say` reads from stdin.
    command
}

/// Resolve which voice to use: the configured one wins, otherwise pick the
/// first installed Korean voice so Korean narration is not read by an English
/// system voice. `None` falls back to the `say` system default.
///
/// The detection result is cached process-wide: probing `say -v ?` spawns a
/// subprocess, which must not repeat on every narration turn.
pub fn resolve_voice(configured: &Option<String>) -> Option<String> {
    if let Some(voice) = configured {
        return Some(voice.clone());
    }
    static DETECTED: OnceLock<Option<String>> = OnceLock::new();
    DETECTED.get_or_init(detect_korean_voice).clone()
}

/// Probe `say -v ?` for the first `ko_*` voice. Returns `None` when `say` is
/// missing (non-macOS) or no Korean voice is installed.
///
/// Listing lines look like `Yuna           ko_KR    # …` on older macOS and
/// `Eddy (한국어(한국))    ko_KR    # …` on newer ones, so the voice name is
/// everything before the locale column — not the first whitespace token.
fn detect_korean_voice() -> Option<String> {
    let output = Command::new("say").arg("-v").arg("?").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().find_map(|line| {
        let locale = line.split_whitespace().find(|token| is_locale(token))?;
        if !locale.starts_with("ko") {
            return None;
        }
        let name_end = line.find(locale)?;
        let name = line[..name_end].trim_end();
        (!name.is_empty()).then(|| name.to_string())
    })
}

/// Whether a whitespace-delimited token is a `xx_YY` BCP-style locale tag.
fn is_locale(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() == 5
        && bytes[0].is_ascii_lowercase()
        && bytes[1].is_ascii_lowercase()
        && bytes[2] == b'_'
        && bytes[3].is_ascii_uppercase()
        && bytes[4].is_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn say_command_targets_say_without_voice_by_default() {
        let command = say_command(None);
        assert_eq!(command.get_program(), "say");
        assert!(
            command.get_args().next().is_none(),
            "no args when voice is auto-detected"
        );
    }

    #[test]
    fn say_command_passes_voice_flag() {
        let command = say_command(Some("Yuna".to_string()));
        let args: Vec<_> = command.get_args().collect::<Vec<_>>();
        assert_eq!(args, vec!["-v", "Yuna"]);
    }

    #[test]
    fn configured_voice_wins_over_detection() {
        assert_eq!(
            resolve_voice(&Some("Heera".to_string())),
            Some("Heera".to_string())
        );
    }

    #[test]
    fn locale_tags_are_recognized_strictly() {
        assert!(is_locale("ko_KR"));
        assert!(is_locale("en_US"));
        assert!(!is_locale("(한국어(한국))"));
        assert!(!is_locale("ko"));
        assert!(!is_locale("ko_KR_extra"));
        assert!(!is_locale("Ko_KR"));
    }

    #[test]
    fn korean_line_with_localized_name_parses() {
        // Mirrors the newer `say -v ?` output where the display name carries
        // spaces and parentheses before the locale column.
        let lines = "Eddy (한국어(한국))      ko_KR    # 안녕하세요.\nAlice              it_IT    # Ciao.\n";
        let found = lines.lines().find_map(|line| {
            let locale = line.split_whitespace().find(|token| is_locale(token))?;
            if !locale.starts_with("ko") {
                return None;
            }
            let name_end = line.find(locale)?;
            let name = line[..name_end].trim_end();
            (!name.is_empty()).then(|| name.to_string())
        });
        assert_eq!(found.as_deref(), Some("Eddy (한국어(한국))"));
    }
}
