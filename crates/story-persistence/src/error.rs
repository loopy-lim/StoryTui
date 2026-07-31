//! Pack error taxonomy (W4, ST-012).
//!
//! [`PackError`] carries Korean diagnostics that include file/field/position
//! where applicable. The loader never bails on the first problem: it
//! accumulates a `Vec<PackError>` and returns a single
//! [`PackError::Multiple`] so the author sees every defect at once.

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
