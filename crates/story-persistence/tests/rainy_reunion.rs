//! Integration test: load the committed `stories/rainy-reunion` example Pack
//! and assert it validates with 0 errors and 0 warnings (W4.6, ST-015).

use std::path::PathBuf;

use story_persistence::PackLoader;

fn rainy_reunion_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `crates/story-persistence`; the workspace root
    // is two levels up.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent() // crates/
        .unwrap()
        .parent() // workspace root
        .unwrap()
        .join("stories")
        .join("rainy-reunion")
}

#[test]
fn rainy_reunion_loads_with_zero_errors_and_zero_warnings() {
    let root = rainy_reunion_root();
    assert!(root.is_dir(), "stories/rainy-reunion not found at {root:?}");
    let pack = PackLoader::load(&root).unwrap_or_else(|e| panic!("rainy-reunion failed: {e}"));

    // Identity & metadata.
    assert_eq!(pack.metadata.story_id, "rainy-reunion");
    assert_eq!(pack.metadata.title, "비가 그친 뒤");
    assert_eq!(pack.metadata.language, "ko-KR");
    assert_eq!(pack.metadata.rating, story_model::PackRating::Teen);
    assert_eq!(pack.metadata.default_choice_count, 3);
    assert_eq!(pack.metadata.schema_version.major, 1);

    // 1 major character, 3 locations, <=5 flags.
    assert_eq!(
        pack.characters.len(),
        1,
        "MVP expects exactly 1 major character"
    );
    assert!(
        pack.characters
            .contains_key(&story_model::CharacterId("yuna".into()))
    );
    assert_eq!(pack.rules.locations.len(), 3, "MVP expects 3 locations");
    assert!(pack.rules.allowed_flags.len() <= 5);
    assert!(pack.rules.allowed_flags.contains("apologized"));

    // Routes, endings, events present and wired with the canonical DSL.
    assert!(pack.rules.routes.contains_key("reconciliation"));
    assert!(pack.rules.endings.contains_key("ending-new-beginning"));
    assert!(
        pack.rules
            .events
            .contains_key(&story_model::EventId("event.reconciled".into()))
    );

    // Markdown bodies and prompts are non-empty.
    assert!(pack.opening_md.contains("오랜만이에요"));
    assert!(!pack.prompts.system.is_empty());
    assert!(!pack.prompts.director.is_empty());
    assert!(!pack.prompts.writer.is_empty());

    // pack_root is the canonical absolute path for diagnostics only.
    assert!(pack.pack_root.is_absolute());
}

#[test]
fn rainy_reunion_initial_state_points_at_defined_location() {
    let root = rainy_reunion_root();
    let pack = PackLoader::load(&root).expect("loads");
    assert!(
        pack.rules
            .locations
            .contains_key(&pack.metadata.initial_state.location)
    );
    assert_eq!(
        pack.metadata.initial_state.location.0,
        "cafe-namu".to_string()
    );
}
