//! Golden JSON fixtures and serde/version policy tests (W2.5).
//!
//! The golden fixtures under `fixtures/golden/` are committed artifacts; the
//! `regenerate_golden_fixtures` test (ignored) rewrites them from the same
//! sample data so they can be refreshed deterministically when the schema
//! changes.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use story_model::{
    ActionType, CharacterId, CharacterState, Choice, ChoiceRef, ConditionExpr, Emotion, GameState,
    GeneratedTurn, GenerationWarning, LocationId, MemoryEntry, MemoryKind, ParsedAction,
    RelationshipStage, RiskLevel, SAVE_SCHEMA_VERSION, StatePatch, TimeSlot, TokenUsage,
    TurnSummary,
};

// ---------------------------------------------------------------------------
// Sample builders (self-contained so the generator is reproducible).
// ---------------------------------------------------------------------------

fn sample_choice(id: &str) -> Choice {
    Choice {
        id: id.to_string(),
        label: format!("Label for {id}"),
        intent: "comfort".to_string(),
        tone: Some("gentle".to_string()),
        target: Some(CharacterId("c-namu".to_string())),
        risk: RiskLevel::Medium,
        hidden_requirements: vec![
            ConditionExpr::AffectionAtLeast {
                character: CharacterId("c-namu".to_string()),
                value: 20,
            },
            ConditionExpr::FlagEquals {
                id: "met_namu".to_string(),
                value: true,
            },
        ],
        preview: Some("She might smile.".to_string()),
    }
}

fn sample_patch() -> StatePatch {
    let mut patch = StatePatch::default();
    patch
        .affection_delta
        .insert(CharacterId("c-namu".to_string()), 6);
    patch
        .trust_delta
        .insert(CharacterId("c-namu".to_string()), 3);
    patch.set_flags.insert("f-brave".to_string(), true);
    patch.add_events.insert("e-rooftop".to_string());
    patch.next_location = Some(LocationId("loc-roof".to_string()));
    patch.next_time_slot = Some(TimeSlot::Evening);
    patch.proposed_route = Some("route-roof".to_string());
    patch
}

fn sample_turn() -> GeneratedTurn {
    GeneratedTurn {
        generation_id: uuid::Uuid::nil(),
        narration: "Wind tugs at your uniform on the rooftop.".to_string(),
        choices: vec![sample_choice("ch-stay"), sample_choice("ch-leave")],
        proposed_patch: sample_patch(),
        accepted_patch: sample_patch(),
        summary: TurnSummary {
            facts: vec!["reached the rooftop".to_string()],
            unresolved: vec!["Nam-u hasn't answered".to_string()],
            relationship_changes: vec!["+6 affection Namu".to_string()],
            knowledge_changes: vec!["rooftop exists".to_string()],
            interpretation: Some("The player chose to stay.".to_string()),
            address_form_changes: Vec::new(),
            important_player_actions: vec!["stay_with_namu".to_string()],
            source_history_ids: vec![uuid::Uuid::nil()],
        },
        warnings: vec![GenerationWarning {
            code: "fallback_parser".to_string(),
            detail: "Used rule-based parser; confidence below threshold.".to_string(),
        }],
        usage: Some(TokenUsage {
            prompt_tokens: 800,
            completion_tokens: 200,
            total_tokens: 1000,
        }),
    }
}

fn sample_game_state() -> GameState {
    let mut characters = BTreeMap::new();
    let mut known_facts = BTreeSet::new();
    known_facts.insert("player_is_transfer".to_string());
    characters.insert(
        CharacterId("c-namu".to_string()),
        CharacterState {
            affection: 25,
            trust: 10,
            tension: 3,
            relationship_stage: RelationshipStage::Friend,
            current_emotion: Emotion::Surprised,
            known_facts,
            address_form_overrides: BTreeMap::new(),
        },
    );

    let mut flags = BTreeMap::new();
    flags.insert("met_namu".to_string(), true);
    let mut numbers = BTreeMap::new();
    numbers.insert("score".to_string(), 7);
    let mut strings = BTreeMap::new();
    strings.insert("address_form".to_string(), "transfer-student".to_string());
    let mut inventory = BTreeMap::new();
    inventory.insert("i-letter".to_string(), 1);
    let mut occurred = BTreeSet::new();
    occurred.insert("e-first-meeting".to_string());

    let mut recent = VecDeque::new();
    recent.push_back("Met Namu in the hallway.".to_string());
    recent.push_back("Walked to the rooftop.".to_string());

    GameState {
        save_schema_version: SAVE_SCHEMA_VERSION,
        story_id: "demo-story".to_string(),
        story_version: semver::Version::new(0, 1, 0),
        pack_schema_version: semver::Version::new(1, 0, 0),
        session_id: uuid::Uuid::nil(),
        current_chapter: "ch-1".to_string(),
        current_scene_id: "sc-roof".to_string(),
        turn: 6,
        location: LocationId("loc-roof".to_string()),
        time_slot: TimeSlot::Evening,
        active_route: Some("route-main".to_string()),
        characters,
        flags,
        numbers,
        strings,
        inventory,
        occurred_events: occurred,
        recent_summaries: recent,
        memories: vec![MemoryEntry {
            id: uuid::Uuid::nil(),
            kind: MemoryKind::FirstMeeting,
            facts: Vec::new(),
            interpretation: Some("First meeting with Namu.".to_string()),
            participants: BTreeSet::from([CharacterId("c-namu".to_string())]),
            location: Some(LocationId("loc-hallway".to_string())),
            importance: 8,
            unresolved: false,
            source_history_ids: Vec::new(),
            created_turn: 1,
            last_recalled_turn: Some(6),
        }],
        history: Vec::new(),
        last_choice: Some(ChoiceRef {
            choice_id: "ch-stay".to_string(),
            label: "Stay with Namu".to_string(),
            turn: 5,
        }),
        revision: 7,
        stable_generation_id: Some(uuid::Uuid::nil()),
    }
}

/// Rewrite the committed golden fixtures from the sample builders above.
///
/// Intentionally `#[ignore]` — run on demand with
/// `cargo test -p story-model regenerate_golden_fixtures -- --ignored` whenever
/// the schema changes, then commit the refreshed JSON.
#[test]
#[ignore]
fn regenerate_golden_fixtures() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/golden");
    std::fs::create_dir_all(dir).expect("create golden dir");

    let state = sample_game_state();
    let state_json = serde_json::to_string_pretty(&state).expect("serialize state");
    std::fs::write(format!("{dir}/gamestate_v1.json"), state_json)
        .expect("write gamestate_v1.json");

    let turn = sample_turn();
    let turn_json = serde_json::to_string_pretty(&turn).expect("serialize turn");
    std::fs::write(format!("{dir}/generated_turn_v1.json"), turn_json)
        .expect("write generated_turn_v1.json");
}

// ---------------------------------------------------------------------------
// Version / serde policy tests
// ---------------------------------------------------------------------------

const GAMESTATE_GOLDEN: &str = include_str!("fixtures/golden/gamestate_v1.json");
const GENERATED_TURN_GOLDEN: &str = include_str!("fixtures/golden/generated_turn_v1.json");

#[test]
fn golden_gamestate_loads_with_save_schema_version_1() {
    let state: GameState = serde_json::from_str(GAMESTATE_GOLDEN).expect("golden loads");
    assert_eq!(state.save_schema_version, 1);
    assert_eq!(state.save_schema_version, SAVE_SCHEMA_VERSION);
    assert_eq!(state.story_id, "demo-story");
    // semver fields deserialize from string form.
    assert_eq!(state.story_version, semver::Version::new(0, 1, 0));
    assert_eq!(state.pack_schema_version, semver::Version::new(1, 0, 0));
    assert!(
        state
            .characters
            .contains_key(&CharacterId("c-namu".to_string()))
    );
}

#[test]
fn golden_generated_turn_loads() {
    let turn: GeneratedTurn =
        serde_json::from_str(GENERATED_TURN_GOLDEN).expect("golden turn loads");
    assert!(!turn.narration.is_empty());
    assert_eq!(turn.choices.len(), 2);
    assert!(!turn.accepted_patch.is_empty());
    assert_eq!(turn.usage.unwrap().total_tokens, 1000);
}

#[test]
fn semver_fields_serialize_as_strings() {
    let state = sample_game_state();
    let json = serde_json::to_string(&state).unwrap();
    // semver::Version serializes as a JSON string, e.g. "0.1.0".
    assert!(
        json.contains("\"story_version\":\"0.1.0\""),
        "expected story_version as string in: {json}"
    );
    assert!(
        json.contains("\"pack_schema_version\":\"1.0.0\""),
        "expected pack_schema_version as string in: {json}"
    );
}

#[test]
fn gamestate_is_forward_tolerant_to_unknown_future_fields() {
    // A save written by a FUTURE schema may carry fields this binary does not
    // know. Because GameState does NOT deny_unknown_fields, loading must
    // succeed and the unknown field is silently dropped.
    let mut json: serde_json::Value =
        serde_json::from_str(GAMESTATE_GOLDEN).expect("parse golden as Value");
    let obj = json.as_object_mut().expect("golden is an object");
    obj.insert(
        "future_field_v2".to_string(),
        serde_json::Value::String("from-the-year-20XX".to_string()),
    );
    let rewritten = serde_json::to_string(&json).unwrap();
    let state: GameState =
        serde_json::from_str(&rewritten).expect("unknown future field must not break loading");
    assert_eq!(state.save_schema_version, 1);
}

#[test]
fn gamestate_is_forward_tolerant_to_missing_optional_fields() {
    // A save from an older toolchain may omit Option fields; serde loads them
    // as None. This mirrors an older save loading into a newer binary that
    // added an optional field.
    let mut json: serde_json::Value =
        serde_json::from_str(GAMESTATE_GOLDEN).expect("parse golden as Value");
    let obj = json.as_object_mut().expect("golden is an object");
    obj.remove("active_route");
    obj.remove("last_choice");
    obj.remove("stable_generation_id");
    let rewritten = serde_json::to_string(&json).unwrap();
    let state: GameState =
        serde_json::from_str(&rewritten).expect("missing optional fields must not break loading");
    assert_eq!(state.active_route, None);
    assert!(state.last_choice.is_none());
    assert_eq!(state.stable_generation_id, None);
}

#[test]
fn state_patch_with_unknown_field_is_rejected() {
    // Re-asserted here (alongside the golden policy set) so the version policy
    // is documented in one place: StatePatch is the strict, closed contract.
    let json = r#"{
        "affection_delta": {},
        "trust_delta": {},
        "tension_delta": {},
        "relationship_transitions": {},
        "set_flags": {},
        "number_delta": {},
        "set_strings": {},
        "inventory_delta": {},
        "add_events": [],
        "next_location": null,
        "next_time_slot": null,
        "proposed_route": null,
        "unknown_key": 1
    }"#;
    let result: Result<StatePatch, serde_json::Error> = serde_json::from_str(json);
    assert!(result.is_err(), "StatePatch must reject unknown fields");
}

// A couple of uses of parsed action / emotion that exercise the broader wire
// format alongside the golden, ensuring the documented enum strings load.
#[test]
fn parsed_action_snake_case_action_type_loads_from_golden_style_json() {
    let json = r#"{
        "action_type": "general_action",
        "target": "c-namu",
        "intent": "wait",
        "tone": null,
        "content": "Wait quietly.",
        "original_text": "wait",
        "confidence": 0.42
    }"#;
    let pa: ParsedAction = serde_json::from_str(json).expect("loads");
    assert_eq!(pa.action_type, ActionType::GeneralAction);
    assert_eq!(pa.target.as_ref().unwrap().0, "c-namu");
    assert_eq!(pa.confidence, Some(0.42));
}
