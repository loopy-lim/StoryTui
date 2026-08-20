//! Integration test: load the committed `stories/starlight-station` example
//! Pack (a second, sci-fi genre pack) and assert it validates like the
//! rainy-reunion pack — proving the engine supports multiple novels.

use std::path::PathBuf;

use story_persistence::PackLoader;

fn starlight_station_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("stories")
        .join("starlight-station")
}

#[test]
fn starlight_station_loads_with_zero_errors_and_zero_warnings() {
    let root = starlight_station_root();
    assert!(
        root.is_dir(),
        "stories/starlight-station not found at {root:?}"
    );
    let pack = PackLoader::load(&root).unwrap_or_else(|e| panic!("starlight-station failed: {e}"));

    assert_eq!(pack.metadata.story_id, "starlight-station");
    assert_eq!(pack.metadata.title, "별빛 정거장");
    assert_eq!(pack.metadata.language, "ko-KR");
    assert!(pack.metadata.all_major_characters_safe);
    assert_eq!(pack.metadata.default_choice_count, 3);

    assert_eq!(
        pack.characters.len(),
        1,
        "MVP expects exactly 1 major character"
    );
    assert!(
        pack.characters
            .contains_key(&story_model::CharacterId("sera".into()))
    );
    assert_eq!(pack.rules.locations.len(), 3, "MVP expects 3 locations");
    assert!(pack.rules.allowed_flags.len() <= 5);

    // Routes, endings, events present and wired with the canonical DSL.
    assert!(pack.rules.routes.contains_key("truth"));
    assert!(pack.rules.endings.contains_key("ending-truth"));
    assert!(
        pack.rules
            .events
            .contains_key(&story_model::EventId("event.truth_revealed".into()))
    );

    // Markdown bodies and prompts are non-empty.
    assert!(!pack.opening_md.is_empty());
    assert!(!pack.prompts.system.is_empty());
    assert!(!pack.prompts.director.is_empty());
    assert!(!pack.prompts.writer.is_empty());
}

#[test]
fn starlight_station_initial_state_points_at_defined_location() {
    let root = starlight_station_root();
    let pack = PackLoader::load(&root).expect("loads");
    assert!(
        pack.rules
            .locations
            .contains_key(&pack.metadata.initial_state.location)
    );
    assert_eq!(pack.metadata.initial_state.location.0, "control-deck");
}
