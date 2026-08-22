//! Integration tests that exercise the W4.5 validators end-to-end via the
//! loader. Bad fixtures cover missing references and duplicate identifiers.
//! The good fixture is a complete supported pack with one major character.

use std::fs;
use std::path::Path;

use story_persistence::PackLoader;
use tempfile::TempDir;

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
rating = "teen"
default_choice_count = 3

[initial_state]
chapter = "c1"
scene = "s1"
location = "cafe-namu"
time_slot = "evening"

[content]
violence = "mild"
"#;

const RULES_TOML_GOOD: &str = r#"
[limits]
normal_delta_min = -3
normal_delta_max = 3
affection_min = -30
affection_max = 30
trust_min = -30
trust_max = 30
tension_min = 0
tension_max = 30

[[locations]]
id = "cafe-namu"
name = "카페 나무"
connects_to = ["riverside-walk"]

[[locations]]
id = "riverside-walk"
name = "강변 산책로"
connects_to = ["cafe-namu"]

[[flags]]
id = "apologized"

[[numbers]]
id = "yuna.affection"
default = 0
min = -30
max = 30

[[routes]]
id = "reconciliation"
premise = "재회"
enter_condition = { kind = "all", children = [
  { kind = "flag_equals", id = "apologized", value = true },
  { kind = "trust_at_least", character = "yuna", value = 8 },
]}

[[endings]]
id = "ending-new-beginning"
label = "새로운 시작"
condition = { kind = "all", children = [
  { kind = "affection_at_least", character = "yuna", value = 12 },
  { kind = "trust_at_least", character = "yuna", value = 12 },
]}

[[events]]
id = "event.first_honest_exchange"
priority = 10
trigger = { kind = "all", children = [
  { kind = "flag_equals", id = "apologized", value = true },
  { kind = "trust_at_least", character = "yuna", value = 2 },
]}

[events.effects]
set_flags = { "apologized" = true }
"#;

fn base_valid_pack() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(root, "story.toml", STORY_TOML);
    write(
        root,
        "characters.toml",
        r#"
schema_version = "1.0.0"
[[characters]]
id = "yuna"
name = "한유나"
age = 29
initial_player_relationship = "acquaintance"
speech_style = "짧은 문장."
honorific_rule = "해요체"
current_goal = "재회"
long_term_desire = "관계"
"#,
    );
    write(root, "rules.toml", RULES_TOML_GOOD);
    write(root, "events.toml", "schema_version = \"1.0.0\"\n");
    write(root, "world.md", "# 세계관\n");
    write(root, "style.md", "# 문체\n");
    write(root, "opening.md", "# 오프닝\n");
    write(root, "prompts/system.md", "system");
    write(root, "prompts/director.md", "director");
    write(root, "prompts/writer.md", "writer");
    dir
}

#[test]
fn good_pack_loads_clean() {
    let dir = base_valid_pack();
    let pack = PackLoader::load(dir.path()).expect("valid pack");
    assert_eq!(pack.metadata.rating, story_model::PackRating::Teen);
    assert_eq!(pack.characters.len(), 1);
    assert_eq!(pack.rules.locations.len(), 2);
    assert!(pack.rules.routes.contains_key("reconciliation"));
}

#[test]
fn unknown_flag_reference_is_caught() {
    let dir = base_valid_pack();
    // Replace the route's enter_condition flag with an unregistered one.
    let rules_path = dir.path().join("rules.toml");
    let s = fs::read_to_string(&rules_path).unwrap().replace(
        "{ kind = \"flag_equals\", id = \"apologized\", value = true },",
        "{ kind = \"flag_equals\", id = \"ghost_flag\", value = true },",
    );
    fs::write(&rules_path, s).unwrap();
    let err = PackLoader::load(dir.path()).expect_err("unknown flag");
    let msg = format!("{err}");
    assert!(
        msg.contains("참조 오류") && msg.contains("ghost_flag"),
        "expected reference error mentioning ghost_flag: {msg}"
    );
}

#[test]
fn unknown_event_effect_flag_is_caught() {
    let dir = base_valid_pack();
    // Inject an event in events.toml that references an unregistered flag.
    write(
        dir.path(),
        "events.toml",
        r#"
schema_version = "1.0.0"
[[events]]
id = "event.x"
priority = 1
trigger = { kind = "flag_equals", id = "apologized", value = true }
[events.effects]
set_flags = { "ghost_flag" = true }
"#,
    );
    let err = PackLoader::load(dir.path()).expect_err("unknown event effect flag");
    let msg = format!("{err}");
    assert!(
        msg.contains("참조 오류") && msg.contains("ghost_flag"),
        "expected reference error mentioning ghost_flag: {msg}"
    );
}

#[test]
fn unknown_event_effect_trust_delta_character_is_caught() {
    let dir = base_valid_pack();
    // trust_delta on a character id that is not in characters.toml must be
    // flagged by the Pack referential-integrity pass on the new typed field
    // (previously the loader routed trust/tension into number_delta, masking
    // a bad character id behind a "{cid}.trust" number-variable lookup).
    write(
        dir.path(),
        "events.toml",
        r#"
schema_version = "1.0.0"
[[events]]
id = "event.x"
priority = 1
trigger = { kind = "flag_equals", id = "apologized", value = true }
[events.effects]
trust_delta = { "ghost_char" = 1 }
"#,
    );
    let err = PackLoader::load(dir.path()).expect_err("unknown trust_delta character");
    let msg = format!("{err}");
    assert!(
        msg.contains("참조 오류") && msg.contains("ghost_char"),
        "expected reference error mentioning ghost_char: {msg}"
    );
}

#[test]
fn unknown_event_effect_tension_delta_character_is_caught() {
    let dir = base_valid_pack();
    write(
        dir.path(),
        "events.toml",
        r#"
schema_version = "1.0.0"
[[events]]
id = "event.x"
priority = 1
trigger = { kind = "flag_equals", id = "apologized", value = true }
[events.effects]
tension_delta = { "ghost_char" = 1 }
"#,
    );
    let err = PackLoader::load(dir.path()).expect_err("unknown tension_delta character");
    let msg = format!("{err}");
    assert!(
        msg.contains("참조 오류") && msg.contains("ghost_char"),
        "expected reference error mentioning ghost_char: {msg}"
    );
}

#[test]
fn duplicate_character_id_is_caught() {
    let dir = base_valid_pack();
    let chars_path = dir.path().join("characters.toml");
    let mut s = fs::read_to_string(&chars_path).unwrap();
    // append a second yuna entry
    s.push_str(
        r#"
[[characters]]
id = "yuna"
name = "복제 유나"
age = 30
initial_player_relationship = "acquaintance"
current_goal = "x"
long_term_desire = "x"
"#,
    );
    fs::write(&chars_path, s).unwrap();
    let err = PackLoader::load(dir.path()).expect_err("dup id");
    let msg = format!("{err}");
    assert!(msg.contains("중복"), "expected duplicate id error: {msg}");
}

#[test]
fn unknown_location_in_initial_state_is_reference_error() {
    let dir = base_valid_pack();
    let story_path = dir.path().join("story.toml");
    let s = fs::read_to_string(&story_path)
        .unwrap()
        .replace("location = \"cafe-namu\"", "location = \"ghost-loc\"");
    fs::write(&story_path, s).unwrap();
    let err = PackLoader::load(dir.path()).expect_err("unknown init loc");
    let msg = format!("{err}");
    assert!(msg.contains("참조 오류"), "expected reference error: {msg}");
}

#[test]
fn duplicate_location_id_is_rejected_instead_of_overwritten() {
    let dir = base_valid_pack();
    let rules_path = dir.path().join("rules.toml");
    let mut rules = fs::read_to_string(&rules_path).unwrap();
    rules.push_str(
        r#"
[[locations]]
id = "cafe-namu"
name = "덮어쓰면 안 되는 장소"
connects_to = []
"#,
    );
    fs::write(rules_path, rules).unwrap();
    let err = PackLoader::load(dir.path()).expect_err("duplicate location");
    let message = format!("{err}");
    assert!(
        message.contains("중복 location id"),
        "unexpected: {message}"
    );
}

#[test]
fn duplicate_event_id_is_rejected_instead_of_overwritten() {
    let dir = base_valid_pack();
    write(
        dir.path(),
        "events.toml",
        r#"
schema_version = "1.0.0"
[[events]]
id = "event.same"
trigger = { kind = "turn_at_least", value = 1 }
[[events]]
id = "event.same"
trigger = { kind = "turn_at_least", value = 2 }
"#,
    );
    let err = PackLoader::load(dir.path()).expect_err("duplicate event");
    assert!(format!("{err}").contains("중복 event id"));
}

#[test]
fn reversed_stat_range_is_rejected() {
    let dir = base_valid_pack();
    let rules_path = dir.path().join("rules.toml");
    let rules = fs::read_to_string(&rules_path)
        .unwrap()
        .replace("affection_min = -30", "affection_min = 31");
    fs::write(rules_path, rules).unwrap();
    let err = PackLoader::load(dir.path()).expect_err("reversed range");
    assert!(format!("{err}").contains("limits.affection 범위 오류"));
}

#[test]
fn number_default_outside_range_is_rejected() {
    let dir = base_valid_pack();
    let rules_path = dir.path().join("rules.toml");
    let rules = fs::read_to_string(&rules_path)
        .unwrap()
        .replace("default = 0\nmin = -30", "default = 31\nmin = -30");
    fs::write(rules_path, rules).unwrap();
    let err = PackLoader::load(dir.path()).expect_err("number default out of range");
    assert!(format!("{err}").contains("default 31"));
}

#[test]
fn disconnected_location_is_rejected() {
    let dir = base_valid_pack();
    let rules_path = dir.path().join("rules.toml");
    let mut rules = fs::read_to_string(&rules_path).unwrap();
    rules.push_str(
        r#"
[[locations]]
id = "isolated-room"
name = "고립된 방"
connects_to = []
"#,
    );
    fs::write(rules_path, rules).unwrap();
    let err = PackLoader::load(dir.path()).expect_err("unreachable location");
    let message = format!("{err}");
    assert!(message.contains("도달 불가"), "unexpected: {message}");
    assert!(message.contains("isolated-room"));
}
