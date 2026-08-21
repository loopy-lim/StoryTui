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

use story_core::prompt_context::{PromptBundle, build_prompt, build_prompt_with_choice};
use story_model::{
    ActionType, CharacterDef, CharacterId, CharacterState, ConditionExpr, Emotion, EventDef,
    EventEffects, EventId, GameState, HistoryEntry, LocationDef, LocationId, PackContentPolicy,
    PackInitialState, PackMetadata, PackRating, ParsedAction, PromptFragments, RelationshipStage,
    SAVE_SCHEMA_VERSION, StatePatch, StoryPack, StoryRules, TimeSlot,
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
fn later_turn_prompt_does_not_repeat_opening_prose() {
    let (pack, state, recent) = fixture(false);
    let bundle = build_prompt(&pack, &state, &recent, None);
    assert!(!bundle.user.contains("유나가 앉아 있다."));
    assert!(bundle.user.contains("The opening has already been shown."));
    assert!(bundle.system.contains("Do not restart the opening."));
}

#[test]
fn selected_numbered_choice_is_included_in_next_prompt() {
    let (pack, state, recent) = fixture(false);
    let choice = story_model::Choice {
        id: "ask-why".to_string(),
        label: "왜 떠났는지 조심스럽게 묻는다".to_string(),
        intent: "유나의 부재 이유를 묻는다".to_string(),
        tone: Some("careful".to_string()),
        target: Some(story_model::CharacterId("yuna".to_string())),
        risk: story_model::RiskLevel::Medium,
        hidden_requirements: vec![],
        preview: None,
    };
    let bundle = build_prompt_with_choice(&pack, &state, &recent, None, Some(&choice));
    assert!(bundle.user.contains("# Player selected choice"));
    assert!(bundle.user.contains("label: 왜 떠났는지 조심스럽게 묻는다"));
    assert!(bundle.user.contains("intent: 유나의 부재 이유를 묻는다"));
}

#[test]
fn prompt_carries_character_canon_event_rules_and_progress_state() {
    let (mut pack, mut state, recent) = fixture(false);
    let yuna = pack
        .characters
        .get_mut(&CharacterId("yuna".to_string()))
        .expect("yuna definition");
    yuna.personality = vec!["감정을 단정하지 않고 증거를 확인한다".to_string()];
    yuna.speech_style = "짧고 정확한 해요체".to_string();
    yuna.known_facts = vec!["카페 나무는 대학 시절 자주 오던 곳이다".to_string()];
    yuna.unknown_facts = vec!["플레이어의 반송된 편지".to_string()];
    yuna.secrets = vec!["보내지 못한 편지를 보관하고 있다".to_string()];
    pack.rules.events.insert(
        EventId("event.honest_exchange".to_string()),
        EventDef {
            id: EventId("event.honest_exchange".to_string()),
            trigger: ConditionExpr::FlagEquals {
                id: "apologized".to_string(),
                value: true,
            },
            priority: 10,
            effects: EventEffects::default(),
            allow_relationship_skip: false,
            route_lock: None,
        },
    );
    state.active_route = Some("reconciliation".to_string());
    state
        .occurred_events
        .insert("event.first_meeting".to_string());

    let bundle = build_prompt(&pack, &state, &recent, None);
    assert!(bundle.user.contains("감정을 단정하지 않고 증거를 확인한다"));
    assert!(bundle.user.contains("speech_style: 짧고 정확한 해요체"));
    assert!(
        bundle
            .user
            .contains("PRIVATE_unknown_facts: 플레이어의 반송된 편지")
    );
    assert!(
        bundle
            .user
            .contains("PRIVATE_secrets: 보내지 못한 편지를 보관하고 있다")
    );
    assert!(
        bundle
            .user
            .contains("event event.honest_exchange: trigger=")
    );
    assert!(bundle.user.contains("\"active_route\":\"reconciliation\""));
    assert!(bundle.user.contains("event.first_meeting"));
    assert!(
        bundle
            .user
            .contains("\"current_scene_id\":\"cafe-reunion\"")
    );
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

fn recap_history_entry(turn: u32, narration: &str) -> HistoryEntry {
    HistoryEntry {
        id: uuid::Uuid::nil(),
        turn,
        scene_id: "cafe-reunion".to_string(),
        created_at: chrono::DateTime::from_timestamp(1_700_000_000 + turn as i64, 0).unwrap(),
        narration: narration.to_string(),
        player_action: None,
        choices: vec![],
        applied_patch: StatePatch::default(),
        generation_id: uuid::Uuid::nil(),
    }
}

#[test]
fn earlier_turns_beyond_the_window_appear_as_a_recap() {
    // Regression guard for content regression: when the session is longer than
    // the recent window, the turns before it must come back as a compact recap
    // so the model keeps long-term continuity instead of repeating old beats.
    let (pack, state, _) = fixture(false);
    // 6 turns with a window of 2 ⇒ turns 1–4 are recap, turns 5–6 are recent.
    let recent = vec![
        recap_history_entry(1, "오래전 첫 만남의 장면이다."),
        recap_history_entry(2, "두 번째 장면, 비가 내렸다."),
        recap_history_entry(3, "세 번째 장면이다."),
        recap_history_entry(4, "네 번째 장면이다."),
        recap_history_entry(5, "다섯 번째 장면이다."),
        recap_history_entry(6, "여섯 번째 장면, 가장 최근이다."),
    ];
    let bundle = build_prompt(&pack, &state, &recent, None);

    assert!(
        bundle.user.contains("# Earlier story (recap"),
        "recap header must appear when history exceeds the window"
    );
    // Older turns are summarized in the recap.
    assert!(bundle.user.contains("오래전 첫 만남"));
    assert!(bundle.user.contains("두 번째 장면"));
    // Only the last two turns are rendered in full.
    assert!(bundle.user.contains("# Recent turns"));
    assert!(bundle.user.contains("여섯 번째 장면"));
    let raw_recent_section = bundle
        .user
        .split("# Recent turns")
        .nth(1)
        .unwrap_or_default()
        .split("# Recent arc")
        .next()
        .unwrap_or_default();
    assert!(raw_recent_section.contains("다섯 번째 장면"));
    assert!(raw_recent_section.contains("여섯 번째 장면"));
    assert!(!raw_recent_section.contains("네 번째 장면"));
    // The raw recent section must NOT reproduce the oldest turn.
    let recent_section = bundle
        .user
        .split("# Recent turns")
        .nth(1)
        .unwrap_or_default();
    assert!(!recent_section.contains("오래전 첫 만남"));
}

#[test]
fn short_session_has_no_recap_section() {
    // Sessions within the recent window must stay byte-identical (no recap
    // header), so the golden snapshot and short-turn prompts are unaffected.
    let (pack, state, recent) = fixture(false);
    let bundle = build_prompt(&pack, &state, &recent, None);
    assert!(!bundle.user.contains("# Earlier story"));
}

#[test]
fn recent_arc_renders_from_stored_summaries() {
    let (pack, mut state, _) = fixture(false);
    state.recent_summaries.extend([
        "유나가 경계하며 한 발 물러섰다".to_string(),
        "유나가 창밖만 바라본다".to_string(),
    ]);
    // Same length as history ⇒ aligned ⇒ stored digests are used verbatim.
    let recent = vec![
        recap_history_entry(1, "무시할 서사 1."),
        recap_history_entry(2, "무시할 서사 2."),
    ];
    let bundle = build_prompt(&pack, &state, &recent, None);
    assert!(bundle.user.contains("# Recent arc"));
    assert!(bundle.user.contains("유나가 경계하며 한 발 물러섰다"));
}

#[test]
fn repetition_note_fires_when_a_motif_is_shared() {
    let (pack, state, _) = fixture(false);
    // The first sentences are unrelated. The repeated motifs only occur in
    // later prose, which the former digest/first-clause scan could not see.
    let recent = vec![
        recap_history_entry(1, "문이 닫혔다. 유나는 시계를 보고 창밖으로 시선을 돌렸다."),
        recap_history_entry(
            2,
            "재즈가 멎었다. 시계 초침 아래에서 창밖의 빛과 시선을 피했다.",
        ),
        recap_history_entry(
            3,
            "주문표가 놓였다. 손가락이 시계를 두드리고 시선은 창밖에 머문다.",
        ),
    ];
    let bundle = build_prompt(&pack, &state, &recent, None);
    assert!(
        bundle.user.contains("# 반복 감지"),
        "expected a repetition note: {}",
        bundle.user
    );
    assert!(bundle.user.contains("시계"));
    assert!(bundle.user.contains("창밖"));
    assert!(bundle.user.contains("시선"));
}

#[test]
fn no_repetition_note_when_each_turn_differs() {
    let (pack, state, _) = fixture(false);
    let recent = vec![
        recap_history_entry(1, "유나가 웃었다."),
        recap_history_entry(2, "편지가 발견됐다."),
    ];
    let bundle = build_prompt(&pack, &state, &recent, None);
    assert!(!bundle.user.contains("# 반복 감지"));
}

#[test]
fn anti_repeat_phrase_block_is_gone() {
    // The surface-level # Anti-repeat list was replaced by the richer
    // # Recent arc + # 반복 감지 blocks.
    let (pack, state, recent) = fixture(false);
    let bundle = build_prompt(&pack, &state, &recent, None);
    assert!(!bundle.user.contains("# Anti-repeat"));
}

#[test]
fn empty_history_has_no_recent_arc_block() {
    let (pack, state, _) = fixture(false);
    let bundle = build_prompt(&pack, &state, &[], None);
    assert!(!bundle.user.contains("# Recent arc"));
    assert!(!bundle.user.contains("# 반복 감지"));
}
