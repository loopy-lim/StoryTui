//! Story Pack loader (W4, ST-013).
//!
//! [`PackLoader::load`] reads a Pack directory, parses each TOML/Markdown
//! file into the DTOs in [`super::dto`], runs safety checks (file existence,
//! UTF-8, per-file size limit, path safety), and assembles a canonical
//! [`story_model::StoryPack`]. Schema-major compatibility and full
//! referential and reachability validation live in
//! [`super::migrate`] and [`super::validate`] respectively; the loader calls
//! them in the right order.
//!
//! The loader executes **no** arbitrary code (ADR-007) — only TOML +
//! Markdown parsing. The Pack files on disk are never mutated by `load`.

use std::path::{Path, PathBuf};

use story_model::{CharacterDef, PromptFragments, StoryPack};

use crate::error::PackError;
use crate::pack::dto::{
    CharactersToml, EventsToml, RulesToml, StoryToml, character_dto_to_canonical,
    merge_events_into_rules, rules_dto_to_canonical, story_to_metadata,
};

/// Maximum accepted size of any single Pack file (1 MiB).
pub const MAX_FILE_BYTES: usize = 1024 * 1024;

/// Required root-relative Pack files (in load order).
pub const REQUIRED_FILES: &[&str] = &[
    "story.toml",
    "characters.toml",
    "rules.toml",
    "events.toml",
    "world.md",
    "style.md",
    "opening.md",
    "prompts/system.md",
    "prompts/director.md",
    "prompts/writer.md",
];

/// Entry point for loading a Story Pack from disk.
pub struct PackLoader;

impl PackLoader {
    /// Load and validate the Pack at `root`, returning the canonical
    /// [`StoryPack`] or an aggregated [`PackError`] listing every problem.
    pub fn load(root: &Path) -> Result<StoryPack, PackError> {
        let mut ctx = LoadCtx::new(root)?;
        ctx.run()?;
        ctx.finish()
    }
}

/// Per-load scratch space that collects errors as it goes. Held private so
/// the public API stays a single function.
struct LoadCtx<'a> {
    root: &'a Path,
    /// Canonicalized absolute root, used both for diagnostics and as the
    /// containment boundary for every Pack file read.
    canon_root: PathBuf,
    /// Resolved file paths (after `[files]` overrides + safety checks).
    paths: FilePaths,
    story_dto: Option<StoryToml>,
    /// Successfully assembled pack, set by `run` when conversion succeeds.
    assembled: Option<StoryPack>,
    errors: Vec<PackError>,
}

/// Resolved per-file paths. Defaults are the canonical names; an optional
/// `[files]` override from `story.toml` replaces them after safety check.
#[derive(Clone, Debug, Default)]
struct FilePaths {
    characters: Option<PathBuf>,
    rules: Option<PathBuf>,
    events: Option<PathBuf>,
    world: Option<PathBuf>,
    style: Option<PathBuf>,
    opening: Option<PathBuf>,
    system_prompt: Option<PathBuf>,
    director_prompt: Option<PathBuf>,
    writer_prompt: Option<PathBuf>,
}

impl<'a> LoadCtx<'a> {
    fn new(root: &'a Path) -> Result<Self, PackError> {
        // Root must be a directory.
        if !root.is_dir() {
            return Err(PackError::MissingFile {
                path: root.display().to_string(),
                detail: "Pack 루트 디렉터리가 없습니다".to_string(),
            });
        }
        // A non-canonical root cannot safely anchor child containment checks,
        // so fail closed instead of falling back to a potentially relative
        // path.
        let canon_root = root.canonicalize().map_err(PackError::Io)?;
        Ok(Self {
            root,
            canon_root,
            paths: FilePaths::default(),
            story_dto: None,
            assembled: None,
            errors: Vec::new(),
        })
    }

    /// Drive the full load pipeline. Each phase appends to `self.errors`
    /// and continues where possible so the author sees every defect.
    fn run(&mut self) -> Result<(), PackError> {
        // Phase 1: load story.toml (the source of truth for [files] overrides).
        self.load_story_toml()?;

        // Phase 2: resolve per-file paths via [files] overrides + safety.
        self.resolve_paths();

        // Phase 3: read every required file (existence + UTF-8 + size).
        // Each missing file is recorded and skipped; we still try the rest.
        let story_dto = match &self.story_dto {
            Some(s) => s.clone(),
            None => return Ok(()), // story.toml failed earlier; nothing else to do
        };

        // Snapshot the resolved per-file paths so we don't hold an immutable
        // borrow of `self` while the read helpers mutate `self.errors`.
        let resolved: [(&str, Option<PathBuf>); 9] = [
            ("characters.toml", self.paths.characters.clone()),
            ("rules.toml", self.paths.rules.clone()),
            ("events.toml", self.paths.events.clone()),
            ("world.md", self.paths.world.clone()),
            ("style.md", self.paths.style.clone()),
            ("opening.md", self.paths.opening.clone()),
            ("prompts/system.md", self.paths.system_prompt.clone()),
            ("prompts/director.md", self.paths.director_prompt.clone()),
            ("prompts/writer.md", self.paths.writer_prompt.clone()),
        ];
        let mut read = Vec::with_capacity(resolved.len());
        for (label, p) in resolved {
            read.push(self.read_required(label, p.as_deref()));
        }
        let characters_str = read.remove(0);
        let rules_str = read.remove(0);
        let events_str = read.remove(0);
        let world_str = read.remove(0);
        let style_str = read.remove(0);
        let opening_str = read.remove(0);
        let system_str = read.remove(0);
        let director_str = read.remove(0);
        let writer_str = read.remove(0);

        // Phase 4: schema-major compat (migrate.rs).
        // Cross-major is rejected via UnsupportedSchemaVersion; same-major
        // continues with in-memory default-fill.
        crate::pack::migrate::check_schema_compat(&story_dto, &mut self.errors);
        crate::pack::migrate::check_engine_compat(&story_dto, &mut self.errors);
        let story_dto = match crate::pack::migrate::migrate_in_memory(&story_dto) {
            Ok(d) => d,
            Err(e) => {
                self.errors.push(e);
                return Ok(());
            }
        };

        // Phase 5: parse TOML DTOs.
        let characters_dto = self.parse_toml::<CharactersToml>("characters.toml", characters_str);
        let rules_dto = self.parse_toml::<RulesToml>("rules.toml", rules_str);
        let events_dto = self.parse_toml::<EventsToml>("events.toml", events_str);

        // Phase 6: convert DTOs to canonical types. Conversion failures
        // (bad enum values, semver parse errors) are recorded but don't
        // abort the rest of the pipeline — we still want to surface
        // referential errors from whatever did parse.
        let metadata = match story_to_metadata(&story_dto) {
            Ok(m) => m,
            Err(detail) => {
                self.errors.push(PackError::Parse {
                    file: "story.toml".to_string(),
                    line: 0,
                    col: 0,
                    detail,
                });
                return Ok(());
            }
        };

        let mut characters: std::collections::BTreeMap<story_model::CharacterId, CharacterDef> =
            Default::default();
        if let Some(dto) = characters_dto {
            for c in &dto.characters {
                match character_dto_to_canonical(c) {
                    Ok(def) => {
                        if characters.contains_key(&def.id) {
                            self.errors.push(PackError::reference(format!(
                                "중복 캐릭터 id: {} (characters.toml)",
                                def.id
                            )));
                        } else {
                            characters.insert(def.id.clone(), def);
                        }
                    }
                    Err(detail) => self.errors.push(PackError::Parse {
                        file: "characters.toml".to_string(),
                        line: 0,
                        col: 0,
                        detail,
                    }),
                }
            }
        }

        let mut rules = match rules_dto.as_ref().map(rules_dto_to_canonical) {
            Some(Ok(r)) => r,
            Some(Err(detail)) => {
                self.errors.push(PackError::Parse {
                    file: "rules.toml".to_string(),
                    line: 0,
                    col: 0,
                    detail,
                });
                story_model::StoryRules::minimal()
            }
            None => story_model::StoryRules::minimal(),
        };
        if let Some(dto) = events_dto.as_ref()
            && let Err(detail) = merge_events_into_rules(dto, &mut rules)
        {
            self.errors.push(PackError::Parse {
                file: "events.toml".to_string(),
                line: 0,
                col: 0,
                detail,
            });
        }

        let pack = StoryPack {
            metadata,
            characters,
            rules,
            world_md: world_str.unwrap_or_default(),
            style_md: style_str.unwrap_or_default(),
            opening_md: opening_str.unwrap_or_default(),
            prompts: PromptFragments {
                system: system_str.unwrap_or_default(),
                director: director_str.unwrap_or_default(),
                writer: writer_str.unwrap_or_default(),
            },
            pack_root: self.canon_root.clone(),
        };

        // Phase 7: referential integrity + reachability.
        crate::pack::validate::validate_all(&pack, &mut self.errors);

        // Phase 8: hand the assembled pack to the caller via finish().
        // We can't move `pack` through `Ok(())`; stash it on a side channel.
        self.assembled = Some(pack);
        Ok(())
    }

    /// Final step: hand back the assembled pack or aggregate every error.
    fn finish(self) -> Result<StoryPack, PackError> {
        if let Some(err) = PackError::aggregate(self.errors) {
            return Err(err);
        }
        self.assembled.ok_or_else(|| PackError::MissingFile {
            path: "<unknown>".to_string(),
            detail: "Pack 로드가 완료되지 않았습니다".to_string(),
        })
    }

    // -----------------------------------------------------------------------
    // phases
    // -----------------------------------------------------------------------

    fn load_story_toml(&mut self) -> Result<(), PackError> {
        let path = self.root.join("story.toml");
        let text = self.read_text("story.toml", &path);
        let Some(text) = text else {
            // missing story.toml is fatal — we can't resolve other files;
            // the MissingFile error was already pushed by read_text.
            return Ok(());
        };
        match toml::from_str::<StoryToml>(&text) {
            Ok(dto) => self.story_dto = Some(dto),
            Err(e) => self.errors.push(parse_error("story.toml", &e, &text)),
        }
        Ok(())
    }

    /// Use [files] overrides from story.toml after path-safety check.
    /// Each path is stored root-relative.
    fn resolve_paths(&mut self) {
        let Some(dto) = &self.story_dto else {
            return;
        };
        let f = &dto.files;
        let mut set = |slot: &mut Option<PathBuf>, raw: &Option<String>, default: &str| match raw {
            Some(p) => match validate_safe_relative_path(p) {
                Some(rel) => *slot = Some(PathBuf::from(rel)),
                None => self.errors.push(PackError::UnsafePath { path: p.clone() }),
            },
            None => *slot = Some(PathBuf::from(default)),
        };
        set(&mut self.paths.characters, &f.characters, "characters.toml");
        set(&mut self.paths.rules, &f.rules, "rules.toml");
        set(&mut self.paths.events, &f.events, "events.toml");
        set(&mut self.paths.world, &f.world, "world.md");
        set(&mut self.paths.style, &f.style, "style.md");
        set(&mut self.paths.opening, &f.opening, "opening.md");
        set(
            &mut self.paths.system_prompt,
            &f.system_prompt,
            "prompts/system.md",
        );
        set(
            &mut self.paths.director_prompt,
            &f.director_prompt,
            "prompts/director.md",
        );
        set(
            &mut self.paths.writer_prompt,
            &f.writer_prompt,
            "prompts/writer.md",
        );
    }

    /// Read a required file from `path` (root-relative). On missing/oversize/
    /// non-UTF8, push a single error and return `None`.
    fn read_required(&mut self, label: &str, path: Option<&Path>) -> Option<String> {
        let rel = path.unwrap_or_else(|| Path::new(label));
        let full = self.root.join(rel);
        self.read_text(label, &full)
    }

    /// Read `path` as UTF-8 text. Maps low-level problems into `self.errors`
    /// and returns `None` when no usable string is available.
    fn read_text(&mut self, label: &str, path: &Path) -> Option<String> {
        // Resolve symlinks before reading and then read the canonical target.
        // This prevents an in-Pack path from escaping the Pack root through a
        // file or parent-directory symlink.
        let canonical = match path.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.errors.push(PackError::missing_file(
                    label,
                    format!("경로: {}", path.display()),
                ));
                return None;
            }
            Err(error) => {
                self.errors.push(PackError::Io(error));
                return None;
            }
        };
        if !canonical.starts_with(&self.canon_root) {
            self.errors.push(PackError::UnsafePath {
                path: path.display().to_string(),
            });
            return None;
        }
        let bytes = match std::fs::read(&canonical) {
            Ok(b) => b,
            Err(e) => {
                self.errors.push(PackError::Io(e));
                return None;
            }
        };
        if bytes.len() > MAX_FILE_BYTES {
            self.errors.push(PackError::Parse {
                file: label.to_string(),
                line: 0,
                col: 0,
                detail: format!(
                    "파일 크기가 {} 바이트 제한을 초과했습니다 ({} 바이트)",
                    MAX_FILE_BYTES,
                    bytes.len()
                ),
            });
            return None;
        }
        match String::from_utf8(bytes) {
            Ok(s) => Some(s),
            Err(e) => {
                self.errors.push(PackError::Parse {
                    file: label.to_string(),
                    line: 0,
                    col: 0,
                    detail: format!("UTF-8 디코딩 실패: {e}"),
                });
                None
            }
        }
    }

    fn parse_toml<T: serde::de::DeserializeOwned>(
        &mut self,
        label: &str,
        src: Option<String>,
    ) -> Option<T> {
        let src = src?;
        match toml::from_str::<T>(&src) {
            Ok(v) => Some(v),
            Err(e) => {
                self.errors.push(parse_error(label, &e, &src));
                None
            }
        }
    }
}

/// Validate a relative path the loader is about to join onto the Pack root.
///
/// Returns `Some(String)` when the path is safe (root-relative, no traversal,
/// no absolute components, no internal NUL); returns `None` otherwise.
/// The caller surfaces this as [`PackError::UnsafePath`].
pub fn validate_safe_relative_path(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    // Quick textual checks: no NUL bytes, no backslashes (treat as separator
    // on Unix), no leading `/`, no drive-letter `C:`-style prefixes.
    if raw.contains('\0') {
        return None;
    }
    if raw.starts_with('/') || raw.starts_with('\\') {
        return None;
    }
    if raw.len() >= 2 && raw.as_bytes()[1] == b':' {
        return None;
    }
    // Component-wise: reject `..`, reject absolute components, reject any
    // component that, when joined, escapes the root.
    let rel = Path::new(raw);
    for comp in rel.components() {
        use std::path::Component;
        match comp {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(raw.to_string())
}

/// Translate a `toml::de::Error` into a [`PackError::Parse`] with file
/// + line + column.
///
/// Line/column are computed from the error's byte span when available;
/// otherwise `0:0` is reported.
fn parse_error(file: &str, e: &toml::de::Error, src: &str) -> PackError {
    let (line, col) = e
        .span()
        .map(|s| byte_to_line_col(src, s.start))
        .unwrap_or((0, 0));
    PackError::Parse {
        file: file.to_string(),
        line,
        col,
        detail: e.message().to_string(),
    }
}

/// Convert a byte offset into a 1-based (line, column) pair.
fn byte_to_line_col(src: &str, byte_idx: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut col = 1usize;
    for (i, b) in src.bytes().enumerate() {
        if i >= byte_idx {
            break;
        }
        if b == b'\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_paths_accepted() {
        assert_eq!(
            validate_safe_relative_path("prompts/system.md").unwrap(),
            "prompts/system.md"
        );
        assert_eq!(
            validate_safe_relative_path("rules.toml").unwrap(),
            "rules.toml"
        );
        assert_eq!(
            validate_safe_relative_path("./rules.toml").unwrap(),
            "./rules.toml"
        );
    }

    #[test]
    fn unsafe_paths_rejected() {
        assert!(validate_safe_relative_path("../etc/passwd").is_none());
        assert!(validate_safe_relative_path("/etc/passwd").is_none());
        assert!(validate_safe_relative_path("a/../../b").is_none());
        assert!(validate_safe_relative_path("C:/windows").is_none());
        assert!(validate_safe_relative_path("\\windows").is_none());
        assert!(validate_safe_relative_path("").is_none());
        assert!(validate_safe_relative_path("a\0b").is_none());
    }
}
