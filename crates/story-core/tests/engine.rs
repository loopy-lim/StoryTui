//! End-to-end integration tests wiring validate → apply → events → ending /
//! route through [`story_core::engine::commit_turn`] (W3.5).
//!
//! Builds a hand-authored [`StoryRules`] + [`GameState`] (no Pack loader, no
//! LLM) and exercises a realistic turn that:
//!
//! - applies in-bounds affection/trust/tension deltas
//! - sets a flag and triggers an event whose `trigger` becomes true
//! - the event adds a follow-up event and forces a relationship stage
//! - the model's `proposed_route` is accepted because its `enter_condition`
//!   is now satisfied
//! - a deterministic ending triggers because a `TurnAtLeast` condition holds

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use story_core::engine::commit_turn;
use story_core::reducer::ApplyResult;
use story_model::{
    CharacterId, CharacterState, ConditionExpr, Emotion, EndingDef, EventDef, EventEffects,
    EventId, GameState, LocationDef, LocationId, RelationshipStage, RouteDef, SAVE_SCHEMA_VERSION,
    StatePatch, StoryRules, TimeSlot,
};

const GEN_ID: uuid::Uuid = uuid::Uuid::nil();

fn namu() -> CharacterId {
    CharacterId("c-namu".to_string())
}

fn state_with() -> GameState {
    let mut chars = BTreeMap::new();
    chars.insert(
        namu(),
        CharacterState {
            affection: 18,
            trust: 4,
            tension: 0,
            relationship_stage: RelationshipStage::Acquaintance,
            current_emotion: Emotion::Neutral,
            known_facts: BTreeSet::new(),
            address_form_overrides: BTreeMap::new(),
        },
    );
    GameState {
        save_schema_version: SAVE_SCHEMA_VERSION,
        story_id: "demo".to_string(),
        story_version: semver::Version::new(0, 1, 0),
        pack_schema_version: semver::Version::new(1, 0, 0),
        session_id: uuid::Uuid::nil(),
        current_chapter: "ch-1".to_string(),
        current_scene_id: "sc-1".to_string(),
        turn: 9,
        location: LocationId("loc-classroom".to_string()),
        time_slot: TimeSlot::Morning,
        active_route: None,
        characters: chars,
        flags: BTreeMap::new(),
        numbers: BTreeMap::new(),
        strings: BTreeMap::new(),
        inventory: BTreeMap::new(),
        occurred_events: BTreeSet::new(),
        recent_summaries: VecDeque::new(),
        memories: Vec::new(),
        history: Vec::new(),
        last_choice: None,
        revision: 1,
        stable_generation_id: None,
        lorebook_activations: Vec::new(),
    }
}

fn rules_with() -> StoryRules {
    let mut r = StoryRules::minimal();
    let mut adj = BTreeSet::new();
    adj.insert(LocationId("loc-hall".to_string()));
    r.locations.insert(
        LocationId("loc-classroom".to_string()),
        LocationDef {
            id: LocationId("loc-classroom".to_string()),
            display_name: "Classroom".to_string(),
            adjacent: adj,
        },
    );
    r.locations.insert(
        LocationId("loc-hall".to_string()),
        LocationDef {
            id: LocationId("loc-hall".to_string()),
            display_name: "Hall".to_string(),
            adjacent: BTreeSet::from([LocationId("loc-classroom".to_string())]),
        },
    );
    r.allowed_flags.extend([
        "met_namu".to_string(),
        "f-brave".to_string(),
        "f-confessed".to_string(),
    ]);
    r.allowed_items.insert("i-letter".to_string());

    // Route accepts when affection >= 20.
    r.routes.insert(
        "route-roof".to_string(),
        RouteDef {
            id: "route-roof".to_string(),
            enter_condition: ConditionExpr::AffectionAtLeast {
                character: namu(),
                value: 20,
            },
            premise: "The rooftop calls.".to_string(),
        },
    );

    // Ending triggers once the player has confessed and reached turn 9.
    r.endings.insert(
        "end-confession".to_string(),
        EndingDef {
            id: "end-confession".to_string(),
            condition: ConditionExpr::All {
                children: vec![
                    ConditionExpr::FlagEquals {
                        id: "f-confessed".to_string(),
                        value: true,
                    },
                    ConditionExpr::TurnAtLeast { value: 9 },
                ],
            },
            label: "Confession ending".to_string(),
        },
    );

    // Event fires when f-confessed becomes true; it forces the relationship to
    // RomanticInterest and adds a follow-up event.
    r.events.insert(
        EventId("e-confession".to_string()),
        EventDef {
            id: EventId("e-confession".to_string()),
            trigger: ConditionExpr::FlagEquals {
                id: "f-confessed".to_string(),
                value: true,
            },
            priority: 10,
            effects: EventEffects {
                affection_delta: BTreeMap::from([(namu(), 2)]),
                add_events: BTreeSet::from(["e-aftermath".to_string()]),
                force_relationship: BTreeMap::from([(namu(), RelationshipStage::RomanticInterest)]),
                ..EventEffects::default()
            },
            allow_relationship_skip: true, // authorizes Stranger→RomanticInterest etc.
            route_lock: None,
        },
    );
    // Follow-up event; will not fire on the first turn because e-aftermath is
    // added to occurred_events (not to add_events of the patch).
    r.events.insert(
        EventId("e-aftermath".to_string()),
        EventDef {
            id: EventId("e-aftermath".to_string()),
            trigger: ConditionExpr::FlagEquals {
                id: "f-confessed".to_string(),
                value: true,
            },
            priority: 1,
            effects: EventEffects {
                set_flags: BTreeMap::from([("f-brave".to_string(), true)]),
                ..EventEffects::default()
            },
            allow_relationship_skip: false,
            route_lock: None,
        },
    );
    r
}

fn confessed_patch() -> StatePatch {
    // +2 affection (18 → 20), set f-confessed=true, fire e-confession (whose
    // trigger checks f-confessed=true after the patch), propose route-roof.
    let mut p = StatePatch::default();
    p.affection_delta.insert(namu(), 2);
    p.set_flags.insert("f-confessed".to_string(), true);
    p.add_events.insert("e-confession".to_string());
    p.proposed_route = Some("route-roof".to_string());
    p
}

#[test]
fn commit_turn_threads_validate_apply_event_route_ending() {
    let state = state_with();
    let rules = rules_with();
    let patch = confessed_patch();

    let result: ApplyResult = commit_turn(&state, &rules, &patch, GEN_ID).expect("turn commits");

    // Event e-confession fired (trigger f-confessed=true after the patch).
    assert_eq!(
        result.fired_events,
        vec![EventId("e-confession".to_string())]
    );

    // Affection: 18 (initial) + 2 (patch) + 2 (event effect) = 22.
    assert_eq!(result.state.characters.get(&namu()).unwrap().affection, 22);

    // Event effect forced the relationship to RomanticInterest.
    assert_eq!(
        result
            .state
            .characters
            .get(&namu())
            .unwrap()
            .relationship_stage,
        RelationshipStage::RomanticInterest
    );

    // The event queued a follow-up event into occurred_events but it did NOT
    // fire this turn (its trigger check happened before it was added).
    assert!(
        result.state.occurred_events.contains("e-aftermath"),
        "event effects add_events should record the follow-up"
    );
    assert!(
        !result
            .fired_events
            .contains(&EventId("e-aftermath".to_string())),
        "follow-up event must not fire on the same turn it is added"
    );

    // proposed_route accepted because affection now >= 20 (true at confirm time).
    assert_eq!(result.state.active_route.as_deref(), Some("route-roof"));

    // Ending triggered from EndingDef.condition (f-confessed && turn >= 9).
    assert_eq!(result.ending.as_deref(), Some("end-confession"));

    // revision bumped exactly once; turn untouched.
    assert_eq!(result.state.revision, state.revision + 1);
    assert_eq!(result.state.turn, state.turn);
}

#[test]
fn commit_turn_rejects_invalid_patch_atomically() {
    let state = state_with();
    let rules = rules_with();
    let mut patch = StatePatch::default();
    patch.affection_delta.insert(namu(), 99); // out of -3..=3 range

    let before = serde_json::to_vec(&state).unwrap();
    let err = commit_turn(&state, &rules, &patch, GEN_ID).unwrap_err();
    let after = serde_json::to_vec(&state).unwrap();
    assert!(err.reason_code().is_some());
    assert_eq!(
        before, after,
        "rejected patch leaves caller state untouched"
    );
}

#[test]
fn commit_turn_is_deterministic_across_repeated_calls() {
    let state = state_with();
    let rules = rules_with();
    let patch = confessed_patch();
    let first = commit_turn(&state, &rules, &patch, GEN_ID).expect("commits");

    // Redact random change-log ids so we can compare structurally.
    let mut first_log = first.change_log.clone();
    for e in &mut first_log {
        e.id = uuid::Uuid::nil();
    }
    let first_key = (
        serde_json::to_string(&first.state).unwrap(),
        serde_json::to_string(&first_log).unwrap(),
        first.fired_events.clone(),
        first.ending.clone(),
    );

    for i in 0..50 {
        let again = commit_turn(&state, &rules, &patch, GEN_ID).expect("commits");
        let mut again_log = again.change_log.clone();
        for e in &mut again_log {
            e.id = uuid::Uuid::nil();
        }
        let key = (
            serde_json::to_string(&again.state).unwrap(),
            serde_json::to_string(&again_log).unwrap(),
            again.fired_events.clone(),
            again.ending.clone(),
        );
        assert_eq!(key, first_key, "determinism broken at iter {i}");
    }
}
