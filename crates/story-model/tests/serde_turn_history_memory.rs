//! Round-trip tests for the generation/history/memory keystone types and the
//! full [`GameState`] save-state root.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use story_model::{
    ActionType, CharacterId, CharacterState, Choice, ChoiceRef, ConditionExpr, Emotion, GameState,
    GeneratedTurn, GenerationWarning, HistoryEntry, LocationId, MemoryEntry, MemoryFact,
    MemoryKind, ParsedAction, RelationshipStage, RiskLevel, SAVE_SCHEMA_VERSION, StatePatch,
    TimeSlot, TokenUsage, TurnSummary,
};

/// Serialize -> deserialize -> re-serialize and compare JSON. Lets us verify
/// round-trip fidelity without requiring `PartialEq` on the type under test.
fn round_trip_stable<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let s1 = serde_json::to_string_pretty(value).expect("serialize (first)");
    let back: T = serde_json::from_str(&s1).expect("deserialize");
    let s2 = serde_json::to_string_pretty(&back).expect("serialize (second)");
    assert_eq!(s1, s2, "round-trip changed the JSON form");
}

fn sample_choice(id: &str) -> Choice {
    Choice {
        id: id.to_string(),
        label: format!("Label for {id}"),
        intent: "intent".to_string(),
        tone: Some("playful".to_string()),
        target: Some(CharacterId("c-namu".to_string())),
        risk: RiskLevel::Low,
        hidden_requirements: vec![ConditionExpr::FlagEquals {
            id: "met_namu".to_string(),
            value: true,
        }],
        preview: Some("preview text".to_string()),
    }
}

fn sample_patch() -> StatePatch {
    let mut patch = StatePatch::default();
    patch
        .affection_delta
        .insert(CharacterId("c-namu".to_string()), 8);
    patch.set_flags.insert("f-brave".to_string(), true);
    patch.next_time_slot = Some(TimeSlot::Evening);
    patch
}

#[test]
fn parsed_action_round_trips() {
    let pa = ParsedAction {
        action_type: ActionType::Dialogue,
        target: Some(CharacterId("c-namu".to_string())),
        intent: Some("comfort".to_string()),
        tone: Some("gentle".to_string()),
        content: "Hey, it'll be okay.".to_string(),
        original_text: "hey, it'll be okay".to_string(),
        confidence: Some(0.91),
    };
    round_trip_stable(&pa);
}

#[test]
fn turn_summary_round_trips() {
    let ts = TurnSummary {
        facts: vec!["Nam-u smiled.".to_string()],
        unresolved: vec!["The letter is unread.".to_string()],
        relationship_changes: vec!["+trust Namu".to_string()],
        knowledge_changes: vec!["learned about the rooftop".to_string()],
        interpretation: Some("The player chose to comfort Namu.".to_string()),
        address_form_changes: vec!["switched to first name".to_string()],
        important_player_actions: vec!["comfort_namu".to_string()],
        source_history_ids: vec![
            uuid::Uuid::nil(),
            uuid::Uuid::parse_str("123e4567-e89b-12d3-a456-426614174000").unwrap(),
        ],
    };
    round_trip_stable(&ts);
}

#[test]
fn generated_turn_round_trips_fully_populated() {
    let turn = GeneratedTurn {
        generation_id: uuid::Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
        narration: "The rooftop wind tugs at your uniform.".to_string(),
        choices: vec![sample_choice("ch-a"), sample_choice("ch-b")],
        proposed_patch: sample_patch(),
        accepted_patch: sample_patch(),
        summary: TurnSummary {
            facts: vec!["reached rooftop".to_string()],
            unresolved: Vec::new(),
            relationship_changes: Vec::new(),
            knowledge_changes: Vec::new(),
            interpretation: None,
            address_form_changes: Vec::new(),
            important_player_actions: Vec::new(),
            source_history_ids: Vec::new(),
        },
        warnings: vec![GenerationWarning {
            code: "retry_budget_used".to_string(),
            detail: "Retried once after a network blip.".to_string(),
        }],
        usage: Some(TokenUsage {
            prompt_tokens: 1234,
            completion_tokens: 567,
            total_tokens: 1801,
        }),
    };
    round_trip_stable(&turn);

    // Spot-check field-level survival.
    let json = serde_json::to_string(&turn).unwrap();
    let back: GeneratedTurn = serde_json::from_str(&json).unwrap();
    assert_eq!(back.choices.len(), 2);
    assert_eq!(back.warnings[0].code, "retry_budget_used");
    let usage = back.usage.expect("usage present");
    assert_eq!(usage.total_tokens, 1801);
    assert_eq!(
        usage.prompt_tokens + usage.completion_tokens,
        usage.total_tokens
    );
    // The embedded patch survives with deny_unknown_fields intact on the
    // accepted patch.
    assert!(!back.accepted_patch.is_empty());
}

#[test]
fn history_entry_round_trips_fully_populated() {
    let entry = HistoryEntry {
        id: uuid::Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap(),
        turn: 42,
        scene_id: "sc-roof".to_string(),
        created_at: chrono::DateTime::parse_from_rfc3339("2026-07-31T12:34:56Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        narration: "She looked out over the city.".to_string(),
        player_action: Some(ParsedAction {
            action_type: ActionType::Dialogue,
            target: Some(CharacterId("c-namu".to_string())),
            intent: Some("comfort".to_string()),
            tone: None,
            content: "I'm here.".to_string(),
            original_text: "i'm here".to_string(),
            confidence: Some(0.9),
        }),
        choices: vec![sample_choice("ch-a")],
        applied_patch: sample_patch(),
        generation_id: uuid::Uuid::nil(),
    };
    round_trip_stable(&entry);
    let json = serde_json::to_string(&entry).unwrap();
    let back: HistoryEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(back.turn, 42);
    assert_eq!(back.scene_id, "sc-roof");
    // DateTime<Utc> is PartialEq; compare directly to avoid magic timestamps.
    assert_eq!(back.created_at, entry.created_at);
}

#[test]
fn memory_fact_and_entry_round_trip() {
    let mut known_by = BTreeSet::new();
    known_by.insert(CharacterId("c-namu".to_string()));
    let fact = MemoryFact {
        subject: "player".to_string(),
        predicate: "promised".to_string(),
        object: "to keep the rooftop secret".to_string(),
        known_by,
        certainty: 0.95,
    };
    round_trip_stable(&fact);

    let mut participants = BTreeSet::new();
    participants.insert(CharacterId("c-namu".to_string()));
    let memory = MemoryEntry {
        id: uuid::Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap(),
        kind: MemoryKind::Promise,
        facts: vec![fact],
        interpretation: Some("A binding promise on the rooftop.".to_string()),
        participants,
        location: Some(LocationId("loc-roof".to_string())),
        importance: 9,
        unresolved: true,
        source_history_ids: vec![
            uuid::Uuid::nil(),
            uuid::Uuid::parse_str("123e4567-e89b-12d3-a456-426614174000").unwrap(),
        ],
        created_turn: 42,
        last_recalled_turn: Some(50),
    };
    round_trip_stable(&memory);
    let json = serde_json::to_string(&memory).unwrap();
    let back: MemoryEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(back.kind, MemoryKind::Promise);
    assert_eq!(back.facts.len(), 1);
    assert_eq!(back.facts[0].certainty, 0.95);
    assert!(back.unresolved);
    assert_eq!(back.last_recalled_turn, Some(50));
}

#[test]
fn game_state_round_trips_and_carries_save_schema_version() {
    let state = sample_game_state();
    round_trip_stable(&state);
    let json = serde_json::to_string(&state).unwrap();
    let back: GameState = serde_json::from_str(&json).unwrap();
    assert_eq!(back.save_schema_version, SAVE_SCHEMA_VERSION);
    assert_eq!(back.story_id, "demo-story");
    assert_eq!(back.location.0, "loc-classroom");
    assert_eq!(back.time_slot, TimeSlot::Morning);
    assert!(
        back.characters
            .contains_key(&CharacterId("c-namu".to_string()))
    );
    assert_eq!(back.revision, 7);
}

/// Build a representative, fully-populated [`GameState`]. Also reused by W2.5
/// golden-fixture and version-policy tests.
pub fn sample_game_state() -> GameState {
    let mut characters = BTreeMap::new();
    let mut known_facts = BTreeSet::new();
    known_facts.insert("player_is_transfer".to_string());
    characters.insert(
        CharacterId("c-namu".to_string()),
        CharacterState {
            affection: 30,
            trust: 15,
            tension: 2,
            relationship_stage: RelationshipStage::Friend,
            current_emotion: Emotion::Happy,
            known_facts,
            address_form_overrides: BTreeMap::new(),
        },
    );

    let mut flags = BTreeMap::new();
    flags.insert("met_namu".to_string(), true);
    let mut numbers = BTreeMap::new();
    numbers.insert("score".to_string(), 12);
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
        session_id: uuid::Uuid::parse_str("44444444-4444-4444-4444-444444444444").unwrap(),
        current_chapter: "ch-1".to_string(),
        current_scene_id: "sc-hallway".to_string(),
        turn: 6,
        location: LocationId("loc-classroom".to_string()),
        time_slot: TimeSlot::Morning,
        active_route: Some("route-main".to_string()),
        characters,
        flags,
        numbers,
        strings,
        inventory,
        occurred_events: occurred,
        recent_summaries: recent,
        memories: Vec::new(),
        history: Vec::new(),
        last_choice: Some(ChoiceRef {
            choice_id: "ch-greet".to_string(),
            label: "Greet Namu".to_string(),
            turn: 5,
        }),
        revision: 7,
        stable_generation_id: Some(uuid::Uuid::nil()),
    }
}
