//! Deterministic PromptContext tests (ST-018).
//!
//! Three contracts pinned here:
//! 1. Golden snapshot — the full prompt (system/user/hash) for a fixed
//!    (pack, state, recent) is byte-stable across runs.
//! 2. Determinism / ordering-independence — building the SAME logical pack and
//!    state with the `BTreeMap`s populated in REVERSED key order yields an
//!    identical system, user, and hash.
//! 3. Format — the hash is exactly 64 lowercase hex characters.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use story_core::prompt_context::{PromptBundle, build_prompt};
use story_model::{
    ActionType, CharacterDef, CharacterId, CharacterState, Emotion, GameState, HistoryEntry,
    LocationDef, LocationId, PackContentPolicy, PackInitialState, PackMetadata, PackRating,
    ParsedAction, PromptFragments, RelationshipStage, SAVE_SCHEMA_VERSION, StatePatch, StoryPack,
    StoryRules, TimeSlot,
};

// ---------------------------------------------------------------------------
// fixture builders
// ---------------------------------------------------------------------------

fn sample_metadata() -> PackMetadata {
    PackMetadata {
        schema_version: semver::Version::parse("1.0.0").unwrap(),
        story_id: "rainy-reunion".to_string(),
        story_version: semver::Version::new(0, 1, 0),
        min_engine_version: semver::Version::new(0, 1, 0),
        title: "비가 그친 뒤".to_string(),
        language: "ko-KR".to_string(),
        genres: vec!["modern-romance".to_string()],
        rating: PackRating::teen,
        all_major_characters_safe: true,
        default_choice_count: 3,
        initial_state: PackInitialState {
            chapter: "chapter-1".to_string(),
            scene: "cafe-reunion".to_string(),
            location: LocationId("cafe-namu".to_string()),
            time_slot: TimeSlot::Evening,
        },
        content: PackContentPolicy {
            safe_content: "permitted_by_pack".to_string(),
            violence: "mild".to_string(),
        },
    }
}

fn sample_character(id: &str) -> CharacterDef {
    CharacterDef {
        id: CharacterId(id.to_string()),
        name: id.to_string(),
        age: Some(29),
        pronouns: None,
        personality: vec![],
        appearance: vec![],
        values: vec![],
        likes: vec![],
        dislikes: vec![],
        secrets: vec![],
        initial_player_relationship: RelationshipStage::Acquaintance,
        speech_style: "".to_string(),
        honorific_rule: "".to_string(),
        frequent_phrases: vec![],
        avoid_phrases: vec![],
        known_facts: vec![],
        unknown_facts: vec![],
        current_goal: "".to_string(),
        long_term_desire: "".to_string(),
    }
}

/// Build the full fixture in either "forward" or "reversed" key-insertion
/// order. The two results must be logically identical — this is what the
/// ordering-independence test verifies.
fn fixture(reversed: bool) -> (StoryPack, GameState, Vec<HistoryEntry>) {
    // Three characters, three locations, three flags, three numbers — inserted
    // in either forward or reversed order to exercise BTreeMap independence.
    let char_keys = if reversed {
        vec!["zoe", "yuna", "ami"]
    } else {
        vec!["ami", "yuna", "zoe"]
    };
    let loc_keys = if reversed {
        vec!["loc-roof", "loc-hall", "cafe-namu"]
    } else {
        vec!["cafe-namu", "loc-hall", "loc-roof"]
    };
    let flag_keys = if reversed {
        vec!["shared_umbrella", "apologized", "met_namu"]
    } else {
        vec!["met_namu", "apologized", "shared_umbrella"]
    };
    let num_keys = if reversed {
        vec![("yuna.trust", 4), ("yuna.affection", 9), ("score", 1)]
    } else {
        vec![("score", 1), ("yuna.affection", 9), ("yuna.trust", 4)]
    };

    let mut characters: BTreeMap<CharacterId, CharacterDef> = BTreeMap::new();
    for k in &char_keys {
        characters.insert(CharacterId(k.to_string()), sample_character(k));
    }

    let mut locations: BTreeMap<LocationId, LocationDef> = BTreeMap::new();
    for k in &loc_keys {
        locations.insert(
            LocationId(k.to_string()),
            LocationDef {
                id: LocationId(k.to_string()),
                display_name: k.to_string(),
                adjacent: BTreeSet::new(),
            },
        );
    }

    let mut allowed_flags: BTreeSet<String> = BTreeSet::new();
    for k in &flag_keys {
        allowed_flags.insert(k.to_string());
    }

    let mut rules = StoryRules::minimal();
    rules.locations = locations;
    rules.allowed_flags = allowed_flags;
    // delta_limits stay at the default -3..=3 for all three axes.

    let pack = StoryPack {
        metadata: sample_metadata(),
        characters,
        rules,
        world_md: "# 세계관\n카페 나무, 비 오는 저녁.".to_string(),
        style_md: "# 문체\n짧고 정확한 문장.".to_string(),
        opening_md: "# 오프닝\n유나가 앉아 있다.".to_string(),
        prompts: PromptFragments {
            system: "당신은 StoryTUI의 한국어 장면 생성 모델이다.".to_string(),
            director: "director".to_string(),
            writer: "writer".to_string(),
        },
        pack_root: std::path::PathBuf::from("/tmp/rainy-reunion"),
    };

    // State with characters/flags/numbers/inventory built in either order.
    // Character stats are keyed by name (not iteration index) so the logical
    // state is identical regardless of insertion order — this is the whole
    // point of the ordering-independence test.
    let stat_for = |k: &str| -> (i16, i16, i16) {
        match k {
            "ami" => (10, 5, 0),
            "yuna" => (11, 6, 1),
            "zoe" => (12, 7, 2),
            _ => (0, 0, 0),
        }
    };
    let mut state_chars: BTreeMap<CharacterId, CharacterState> = BTreeMap::new();
    for k in &char_keys {
        let (aff, tru, ten) = stat_for(k);
        state_chars.insert(
            CharacterId(k.to_string()),
            CharacterState {
                affection: aff,
                trust: tru,
                tension: ten,
                relationship_stage: RelationshipStage::Acquaintance,
                current_emotion: Emotion::Neutral,
                known_facts: BTreeSet::new(),
                address_form_overrides: BTreeMap::new(),
            },
        );
    }
    let mut flags: BTreeMap<String, bool> = BTreeMap::new();
    for k in &flag_keys {
        flags.insert(k.to_string(), *k == "met_namu");
    }
    let mut numbers: BTreeMap<String, i64> = BTreeMap::new();
    for (k, v) in &num_keys {
        numbers.insert(k.to_string(), *v);
    }
    let mut inventory: BTreeMap<String, u32> = BTreeMap::new();
    inventory.insert("i-letter".to_string(), 2);

    let state = GameState {
        save_schema_version: SAVE_SCHEMA_VERSION,
        story_id: "rainy-reunion".to_string(),
        story_version: semver::Version::new(0, 1, 0),
        pack_schema_version: semver::Version::new(1, 0, 0),
        session_id: uuid::Uuid::nil(),
        current_chapter: "chapter-1".to_string(),
        current_scene_id: "cafe-reunion".to_string(),
        turn: 3,
        location: LocationId("cafe-namu".to_string()),
        time_slot: TimeSlot::Evening,
        active_route: None,
        characters: state_chars,
        flags,
        numbers,
        strings: BTreeMap::new(),
        inventory,
        occurred_events: BTreeSet::new(),
        recent_summaries: VecDeque::new(),
        memories: Vec::new(),
        history: Vec::new(),
        last_choice: None,
        revision: 2,
        stable_generation_id: None,
    };

    let recent = vec![
        HistoryEntry {
            id: uuid::Uuid::nil(),
            turn: 1,
            scene_id: "cafe-reunion".to_string(),
            created_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            narration: "유나가 커피를 내렸다.".to_string(),
            player_action: Some(ParsedAction {
                action_type: ActionType::Dialogue,
                target: Some(CharacterId("yuna".to_string())),
                intent: Some("greet".to_string()),
                tone: None,
                content: "오래간만이야.".to_string(),
                original_text: "오래간만이야.".to_string(),
                confidence: None,
            }),
            choices: vec![],
            applied_patch: StatePatch::default(),
            generation_id: uuid::Uuid::nil(),
        },
        HistoryEntry {
            id: uuid::Uuid::nil(),
            turn: 2,
            scene_id: "cafe-reunion".to_string(),
            created_at: chrono::DateTime::from_timestamp(1_700_000_060, 0).unwrap(),
            narration: "비가 창을 때렸다.".to_string(),
            player_action: None,
            choices: vec![],
            applied_patch: StatePatch::default(),
            generation_id: uuid::Uuid::nil(),
        },
    ];

    (pack, state, recent)
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[test]
fn golden_prompt_snapshot_is_stable() {
    let (pack, state, recent) = fixture(false);
    let bundle = build_prompt(&pack, &state, &recent, None);
    let combined = format!(
        "=== SYSTEM ===\n{}\n\n=== USER ===\n{}\n\n=== HASH ===\n{}",
        bundle.system, bundle.user, bundle.hash
    );
    insta::assert_snapshot!(combined);
}

#[test]
fn hash_is_64_lowercase_hex_chars() {
    let (pack, state, recent) = fixture(false);
    let bundle = build_prompt(&pack, &state, &recent, None);
    assert_eq!(bundle.hash.len(), 64);
    assert!(
        bundle
            .hash
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "hash must be lowercase hex: {}",
        bundle.hash
    );
}

#[test]
fn same_inputs_produce_identical_bundle() {
    // Determinism: pure function of inputs.
    let (pack, state, recent) = fixture(false);
    let b1 = build_prompt(&pack, &state, &recent, None);
    let b2 = build_prompt(&pack, &state, &recent, None);
    assert_eq!(b1, b2);
}

#[test]
fn ordering_independent_of_btreemap_construction_order() {
    // The critical determinism contract: building the SAME logical pack/state
    // with BTreeMaps populated in REVERSED key order yields byte-identical
    // system, user, and hash. This would fail if any iteration depended on
    // insertion order rather than BTreeMap key order.
    let (pack_f, state_f, recent_f) = fixture(false);
    let (pack_r, state_r, recent_r) = fixture(true);

    let bf: PromptBundle = build_prompt(&pack_f, &state_f, &recent_f, None);
    let br: PromptBundle = build_prompt(&pack_r, &state_r, &recent_r, None);

    assert_eq!(bf.system, br.system, "system differs under reordering");
    assert_eq!(bf.user, br.user, "user differs under reordering");
    assert_eq!(bf.hash, br.hash, "hash differs under reordering");
}

#[test]
fn different_recent_history_changes_hash() {
    // Sanity: the hash is sensitive to inputs (not a constant).
    let (pack, state, recent) = fixture(false);
    let b1 = build_prompt(&pack, &state, &recent, None);
    let b2 = build_prompt(&pack, &state, &recent[..recent.len() - 1], None);
    assert_ne!(b1.hash, b2.hash, "hash must react to history changes");
}
