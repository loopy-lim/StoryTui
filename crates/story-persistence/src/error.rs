//! Pack error taxonomy (W4, ST-012) and Save error taxonomy (W8, ST-027).
//!
//! [`PackError`] carries Korean diagnostics that include file/field/position
//! where applicable. The loader never bails on the first problem: it
//! accumulates a `Vec<PackError>` and returns a single
//! [`PackError::Multiple`] so the author sees every defect at once.
//!
//! [`SaveError`] is the save/load diagnosis taxonomy: it distinguishes
//! missing, truncated, checksum-mismatched, unsupported-version, and
//! invalid-payload cases so the host can surface specific Korean guidance.

use thiserror::Error;

/// All errors a Pack loader can produce.
#[derive(Debug, Error)]
pub enum PackError {
    /// A required Pack file is missing.
    #[error("파일 누락: {path} — {detail}")]
    MissingFile { path: String, detail: String },

    /// A path component tried to escape the Pack root (`..`, absolute path,
    /// symlink escape, etc.).
    #[error("안전하지 않은 경로: {path}")]
    UnsafePath { path: String },

    /// TOML or Markdown structural/parse error, with file + line + column.
    #[error("구문 오류 ({file}:{line}:{col}): {detail}")]
    Parse {
        file: String,
        line: usize,
        col: usize,
        detail: String,
    },

    /// Pack schema major version is unsupported by this loader.
    #[error("지원하지 않는 Pack 스키마: {found} (지원 {supported})")]
    UnsupportedSchemaVersion { found: String, supported: String },

    /// Referential-integrity failure: unknown character/location/flag/item/
    /// event/route id, duplicate id, or an unresolved `ConditionExpr` ref.
    #[error("참조 오류: {detail}")]
    Reference { detail: String },

    /// A required scene/event/location is unreachable from the initial state,
    /// or a transition is impossible given adjacency / event overrides.
    #[error("도달 불가: {detail}")]
    Unreachable { detail: String },

    /// teen Pack policy violation: `rating == "teen"` requires
    /// `all_major_characters_safe == true` AND every major character age
    /// present and `>= 18`.
    #[error("모든 인물 Pack 정책 위반: {detail}")]
    ContentPolicyViolation { detail: String },

    /// Aggregated bundle of every error collected during load. Use
    /// [`PackError::aggregate`] to build a single error from a `Vec`.
    #[error("팩 오류 {count}건:\n{items}")]
    Multiple { count: usize, items: String },

    /// Underlying IO error reading a Pack file.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Underlying TOML deserialize error.
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
}

impl PackError {
    /// Convenience constructor for `MissingFile`.
    pub fn missing_file(path: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::MissingFile {
            path: path.into(),
            detail: detail.into(),
        }
    }

    /// Convenience constructor for `Reference`.
    pub fn reference(detail: impl Into<String>) -> Self {
        Self::Reference {
            detail: detail.into(),
        }
    }

    /// Aggregate a list of errors into one. An empty list yields `None`
    /// (treated as success by the loader); a single-error list collapses to
    /// that error directly; otherwise the errors are formatted as a numbered
    /// Korean list inside [`PackError::Multiple`].
    pub fn aggregate(errs: Vec<PackError>) -> Option<Self> {
        match errs.len() {
            0 => None,
            1 => errs.into_iter().next(),
            _ => {
                let items = errs
                    .iter()
                    .enumerate()
                    .map(|(i, e)| format!("  {}. {e}", i + 1))
                    .collect::<Vec<_>>()
                    .join("\n");
                Some(Self::Multiple {
                    count: errs.len(),
                    items,
                })
            }
        }
    }

    /// Returns `true` if this is an `UnsafePath` error. Useful in tests.
    pub fn is_unsafe_path(&self) -> bool {
        matches!(self, Self::UnsafePath { .. })
    }
}

// ---------------------------------------------------------------------------
// SaveError (W8, ST-027)
// ---------------------------------------------------------------------------

/// Save schema version this build of `story-persistence` writes and reads.
/// The loader compares an envelope's `save_schema_version` against this and
/// routes through [`crate::migration`] (older) or rejects (newer).
pub const SAVE_SCHEMA_VERSION: u32 = 1;

/// All errors a [`crate::json_store::SaveStore`] load can produce. Korean
/// messages are surfaced to the player; the engine/app maps these to
/// `DomainErrorView`.
#[derive(Debug, Error)]
pub enum SaveError {
    /// Underlying IO error reading or writing the save file.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// The requested slot id has no save file. Nothing to read.
    #[error("저장 슬롯이 없습니다: {0}")]
    MissingSlot(String),

    /// The save file exists but is not valid JSON (truncated mid-write,
    /// cat-ed garbage, encoding corruption). The detail carries the parser's
    /// error message.
    #[error("저장 파일이 손상되었습니다 (해석 불가): {0}")]
    TruncatedOrCorrupt(String),

    /// The on-disk payload does not match its recorded checksum. The save is
    /// treated as corrupt; the on-disk file is left untouched.
    #[error("저장 파일 체크섬 불일치: {slot}")]
    ChecksumMismatch {
        /// Slot id whose save failed verification.
        slot: String,
    },

    /// The save's `save_schema_version` is NEWER than this build understands.
    /// The player should update the app. (Older versions route through
    /// [`crate::migration`] instead.)
    #[error("지원하지 않는 저장 스키마: {found} (현재 {supported}) — 앱을 업데이트하세요")]
    UnsupportedVersion {
        /// Schema version found on disk.
        found: u32,
        /// Highest schema version this build supports.
        supported: u32,
    },

    /// The save parsed and verified but failed a domain-invariant check
    /// (non-empty `story_id`, sane revision, etc.).
    #[error("저장 내용이 올바르지 않습니다: {0}")]
    InvalidPayload(String),

    /// Underlying serde error (envelope structure mismatched).
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_none_when_empty() {
        assert!(PackError::aggregate(Vec::new()).is_none());
    }

    #[test]
    fn aggregate_single_collapses() {
        let e = PackError::aggregate(vec![PackError::missing_file("story.toml", "없음")])
            .expect("some");
        assert!(matches!(e, PackError::MissingFile { .. }));
    }

    #[test]
    fn aggregate_multiple_joins_korean_list() {
        let e = PackError::aggregate(vec![
            PackError::missing_file("story.toml", "없음"),
            PackError::reference("캐릭터 c-x 없음"),
        ])
        .expect("some");
        let s = format!("{e}");
        assert!(s.contains("팩 오류 2건"));
        assert!(s.contains("1. 파일 누락: story.toml"));
        assert!(s.contains("2. 참조 오류"));
    }

    #[test]
    fn missing_file_displays_with_path() {
        let s = format!("{}", PackError::missing_file("rules.toml", "xxx"));
        assert!(s.contains("rules.toml"));
    }
}
