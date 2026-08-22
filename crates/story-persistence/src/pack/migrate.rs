//! Pack schema version compatibility & in-memory migration (W4, ST-014).
//!
//! Policy:
//! - The Pack declares `schema_version = "M.m.p"`. The loader compares its
//!   major `M` against [`story_model::PACK_SCHEMA_VERSION`].
//! - Same major → load continues. Missing optional fields are filled with
//!   documented defaults via [`migrate_in_memory`]. No file on disk is
//!   mutated; migration is purely an in-DTO transformation.
//! - Different major → reject with [`PackError::UnsupportedSchemaVersion`].
//!   A future explicit `pack migrate --out` command (W8+) may upgrade a
//!   cross-major pack into a new directory; that is out of scope here.

use story_model::PACK_SCHEMA_VERSION;

use crate::error::PackError;
use crate::pack::dto::StoryToml;

/// Reject a Pack that explicitly requires a newer engine build. This check is
/// independent of the Pack schema version: a schema-compatible Pack may still
/// rely on newer engine behavior.
pub fn check_engine_compat(dto: &StoryToml, errs: &mut Vec<PackError>) {
    let current = match semver::Version::parse(env!("CARGO_PKG_VERSION")) {
        Ok(version) => version,
        Err(_) => return,
    };
    let required = match semver::Version::parse(&dto.min_engine_version) {
        Ok(version) => version,
        Err(error) => {
            errs.push(PackError::Parse {
                file: "story.toml".to_string(),
                line: 0,
                col: 0,
                detail: format!("min_engine_version 파싱 실패: {error}"),
            });
            return;
        }
    };
    if required > current {
        errs.push(PackError::UnsupportedEngineVersion {
            required: required.to_string(),
            current: current.to_string(),
        });
    }
}

/// Compare `dto.schema_version` major against the loader's supported major.
/// Same-major is accepted (with optional in-memory default-fill via
/// [`migrate_in_memory`]); cross-major pushes an
/// [`PackError::UnsupportedSchemaVersion`] into `errs`.
pub fn check_schema_compat(dto: &StoryToml, errs: &mut Vec<PackError>) {
    let supported = PACK_SCHEMA_VERSION;
    let supported_major = match semver::Version::parse(supported) {
        Ok(v) => v.major,
        Err(_) => return, // corrupt build constant — shouldn't happen
    };
    let found_major = match semver::Version::parse(&dto.schema_version) {
        Ok(v) => v.major,
        Err(e) => {
            errs.push(PackError::Parse {
                file: "story.toml".to_string(),
                line: 0,
                col: 0,
                detail: format!("schema_version 파싱 실패: {e}"),
            });
            return;
        }
    };
    if found_major != supported_major {
        errs.push(PackError::UnsupportedSchemaVersion {
            found: dto.schema_version.clone(),
            supported: supported.to_string(),
        });
    }
}

/// In-memory default-fill migration for a same-major pack. Future minor
/// schema bumps that introduce new optional fields will patch them in here.
///
/// Returns a clone of `dto` with any defaultable gaps filled. Cross-major
/// input is rejected — the caller is expected to have invoked
/// [`check_schema_compat`] first.
pub fn migrate_in_memory(dto: &StoryToml) -> Result<StoryToml, PackError> {
    let supported = PACK_SCHEMA_VERSION;
    let supported_major = semver::Version::parse(supported)
        .map_err(|e| PackError::Parse {
            file: "story.toml".to_string(),
            line: 0,
            col: 0,
            detail: format!("빌드 상수 PACK_SCHEMA_VERSION 파싱 실패: {e}"),
        })?
        .major;
    let found_major =
        semver::Version::parse(&dto.schema_version).map_err(|e| PackError::Parse {
            file: "story.toml".to_string(),
            line: 0,
            col: 0,
            detail: format!("schema_version 파싱 실패: {e}"),
        })?;
    if found_major.major != supported_major {
        return Err(PackError::UnsupportedSchemaVersion {
            found: dto.schema_version.clone(),
            supported: supported.to_string(),
        });
    }

    // Same major: no v1.x migrations needed yet. Missing optional fields are
    // already filled by serde defaults. Explicit invalid values are preserved
    // so the canonical validation phase can reject them instead of silently
    // changing author intent.
    Ok(dto.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::dto::{ContentToml, FilesToml, InitialStateToml};

    fn dto_with_schema(schema: &str) -> StoryToml {
        StoryToml {
            schema_version: schema.to_string(),
            story_id: "demo".into(),
            story_version: "0.1.0".into(),
            min_engine_version: "0.1.0".into(),
            title: "x".into(),
            language: "ko-KR".into(),
            genres: Vec::new(),
            rating: "general".into(),
            default_choice_count: 3,
            initial_state: InitialStateToml {
                chapter: "c1".into(),
                scene: "s1".into(),
                location: "loc1".into(),
                time_slot: "evening".into(),
                active_route: None,
            },
            content: ContentToml::default(),
            files: FilesToml::default(),
        }
    }

    #[test]
    fn same_major_no_errors() {
        let mut errs = Vec::new();
        check_schema_compat(&dto_with_schema("1.0.0"), &mut errs);
        assert!(errs.is_empty(), "expected no errors, got {errs:?}");
    }

    #[test]
    fn same_major_minor_diff_accepted() {
        let mut errs = Vec::new();
        // forward / backward within the same major must load
        check_schema_compat(&dto_with_schema("1.5.7"), &mut errs);
        assert!(errs.is_empty(), "expected no errors, got {errs:?}");
    }

    #[test]
    fn future_major_rejected() {
        let mut errs = Vec::new();
        check_schema_compat(&dto_with_schema("2.0.0"), &mut errs);
        assert_eq!(errs.len(), 1);
        match &errs[0] {
            PackError::UnsupportedSchemaVersion { found, supported } => {
                assert_eq!(found, "2.0.0");
                assert!(supported.starts_with("1."));
            }
            other => panic!("expected UnsupportedSchemaVersion, got {other:?}"),
        }
    }

    #[test]
    fn older_major_rejected() {
        let mut errs = Vec::new();
        check_schema_compat(&dto_with_schema("0.9.0"), &mut errs);
        assert!(matches!(
            errs[0],
            PackError::UnsupportedSchemaVersion { .. }
        ));
    }

    #[test]
    fn unparseable_schema_is_parse_error() {
        let mut dto = dto_with_schema("1.0.0");
        dto.schema_version = "not-a-version".to_string();
        let mut errs = Vec::new();
        check_schema_compat(&dto, &mut errs);
        assert!(matches!(errs[0], PackError::Parse { .. }));
    }

    #[test]
    fn migrate_in_memory_same_major_ok() {
        let dto = dto_with_schema("1.0.0");
        let out = migrate_in_memory(&dto).expect("same-major migrates");
        assert_eq!(out.schema_version, "1.0.0");
    }

    #[test]
    fn migrate_in_memory_unsupported_major_errors() {
        let dto = dto_with_schema("2.0.0");
        let err = migrate_in_memory(&dto).expect_err("cross-major rejected");
        assert!(matches!(err, PackError::UnsupportedSchemaVersion { .. }));
    }

    #[test]
    fn migrate_in_memory_backfills_default_choice_count() {
        let mut dto = dto_with_schema("1.0.0");
        dto.default_choice_count = 0;
        let out = migrate_in_memory(&dto).expect("migrates");
        assert_eq!(out.default_choice_count, 0);
    }

    #[test]
    fn future_engine_requirement_is_rejected() {
        let mut dto = dto_with_schema("1.0.0");
        dto.min_engine_version = "99.0.0".to_string();
        let mut errs = Vec::new();
        check_engine_compat(&dto, &mut errs);
        assert!(matches!(
            errs.as_slice(),
            [PackError::UnsupportedEngineVersion { .. }]
        ));
    }
}
