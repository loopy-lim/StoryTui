//! The import report: what mapped, what was ignored, and why.
//!
//! The import policy is "recorded reduction, never silent loss": every field,
//! macro, or lorebook feature the importer does not carry into the pack is
//! listed here, and the untouched original ships in `sources/card.json`.

use std::path::{Path, PathBuf};

/// Hard token ceiling for the free-form description prose injected every turn
/// (CJK-aware conservative estimate; see [`estimate_tokens`]).
pub const MAX_DESCRIPTION_TOKENS: usize = 1_600;

/// Everything the importer wants a human to review after an import.
#[derive(Debug, Default)]
pub struct ImportReport {
    /// Card display name.
    pub card_name: String,
    /// Detected card spec (`chara_card_v2`, `chara_card_v3`, `v1`, ...).
    pub spec_label: String,
    /// Generated pack directory.
    pub pack_dir: PathBuf,
    /// One line per successful field mapping (human-readable).
    pub mapped: Vec<String>,
    /// Distinct unknown card macros that were removed.
    pub ignored_macros: Vec<String>,
    /// (field path, reason) for card content not carried into the pack.
    pub ignored_fields: Vec<(String, String)>,
    /// Lorebook features present but not executed (depth positions, sticky, ...).
    pub approximations: Vec<String>,
    /// Stability-gate warnings (token budget, language mismatch, empty fields).
    pub warnings: Vec<String>,
    /// Estimated tokens of the (possibly truncated) description prose.
    pub description_tokens: usize,
    /// Lorebook entries written to `lorebook.toml`.
    pub lorebook_entries: usize,
    /// `true` when [`crate::scaffold`] already verified the pack with
    /// [`story_persistence::PackLoader`].
    pub loader_verified: bool,
}

impl ImportReport {
    /// An empty report for `card_name`.
    pub fn new(card_name: &str, spec_label: &str) -> Self {
        Self {
            card_name: card_name.to_string(),
            spec_label: spec_label.to_string(),
            ..Self::default()
        }
    }

    /// Record an unknown card macro (deduplicated).
    pub fn note_ignored_macro(&mut self, name: &str) {
        if !self.ignored_macros.iter().any(|seen| seen == name) {
            self.ignored_macros.push(name.to_string());
        }
    }

    /// Record a card field that did not map into the pack.
    pub fn note_ignored_field(&mut self, path: &str, reason: &str) {
        self.ignored_fields
            .push((path.to_string(), reason.to_string()));
    }

    /// Record a lorebook feature that is approximated or deferred.
    pub fn note_approximation(&mut self, detail: &str) {
        if !self.approximations.iter().any(|seen| seen == detail) {
            self.approximations.push(detail.to_string());
        }
    }

    /// Record a stability-gate warning.
    pub fn warn(&mut self, detail: String) {
        if !self.warnings.contains(&detail) {
            self.warnings.push(detail);
        }
    }

    /// Render the Korean Markdown report written next to the pack.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# 임포트 리포트\n\n");
        out.push_str(&format!(
            "- 카드: {} (`{}`)\n",
            self.card_name, self.spec_label
        ));
        out.push_str(&format!("- 팩: `{}`\n", self.pack_dir.display()));
        out.push_str(&format!(
            "- 로어북 엔트리: {}개, 설명 본문 약 {}토큰\n",
            self.lorebook_entries, self.description_tokens
        ));
        out.push_str(&format!(
            "- 팩 로더 검증: {}\n",
            if self.loader_verified {
                "통과"
            } else {
                "미실시"
            }
        ));

        if !self.mapped.is_empty() {
            out.push_str("\n## 매핑\n\n");
            for line in &self.mapped {
                out.push_str(&format!("- {line}\n"));
            }
        }
        if !self.approximations.is_empty() {
            out.push_str("\n## 근사·보류 (보존됨, 미실행)\n\n");
            for line in &self.approximations {
                out.push_str(&format!("- {line}\n"));
            }
        }
        if !self.ignored_fields.is_empty() {
            out.push_str("\n## 무시된 필드\n\n");
            for (path, reason) in &self.ignored_fields {
                out.push_str(&format!("- `{path}` — {reason}\n"));
            }
            out.push_str("\n무시된 내용은 `sources/card.json` 원본에 그대로 남아 있습니다.\n");
        }
        if !self.ignored_macros.is_empty() {
            out.push_str("\n## 제거된 매크로\n\n");
            for name in &self.ignored_macros {
                out.push_str("- `{{");
                out.push_str(name);
                out.push_str("}}`\n");
            }
        }
        if !self.warnings.is_empty() {
            out.push_str("\n## 경고\n\n");
            for line in &self.warnings {
                out.push_str(&format!("- {line}\n"));
            }
        }
        out
    }

    /// Write `markdown()` to `path`.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.markdown())
            .map_err(|e| format!("임포트 리포트를 쓰지 못했습니다 ({}): {e}", path.display()))
    }
}

/// Conservative CJK-aware token estimate: Hangul/CJK ≈ 1 token per character,
/// other scripts ≈ 4 characters per token.
pub(crate) fn estimate_tokens(text: &str) -> usize {
    let mut wide = 0_usize;
    let mut narrow = 0_usize;
    for ch in text.chars() {
        // Hangul syllables/jamo, CJK, kana, fullwidth forms.
        let code = ch as u32;
        if (0x1100..=0x11FF).contains(&code)
            || (0x2E80..=0x9FFF).contains(&code)
            || (0xAC00..=0xD7FF).contains(&code)
            || (0xF900..=0xFAFF).contains(&code)
            || (0xFF00..=0xFF60).contains(&code)
        {
            wide += 1;
        } else {
            narrow += 1;
        }
    }
    wide + narrow.div_ceil(4)
}

/// Ratio of non-ASCII characters, for the language-mismatch warning.
pub(crate) fn non_ascii_ratio(text: &str) -> f64 {
    let total = text.chars().count();
    if total == 0 {
        return 0.0;
    }
    let non_ascii = text.chars().filter(|ch| !ch.is_ascii()).count();
    non_ascii as f64 / total as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_estimate_is_cjk_heavy() {
        assert_eq!(estimate_tokens("비가 그친 뒤"), 6);
        assert!(estimate_tokens("hello world") <= 3);
    }

    #[test]
    fn non_ascii_ratio_detects_english_cards() {
        assert!(non_ascii_ratio("hello world") < 0.1);
        assert!(non_ascii_ratio("비가그친뒤") > 0.9);
        assert!(
            non_ascii_ratio("비가 그친 뒤") > 0.5,
            "spaces may be ASCII but the corpus stays Hangul-dominant"
        );
    }

    #[test]
    fn macro_report_is_deduplicated() {
        let mut report = ImportReport::new("테스트", "v1");
        report.note_ignored_macro("random");
        report.note_ignored_macro("random");
        assert_eq!(report.ignored_macros.len(), 1);
    }

    #[test]
    fn markdown_lists_ignored_fields_and_warnings() {
        let mut report = ImportReport::new("카드", "chara_card_v2");
        report.note_ignored_field("system_prompt", "기본 무시 정책");
        report.warn("설명 본문이 토큰 상한을 초과해 절단했습니다.".to_string());
        let text = report.markdown();
        assert!(text.contains("`system_prompt`"));
        assert!(text.contains("토큰 상한"));
        assert!(text.contains("sources/card.json"));
    }
}
