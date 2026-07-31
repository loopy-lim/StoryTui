# W4 Story Pack Loader & Example Content Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Load a Story Pack (TOML + Markdown) into a canonical `StoryPack`, validating everything before play starts — file/encoding/path safety, schema-version compatibility, referential integrity, reachability, and teen metadata. Produce the `rainy-reunion` example Pack with zero validator errors/warnings. ST-011, ST-012, ST-013, ST-014, ST-015.

**Architecture:** Canonical `StoryPack` + `CharacterDef` + Pack DTOs live in `story-model` (pure data). The loader + file/parse/validation orchestration lives in `story-persistence` (depends on story-model, story-core, toml, semver, sha2, tempfile, thiserror). Semantic referential-integrity & reachability checks reuse `story-core` where possible. **The loader executes NO arbitrary code** (ADR-007) and collects ALL errors before reporting (Korean, with file/field/position).

**Tech Stack:** story-model, story-core, toml 1.1.4, semver 1.0.28, thiserror, serde/serde_json. No UI/HTTP.

---

## Part A — story-model: canonical Pack types (new file `pack.rs`)

```rust
pub const PACK_SCHEMA_VERSION: &str = "1.0.0";

pub struct PackMetadata {
    pub schema_version: semver::Version,     // from story.toml "schema_version"
    pub story_id: String,
    pub story_version: semver::Version,
    pub min_engine_version: semver::Version,
    pub title: String,
    pub language: String,                    // "ko-KR"
    pub genres: Vec<String>,
    pub rating: PackRating,
    pub all_major_characters_safe: bool,
    pub default_choice_count: u8,
    pub initial_state: PackInitialState,
    pub content: PackContentPolicy,
}
#[serde(rename_all="snake_case")] pub enum PackRating { General, Teen, teen }
pub struct PackInitialState { pub chapter: String, pub scene: String, pub location: LocationId, pub time_slot: TimeSlot }
#[serde(rename_all="snake_case")] pub struct PackContentPolicy { pub safe_content: String, pub violence: String }

pub struct CharacterDef {
    pub id: CharacterId, pub name: String, pub age: Option<u32>,
    pub pronouns: Option<String>,
    pub personality: Vec<String>, pub appearance: Vec<String>, pub values: Vec<String>,
    pub likes: Vec<String>, pub dislikes: Vec<String>, pub secrets: Vec<String>,
    pub initial_player_relationship: RelationshipStage,
    pub speech_style: String, pub honorific_rule: String,
    pub frequent_phrases: Vec<String>, pub avoid_phrases: Vec<String>,
    pub known_facts: Vec<String>, pub unknown_facts: Vec<String>,
    pub current_goal: String, pub long_term_desire: String,
    // optional per-emotion example lines, relationships — keep as Vec<...> or BTreeMap; omit if not in MVP fixture
}

pub struct PromptFragments { pub system: String, pub director: String, pub writer: String }

pub struct StoryPack {
    pub metadata: PackMetadata,
    pub characters: BTreeMap<CharacterId, CharacterDef>,
    pub rules: StoryRules,            // the W3 canonical rules type
    pub world_md: String,
    pub style_md: String,
    pub opening_md: String,
    pub prompts: PromptFragments,
    pub pack_root: std::path::PathBuf,   // canonical absolute root, for diagnostics only
}
```
Re-export from `lib.rs`. Serde round-trip + golden tests for these.

## Part B — story-persistence: DTOs, loader, validation

File layout `crates/story-persistence/src/`:
```
lib.rs
pack/
  mod.rs
  dto.rs          — raw TOML structs: StoryToml, CharactersToml ([[characters]]), RulesToml, EventsToml ([[events]])
  loader.rs       — PackLoader::load(root: &Path) -> Result<StoryPack, PackError>
  validate.rs     — referential integrity, reachability, teen metadata (collects all errors)
  migrate.rs      — same-major in-memory default fill; reject unsupported major
error.rs          — PackError (Korean messages, file/field/position)
```

### DTOs (dto.rs) — mirror the TOML field names from Technical Spec §6.2/§6.3
- `StoryToml`: `schema_version: String, story_id, story_version, min_engine_version, title, language, genres, rating, all_major_characters_safe, default_choice_count, [initial_state] {chapter,scene,location,time_slot}, [content] {safe_content, violence}`.
- `CharactersToml`: `schema_version: String, [[characters]] { id, name, age, pronouns, personality, appearance, values, likes, dislikes, secrets, initial_player_relationship, speech_style, honorific_rule, frequent_phrases, avoid_phrases, known_facts, unknown_facts, current_goal, long_term_desire }`.
- `RulesToml`: locations, allowed flags/numbers/strings/items, delta/stat limits, relationship rules. Map to the W3 `StoryRules` fields.
- `EventsToml`: `[[events]] { id, trigger(toml-encoded ConditionExpr), priority, effects{...}, allow_relationship_skip, route_lock }`.

Converters `dto -> canonical` (story.toml → PackMetadata; rules.toml → StoryRules; etc.).

### loader.rs — `PackLoader::load`
Order (each failure collected, all reported):
1. File existence + UTF-8 + allowed-filename + per-file size limit (e.g. 1 MiB). Required files: `story.toml`, `characters.toml`, `rules.toml`, `events.toml`, `world.md`, `style.md`, `opening.md`, `prompts/system.md`, `prompts/director.md`, `prompts/writer.md`. Missing required file ⇒ `PackError::MissingFile`.
2. **Path safety:** every referenced path must be root-relative; reject `..`/absolute/path-traversal ⇒ `PackError::UnsafePath`.
3. Parse TOML/Markdown. Parse errors include file + line/col + the TOML error span ⇒ `PackError::Parse`.
4. **Schema major compat:** compare `schema_version` major vs `PACK_SCHEMA_VERSION`. Same major → continue (minor diffs migrated in-memory). Unsupported major ⇒ `PackError::UnsupportedSchemaVersion`.
5. Build canonical types; collect referential-integrity errors (validate.rs).
6. Reachability/transition checks (validate.rs).
7. teen metadata check (validate.rs).
8. Return `Ok(StoryPack)` OR a single aggregated `PackError::Multiple(Vec<..>)` listing every problem (file/field/position, Korean).

**Original Pack files are NEVER written/mutated by load.** (Migration output is a separate safe `pack migrate --out` later — not in W4.)

### validate.rs
- `check_references(pack) -> Vec<PackError>`: every character/location/flag/number/string/item/event id referenced in conditions/effects exists; duplicate ids flagged; `ConditionExpr` refs resolve. Use `story-core`'s condition types (they're already `Serialize`/tagged; reuse, don't re-eval here — just structural id resolution).
- `check_reachability(pack) -> Vec<PackError>`: warn (not error) on unreachable required events/locations from the initial state; error on impossible location transitions not covered by adjacency or event `force_location`.
- `check_content(pack) -> Vec<PackError>`: if `rating == teen`, require `all_major_characters_safe == true` AND every major character `age >= Some(18)` (age present and ≥18). Else ⇒ `PackError::ContentPolicyViolation`.

### migrate.rs
- `migrate_in_memory(dto, target_major) -> Result<Dto, PackError>`: for same-major, fill missing optional fields with documented defaults; reject cross-major.

### error.rs — `PackError`
```rust
#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("파일 누락: {path} — {detail}")]
    MissingFile { path: String, detail: String },
    #[error("안전하지 않은 경로: {path}")]
    UnsafePath { path: String },
    #[error("구문 오류 ({file}:{line}:{col}): {detail}")]
    Parse { file: String, line: usize, col: usize, detail: String },
    #[error("지원하지 않는 Pack 스키마: {found} (지원 {supported})")]
    UnsupportedSchemaVersion { found: String, supported: String },
    #[error("참조 오류: {detail}")]
    Reference { detail: String },
    #[error("도달 불가: {detail}")]
    Unreachable { detail: String },
    #[error("모든 인물 Pack 정책 위반: {detail}")]
    ContentPolicyViolation { detail: String },
    #[error("팩 오류 {count}건:\n{items}")]
    Multiple { count: usize, items: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
}
```

## Part C — `rainy-reunion` example Pack fixture
Author the Pack under `stories/rainy-reunion/` with all required files. **Use the authoritative content from `docs/StoryTUI_Example_Story_Pack_KO.md`** (read it; adapt field names to the DTO schema above). Constraints (ST-015 / MVP): 1 major character, 3 locations, ≤5 flags, `default_choice_count = 3`, Korean prose, opening.md + system/director/writer prompts, character speech/knowledge, rules + events. Must validate with **0 errors and 0 warnings**.

## Tasks (TDD)
- **W4.1 (story-model):** `pack.rs` canonical types + serde round-trip/golden. Commit `feat(story-model): canonical StoryPack and character definition types`.
- **W4.2 (story-persistence):** DTOs + converters + `PackError`. Round-trip DTO↔canonical. Commit `feat(story-persistence): Story Pack TOML DTOs and converters`.
- **W4.3:** `loader.rs` happy path (load a hand-built valid Pack dir from a tempdir) + file/encoding/path-traversal error tests. Commit `feat(story-persistence): Story Pack loader with safety checks`.
- **W4.4:** `migrate.rs` + schema-major test (same-major ok, future-major rejected). Commit `feat(story-persistence): Pack schema version and in-memory migration`.
- **W4.5:** `validate.rs` referential integrity + reachability + teen metadata; bad fixtures for each error code. Commit `feat(story-persistence): Pack referential integrity, reachability, teen validation`.
- **W4.6:** `rainy-reunion` Pack fixture (from example doc) + integration test `load("stories/rainy-reunion")` → 0 errors/0 warnings. Commit `feat(stories): rainy-reunion example Story Pack`.

## Completion gate (ST-011..015)
- `cargo test --workspace` green; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --check` clean.
- `./scripts/check-core-deps.sh` OK; `story-persistence` does NOT pull UI crates (it may pull std::fs, toml, sha2 — that's allowed; only story-core/story-model are FS-free).
- Good Pack loads to canonical `StoryPack`; each bad fixture fails with the expected error code (missing file, malformed TOML, path traversal, unsupported major, missing ref, duplicate id, teen-violation).
- `rainy-reunion` validates with 0 errors / 0 warnings; loader executes no arbitrary code (no `eval`, no script execution — only TOML/Markdown parse).
