//! Integration tests for `PackLoader` (W4.3).
//!
//! These tests build small Pack directories in a `tempfile::TempDir` and
//! assert that the loader produces the right `PackError` variant for each
//! defect: missing file, malformed TOML, unsafe path traversal, oversized
//! file, and (happy path) a complete pack that loads cleanly.

use std::fs;
use std::path::Path;

use story_persistence::PackLoader;
use tempfile::TempDir;

/// Write `text` to `root/rel`, creating parent dirs as needed.
fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, text).unwrap();
}

const STORY_TOML: &str = r#"schema_version = "1.0.0"
story_id = "demo"
story_version = "0.1.0"
min_engine_version = "0.1.0"
title = "데모"
language = "ko-KR"
genres = ["drama"]
rating = "general"
default_choice_count = 3

[initial_state]
chapter = "c1"
scene = "s1"
location = "loc1"
time_slot = "evening"

[content]
violence = "mild"
"#;

const CHARACTERS_TOML: &str = r#"
schema_version = "1.0.0"

[[characters]]
id = "yuna"
name = "유나"
age = 29
initial_player_relationship = "acquaintance"
speech_style = "짧은 문장."
honorific_rule = "해요체"
current_goal = "재회"
long_term_desire = "관계"
"#;

const RULES_TOML: &str = r#"
[limits]
normal_delta_min = -3
normal_delta_max = 3
affection_min = -30
affection_max = 30

[[locations]]
id = "loc1"
name = "첫 장소"
connects_to = []
"#;

const EVENTS_TOML: &str = "schema_version = \"1.0.0\"\n";

/// Build a complete valid Pack into a fresh TempDir.
fn build_valid_pack() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(root, "story.toml", STORY_TOML);
    write(root, "characters.toml", CHARACTERS_TOML);
    write(root, "rules.toml", RULES_TOML);
    write(root, "events.toml", EVENTS_TOML);
    write(root, "world.md", "# 세계관\n");
    write(root, "style.md", "# 문체\n");
    write(root, "opening.md", "# 오프닝\n");
    write(root, "prompts/system.md", "system prompt");
    write(root, "prompts/director.md", "director prompt");
    write(root, "prompts/writer.md", "writer prompt");
    dir
}

#[test]
fn happy_path_loads_canonical_pack() {
    let dir = build_valid_pack();
    let pack = PackLoader::load(dir.path()).expect("valid pack loads");
    assert_eq!(pack.metadata.story_id, "demo");
    assert_eq!(pack.metadata.default_choice_count, 3);
    assert_eq!(pack.metadata.initial_state.location.0, "loc1");
    assert_eq!(pack.characters.len(), 1);
    assert_eq!(pack.world_md, "# 세계관\n");
    assert_eq!(pack.prompts.writer, "writer prompt");
    assert!(
        pack.rules
            .locations
            .contains_key(&story_model::LocationId("loc1".into()))
    );
    // pack_root is canonical absolute for diagnostics only.
    assert!(pack.pack_root.is_absolute());
}

#[test]
fn missing_required_file_is_reported() {
    let dir = build_valid_pack();
    fs::remove_file(dir.path().join("rules.toml")).unwrap();
    let err = PackLoader::load(dir.path()).expect_err("missing rules.toml");
    let s = format!("{err}");
    assert!(
        s.contains("파일 누락") && s.contains("rules.toml"),
        "expected missing-file error for rules.toml, got: {s}"
    );
}

#[test]
fn missing_required_file_aggregates_with_other_missing() {
    let dir = build_valid_pack();
    fs::remove_file(dir.path().join("rules.toml")).unwrap();
    fs::remove_file(dir.path().join("events.toml")).unwrap();
    let err = PackLoader::load(dir.path()).expect_err("multiple missing");
    let s = format!("{err}");
    // Both missing-file errors must appear in the aggregated bundle; later
    // validators may add more (e.g. unknown initial_state.location) so we
    // assert each missing-file entry is present, not the exact count.
    assert!(s.contains("팩 오류"), "expected aggregation: {s}");
    assert!(s.contains("rules.toml"));
    assert!(s.contains("events.toml"));
}

#[test]
fn malformed_toml_is_parse_error_with_position() {
    let dir = build_valid_pack();
    write(
        dir.path(),
        "story.toml",
        "schema_version = \"1.0.0\"\nthis is not = valid = toml\n",
    );
    let err = PackLoader::load(dir.path()).expect_err("malformed toml");
    let s = format!("{err}");
    assert!(s.contains("구문 오류"), "expected Parse error: {s}");
    assert!(s.contains("story.toml"));
}

#[test]
fn non_utf8_file_is_parse_error() {
    let dir = build_valid_pack();
    fs::write(dir.path().join("world.md"), [0xff, 0xfe, 0x00]).unwrap();
    let err = PackLoader::load(dir.path()).expect_err("non-utf8");
    let s = format!("{err}");
    assert!(s.contains("UTF-8"));
}

#[test]
fn oversized_file_is_reported() {
    let dir = build_valid_pack();
    let huge = "a".repeat(story_persistence::pack::loader::MAX_FILE_BYTES + 1);
    write(dir.path(), "world.md", &huge);
    let err = PackLoader::load(dir.path()).expect_err("oversize");
    let s = format!("{err}");
    assert!(s.contains("크기"));
}

#[test]
fn path_traversal_in_files_section_is_unsafe_path() {
    let dir = build_valid_pack();
    // Inject a [files] override that tries to escape the Pack root.
    let with_traversal = "schema_version = \"1.0.0\"\n\
         story_id = \"demo\"\n\
         story_version = \"0.1.0\"\n\
         min_engine_version = \"0.1.0\"\n\
         title = \"x\"\n\
         language = \"ko-KR\"\n\
         rating = \"general\"\n\
         [files]\n\
         rules = \"../../etc/passwd\"\n\
         [initial_state]\n\
         chapter = \"c1\"\n\
         scene = \"s1\"\n\
         location = \"loc1\"\n\
         time_slot = \"evening\"\n";
    write(dir.path(), "story.toml", with_traversal);
    let err = PackLoader::load(dir.path()).expect_err("traversal");
    assert!(err.is_unsafe_path(), "expected UnsafePath, got: {err}");
}

#[cfg(unix)]
#[test]
fn symlink_escape_is_rejected_before_external_file_is_read() {
    use std::os::unix::fs::symlink;

    let dir = build_valid_pack();
    let outside = TempDir::new().unwrap();
    let external = outside.path().join("external-secret.md");
    fs::write(&external, "Pack 밖의 민감한 내용").unwrap();
    let world = dir.path().join("world.md");
    fs::remove_file(&world).unwrap();
    symlink(&external, &world).unwrap();

    let err = PackLoader::load(dir.path()).expect_err("escape symlink must fail closed");
    assert!(err.is_unsafe_path(), "expected UnsafePath, got: {err}");
    assert!(
        !format!("{err}").contains("민감한 내용"),
        "diagnostic must not echo external file content"
    );
}

#[test]
fn root_not_a_directory_is_missing_file() {
    let dir = build_valid_pack();
    let file_path = dir.path().join("story.toml");
    let err = PackLoader::load(&file_path).expect_err("not a dir");
    let s = format!("{err}");
    assert!(s.contains("루트") || s.contains("디렉터리"));
}

#[test]
fn load_does_not_mutate_pack_files() {
    let dir = build_valid_pack();
    let story_path = dir.path().join("story.toml");
    let before = fs::read(&story_path).unwrap();
    let _ = PackLoader::load(dir.path()).expect("loads");
    let after = fs::read(&story_path).unwrap();
    assert_eq!(before, after, "load must not mutate story.toml");
    // also confirm no extra files appeared
    let before_count = fs::read_dir(dir.path()).unwrap().count();
    let _ = PackLoader::load(dir.path()).expect("loads again");
    let after_count = fs::read_dir(dir.path()).unwrap().count();
    assert_eq!(before_count, after_count);
}

#[test]
fn unsupported_schema_major_is_rejected() {
    let dir = build_valid_pack();
    let future = STORY_TOML.replace("\"1.0.0\"", "\"2.0.0\"");
    write(dir.path(), "story.toml", &future);
    let err = PackLoader::load(dir.path()).expect_err("future major rejected");
    let s = format!("{err}");
    assert!(
        s.contains("지원하지 않는 Pack 스키마"),
        "expected unsupported, got: {s}"
    );
    assert!(s.contains("2.0.0"));
}

#[test]
fn forward_minor_within_same_major_loads() {
    let dir = build_valid_pack();
    let future = STORY_TOML.replace("\"1.0.0\"", "\"1.7.3\"");
    write(dir.path(), "story.toml", &future);
    let pack = PackLoader::load(dir.path()).expect("forward minor loads");
    assert_eq!(pack.metadata.schema_version.minor, 7);
    assert_eq!(pack.metadata.schema_version.patch, 3);
}

#[test]
fn future_engine_requirement_is_rejected() {
    let dir = build_valid_pack();
    let future = STORY_TOML.replace(
        "min_engine_version = \"0.1.0\"",
        "min_engine_version = \"99.0.0\"",
    );
    write(dir.path(), "story.toml", &future);
    let err = PackLoader::load(dir.path()).expect_err("future engine requirement");
    let message = format!("{err}");
    assert!(message.contains("엔진 버전"), "unexpected error: {message}");
    assert!(message.contains("99.0.0"));
}

#[test]
fn choice_count_outside_provider_contract_is_rejected() {
    for invalid in [0, 7] {
        let dir = build_valid_pack();
        let story = STORY_TOML.replace(
            "default_choice_count = 3",
            &format!("default_choice_count = {invalid}"),
        );
        write(dir.path(), "story.toml", &story);
        let err = PackLoader::load(dir.path()).expect_err("invalid choice count");
        assert!(
            format!("{err}").contains("default_choice_count"),
            "unexpected error for {invalid}: {err}"
        );
    }
}
