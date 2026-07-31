//! Atomic-reducer + state-machine invariants (W3.4 / ST-008 / ST-010).
//!
//! Each of the four hard invariants has a dedicated test that would fail if
//! the invariant were violated:
//!
//! - atomic rollback: a failed patch leaves the caller's state byte-identical
//! - revision-exactly-once: success increments `revision` by exactly 1
//! - 100× determinism: same input → identical result
//! - route-confirm-only-when-true: `proposed_route` is accepted only when
//!   its `enter_condition` evaluates `true`
//! - ending-never-from-patch: the ending is computed from `EndingDef.condition`
//!   regardless of what the patch carries (the patch has no `ending` field)
//!
//! Plus a forbidden-relationship-transition test.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use story_core::error::PATCH_DELTA_OUT_OF_RANGE;
use story_core::reducer::{ApplyResult, apply_patch};
use story_model::{
    CharacterId, CharacterState, ConditionExpr, Emotion, EndingDef, EventDef, EventEffects,
    EventId, GameState, LocationDef, LocationId, RelationshipStage, RouteDef, SAVE_SCHEMA_VERSION,
    StatePatch, StoryRules,
};

const GEN_ID: uuid::Uuid = uuid::Uuid::nil();

fn hand_built_state() -> GameState {
    let mut chars = BTreeMap::new();
    chars.insert(
        CharacterId("c-namu".to_string()),
        CharacterState {
            affection: 25,
            trust: 10,
            tension: 3,
            relationship_stage: RelationshipStage::Friend,
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
        turn: 6,
        location: LocationId("loc-classroom".to_string()),
        time_slot: story_model::TimeSlot::Morning,
        active_route: None,
        characters: chars,
        flags: BTreeMap::from([("met_namu".to_string(), true)]),
        numbers: BTreeMap::from([("score".to_string(), 7)]),
        strings: BTreeMap::new(),
        inventory: BTreeMap::from([("i-letter".to_string(), 2)]),
        occurred_events: BTreeSet::new(),
        recent_summaries: VecDeque::new(),
        memories: Vec::new(),
        history: Vec::new(),
        last_choice: None,
        revision: 5,
        stable_generation_id: None,
    }
}

fn hand_built_rules() -> StoryRules {
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
    r.allowed_flags
        .extend(["met_namu", "f-brave"].map(String::from));
    r.allowed_items.insert("i-letter".to_string());
    r.endings.insert(
        "end-good".to_string(),
        EndingDef {
            id: "end-good".to_string(),
            condition: ConditionExpr::AffectionAtLeast {
                character: CharacterId("c-namu".to_string()),
                value: 100,
            },
            label: "Good".to_string(),
        },
    );
    r.routes.insert(
        "route-roof".to_string(),
        RouteDef {
            id: "route-roof".to_string(),
            enter_condition: ConditionExpr::AffectionAtLeast {
                character: CharacterId("c-namu".to_string()),
                value: 20,
            },
            premise: "The rooftop calls.".to_string(),
        },
    );
    r.events.insert(
        EventId("e-namu".to_string()),
        EventDef {
            id: EventId("e-namu".to_string()),
            trigger: ConditionExpr::FlagEquals {
                id: "met_namu".to_string(),
                value: true,
            },
            priority: 1,
            effects: EventEffects {
                affection_delta: BTreeMap::from([(CharacterId("c-namu".to_string()), 2)]),
                ..EventEffects::default()
            },
            allow_relationship_skip: false,
            route_lock: None,
        },
    );
    r
}

// ===========================================================================
// Invariant 1: atomic rollback (byte-identical original on failure)
// ===========================================================================

#[test]
fn failed_patch_leaves_caller_state_byte_identical() {
    let state = hand_built_state();
    let rules = hand_built_rules();
    // Build a patch that fails at the range stage: -4 is outside (-3..=3).
    let mut patch = StatePatch::default();
    patch
        .affection_delta
        .insert(CharacterId("c-namu".to_string()), -4);

    let before_bytes = serde_json::to_vec(&state).expect("serialize before");

    let err = apply_patch(&state, &rules, &patch, GEN_ID).unwrap_err();
    assert_eq!(err.reason_code(), Some(PATCH_DELTA_OUT_OF_RANGE));

    // The caller's state must be byte-identical after the failed apply.
    let after_bytes = serde_json::to_vec(&state).expect("serialize after");
    assert_eq!(
        before_bytes, after_bytes,
        "failed patch must leave the original state byte-identical"
    );
    assert_eq!(state.revision, 5, "revision must not change on failure");
}

// ===========================================================================
// Invariant 2: revision increments exactly once on success
// ===========================================================================

#[test]
fn success_increments_revision_exactly_once() {
    let state = hand_built_state();
    let rules = hand_built_rules();
    let before_revision = state.revision;

    let mut patch = StatePatch::default();
    patch
        .affection_delta
        .insert(CharacterId("c-namu".to_string()), 3);

    let result = apply_patch(&state, &rules, &patch, GEN_ID).expect("applies");
    assert_eq!(result.state.revision, before_revision + 1);

    // A second, independent apply from the SAME starting state must also bump
    // by exactly one — i.e. there is no hidden double-increment.
    let result2 = apply_patch(&state, &rules, &patch, GEN_ID).expect("applies");
    assert_eq!(result2.state.revision, before_revision + 1);
}

// ===========================================================================
// Invariant 3: 100× determinism
// ===========================================================================

#[test]
fn same_input_produces_identical_result_100_times() {
    let state = hand_built_state();
    let rules = hand_built_rules();

    let mut patch = StatePatch::default();
    patch
        .affection_delta
        .insert(CharacterId("c-namu".to_string()), 3);
    patch
        .trust_delta
        .insert(CharacterId("c-namu".to_string()), 1);
    patch.set_flags.insert("f-brave".to_string(), true);
    patch
        .next_location
        .replace(LocationId("loc-hall".to_string()));
    patch.add_events.insert("e-namu".to_string());
    patch.proposed_route.replace("route-roof".to_string());

    fn normalize(r: &ApplyResult) -> String {
        // Change-log entry ids are random UUIDs; to compare deterministically
        // we redact them before serializing.
        let mut redacted: Vec<story_model::StateChangeLog> = r.change_log.clone();
        for e in &mut redacted {
            e.id = uuid::Uuid::nil();
        }
        format!(
            "{:?}|{:?}|{:?}|{:?}",
            serde_json::to_string(&r.state).unwrap(),
            serde_json::to_string(&redacted).unwrap(),
            r.fired_events,
            r.ending,
        )
    }

    let first = apply_patch(&state, &rules, &patch, GEN_ID).expect("applies");
    let first_key = normalize(&first);
    for i in 0..100 {
        let again = apply_patch(&state, &rules, &patch, GEN_ID).expect("applies");
        let key = normalize(&again);
        assert_eq!(key, first_key, "determinism violated on iteration {i}");
    }
}

// ===========================================================================
// Invariant 4: route confirmed only when enter_condition is true
// ===========================================================================

#[test]
fn proposed_route_accepted_only_when_enter_condition_is_true() {
    let rules = hand_built_rules();
    // route-roof requires affection >= 20.

    // Case A: affection = 25 → enter_condition true → route accepted.
    let state_hi = hand_built_state();
    let patch_accept = StatePatch {
        proposed_route: Some("route-roof".to_string()),
        ..StatePatch::default()
    };
    let r_hi = apply_patch(&state_hi, &rules, &patch_accept, GEN_ID).expect("applies");
    assert_eq!(r_hi.state.active_route.as_deref(), Some("route-roof"));

    // Case B: affection = 5 → enter_condition false → proposal ignored,
    // active_route stays at its previous value (None here).
    let mut state_lo = hand_built_state();
    state_lo
        .characters
        .get_mut(&CharacterId("c-namu".to_string()))
        .unwrap()
        .affection = 5;
    let patch_reject = StatePatch {
        proposed_route: Some("route-roof".to_string()),
        ..StatePatch::default()
    };
    let r_lo = apply_patch(&state_lo, &rules, &patch_reject, GEN_ID).expect("applies");
    assert!(
        r_lo.state.active_route.is_none(),
        "unmet route proposal must not switch active_route; got {:?}",
        r_lo.state.active_route
    );
}

// ===========================================================================
// Invariant 5: ending never comes from the patch
// ===========================================================================

#[test]
fn ending_is_computed_from_condition_never_from_patch() {
    let mut rules = hand_built_rules();
    // Add an ending with an easily-triggered condition (turn >= 1).
    rules.endings.insert(
        "end-current".to_string(),
        EndingDef {
            id: "end-current".to_string(),
            condition: ConditionExpr::TurnAtLeast { value: 1 },
            label: "Current".to_string(),
        },
    );

    let state = hand_built_state();
    // The patch has NO ending field at all — prove the ending comes purely
    // from the rules + state. The reducer must surface `end-current`.
    let patch = StatePatch::default();
    let result = apply_patch(&state, &rules, &patch, GEN_ID).expect("applies");
    assert_eq!(result.ending.as_deref(), Some("end-current"));
}

#[test]
fn ending_none_when_no_condition_matches() {
    let rules = hand_built_rules(); // end-good needs affection >= 100
    let state = hand_built_state(); // affection = 25
    let patch = StatePatch::default();
    let result = apply_patch(&state, &rules, &patch, GEN_ID).expect("applies");
    assert!(result.ending.is_none());
}

// ===========================================================================
// Forbidden relationship transition
// ===========================================================================

#[test]
fn forbidden_relationship_transition_rejects_patch() {
    // Two-step transition: validator lets it through because the patch carries
    // a skip-granting candidate event, but the reducer's authoritative check
    // fires that event (its trigger is false) and returns ForbiddenTransition.
    let mut rules = hand_built_rules();
    rules.events.insert(
        EventId("e-skip-never-fires".to_string()),
        EventDef {
            id: EventId("e-skip-never-fires".to_string()),
            // Trigger is false because `nonexistent_flag` is absent (= false).
            trigger: ConditionExpr::FlagEquals {
                id: "nonexistent_flag".to_string(),
                value: true,
            },
            priority: 1,
            effects: EventEffects::default(),
            allow_relationship_skip: true,
            route_lock: None,
        },
    );
    // Allow the flag id so the reference check passes; the value stays absent.
    rules.allowed_flags.insert("nonexistent_flag".to_string());

    let state = hand_built_state(); // Friend (2)
    let patch = StatePatch {
        relationship_transitions: BTreeMap::from([(
            CharacterId("c-namu".to_string()),
            RelationshipStage::RomanticInterest, // 2 steps
        )]),
        add_events: BTreeSet::from(["e-skip-never-fires".to_string()]),
        ..StatePatch::default()
    };
    let err = apply_patch(&state, &rules, &patch, GEN_ID).unwrap_err();
    assert!(
        matches!(err, story_core::error::CoreError::ForbiddenTransition(_)),
        "expected reducer-side ForbiddenTransition, got {err:?}"
    );
    // The caller's state is still byte-identical (atomicity).
    assert_eq!(state.revision, 5);
}

// ===========================================================================
// Event firing + ordering
// ===========================================================================

#[test]
fn fired_events_apply_their_effects_and_are_recorded() {
    let state = hand_built_state(); // affection 25, has met_namu flag → e-namu fires
    let rules = hand_built_rules();
    let patch = StatePatch {
        add_events: BTreeSet::from(["e-namu".to_string()]),
        ..StatePatch::default()
    };
    let result = apply_patch(&state, &rules, &patch, GEN_ID).expect("applies");
    assert_eq!(result.fired_events, vec![EventId("e-namu".to_string())]);
    // e-namu grants +2 affection → 25 + 2 = 27.
    assert_eq!(
        result
            .state
            .characters
            .get(&CharacterId("c-namu".to_string()))
            .unwrap()
            .affection,
        27
    );
}

#[test]
fn equal_priority_events_ordered_by_priority_desc_then_id_asc() {
    let mut rules = hand_built_rules();
    // Three events that all fire (trigger met_namu=true). Equal priority.
    for id in ["e-aaa", "e-bbb", "e-ccc"] {
        rules.events.insert(
            EventId(id.to_string()),
            EventDef {
                id: EventId(id.to_string()),
                trigger: ConditionExpr::FlagEquals {
                    id: "met_namu".to_string(),
                    value: true,
                },
                priority: 5,
                effects: EventEffects::default(),
                allow_relationship_skip: false,
                route_lock: None,
            },
        );
    }
    // Add a higher-priority event that should sort first.
    rules.events.insert(
        EventId("e-king".to_string()),
        EventDef {
            id: EventId("e-king".to_string()),
            trigger: ConditionExpr::FlagEquals {
                id: "met_namu".to_string(),
                value: true,
            },
            priority: 99,
            effects: EventEffects::default(),
            allow_relationship_skip: false,
            route_lock: None,
        },
    );

    let state = hand_built_state();
    let patch = StatePatch {
        add_events: BTreeSet::from([
            "e-aaa".to_string(),
            "e-bbb".to_string(),
            "e-ccc".to_string(),
            "e-king".to_string(),
            "e-namu".to_string(),
        ]),
        ..StatePatch::default()
    };
    let result = apply_patch(&state, &rules, &patch, GEN_ID).expect("applies");
    assert_eq!(
        result.fired_events,
        vec![
            EventId("e-king".to_string()), // priority 99 first
            EventId("e-aaa".to_string()),  // priority 5, id asc
            EventId("e-bbb".to_string()),
            EventId("e-ccc".to_string()),
            EventId("e-namu".to_string()), // priority 1
        ]
    );
}

// ===========================================================================
// No narration text in change log
// ===========================================================================

#[test]
fn no_narration_text_appears_in_change_log() {
    let state = hand_built_state();
    let rules = hand_built_rules();
    let mut patch = StatePatch::default();
    patch
        .affection_delta
        .insert(CharacterId("c-namu".to_string()), 2);
    let result = apply_patch(&state, &rules, &patch, GEN_ID).expect("applies");
    for entry in &result.change_log {
        // Field paths are structural identifiers; they never carry prose.
        assert!(
            !entry.field_path.contains("narration"),
            "narration leaked into change log field_path: {}",
            entry.field_path
        );
        assert!(
            !entry.field_path.contains("summary"),
            "summary leaked into change log field_path: {}",
            entry.field_path
        );
    }
}

// ===========================================================================
// Event trust_delta / tension_delta reach the character stat (W5 fix)
//
// Regression: previously the Pack loader rewrote event trust/tension deltas
// into effects.number_delta under keys like "c-namu.trust", so at runtime they
// landed in state.numbers — never in state.characters[c].trust/.tension where
// TrustAtLeast/AffectionAtLeast actually read. These tests pin the typed path.
// ===========================================================================

use story_core::error::PATCH_STAT_OUT_OF_RANGE;
use story_core::eval::{EvalContext, evaluate};

fn rules_with_event(axis: &'static str, delta: i16) -> StoryRules {
    let mut r = hand_built_rules();
    let effects = match axis {
        "trust" => EventEffects {
            trust_delta: BTreeMap::from([(CharacterId("c-namu".to_string()), delta)]),
            ..EventEffects::default()
        },
        "tension" => EventEffects {
            tension_delta: BTreeMap::from([(CharacterId("c-namu".to_string()), delta)]),
            ..EventEffects::default()
        },
        _ => unreachable!("unknown axis {axis}"),
    };
    r.events.insert(
        EventId("e-axis".to_string()),
        EventDef {
            id: EventId("e-axis".to_string()),
            // met_namu is already true in hand_built_state → trigger fires.
            trigger: ConditionExpr::FlagEquals {
                id: "met_namu".to_string(),
                value: true,
            },
            priority: 1,
            effects,
            allow_relationship_skip: false,
            route_lock: None,
        },
    );
    r
}

fn patch_firing_axis_event() -> StatePatch {
    StatePatch {
        add_events: BTreeSet::from(["e-axis".to_string()]),
        ..StatePatch::default()
    }
}

#[test]
fn event_trust_delta_advances_character_trust_stat() {
    let state = hand_built_state(); // c-namu.trust = 10
    let rules = rules_with_event("trust", 2);
    let result = apply_patch(&state, &rules, &patch_firing_axis_event(), GEN_ID).expect("applies");

    assert!(result.fired_events.contains(&EventId("e-axis".to_string())));
    // Event trust_delta reached the character stat: 10 + 2 = 12.
    let after = result
        .state
        .characters
        .get(&CharacterId("c-namu".to_string()))
        .unwrap();
    assert_eq!(
        after.trust, 12,
        "event trust_delta must land in characters[c].trust"
    );

    // A subsequent TrustAtLeast reads the advanced stat (the bug being fixed).
    let ctx = EvalContext {
        state: &result.state,
        rules: &rules,
    };
    assert!(
        evaluate(
            &ConditionExpr::TrustAtLeast {
                character: CharacterId("c-namu".to_string()),
                value: 12,
            },
            &ctx,
        ),
        "TrustAtLeast {{ c-namu, 12 }} must be true after the event fired"
    );
    assert!(
        !evaluate(
            &ConditionExpr::TrustAtLeast {
                character: CharacterId("c-namu".to_string()),
                value: 13,
            },
            &ctx,
        ),
        "TrustAtLeast {{ c-namu, 13 }} must be false — proves the gate reads the real stat"
    );

    // The change log carries an EventRule entry on the trust field path.
    assert!(result.change_log.iter().any(|e| {
        e.field_path == "characters.c-namu.trust"
            && e.source == story_model::ChangeSource::EventRule
            && e.rule_id.as_deref() == Some("e-axis")
    }));
}

#[test]
fn event_tension_delta_advances_character_tension_stat() {
    let state = hand_built_state(); // c-namu.tension = 3
    let rules = rules_with_event("tension", 2);
    let result = apply_patch(&state, &rules, &patch_firing_axis_event(), GEN_ID).expect("applies");

    let after = result
        .state
        .characters
        .get(&CharacterId("c-namu".to_string()))
        .unwrap();
    assert_eq!(
        after.tension, 5,
        "event tension_delta must land in characters[c].tension"
    );

    let ctx = EvalContext {
        state: &result.state,
        rules: &rules,
    };
    // Tension has no dedicated ConditionExpr variant, but the stat change is
    // observable directly and the change-log entry proves the typed path.
    assert!(result.change_log.iter().any(|e| {
        e.field_path == "characters.c-namu.tension"
            && e.source == story_model::ChangeSource::EventRule
    }));
    let _ = ctx; // silence unused warning if ConditionExpr grows no tension variant
}

#[test]
fn event_trust_delta_out_of_range_is_rejected_atomically() {
    let state = hand_built_state(); // c-namu.trust = 10
    let rules = rules_with_event("trust", 5); // default delta_limits.trust = -3..=3
    let err = apply_patch(&state, &rules, &patch_firing_axis_event(), GEN_ID).unwrap_err();
    assert_eq!(err.reason_code(), Some(PATCH_DELTA_OUT_OF_RANGE));
    // Atomicity: caller's state untouched.
    assert_eq!(state.revision, 5);
    assert_eq!(
        state
            .characters
            .get(&CharacterId("c-namu".to_string()))
            .unwrap()
            .trust,
        10
    );
}

#[test]
fn event_trust_delta_projected_stat_out_of_range_is_rejected() {
    let state = hand_built_state();
    let mut rules = hand_built_rules();
    // Permit the delta magnitude but shrink the stat ceiling so 10 + 3 = 13
    // exceeds stat_limits.max (= 12).
    rules.stat_limits = story_model::Int16Range { min: -100, max: 12 };
    rules.events.insert(
        EventId("e-axis".to_string()),
        EventDef {
            id: EventId("e-axis".to_string()),
            trigger: ConditionExpr::FlagEquals {
                id: "met_namu".to_string(),
                value: true,
            },
            priority: 1,
            effects: EventEffects {
                trust_delta: BTreeMap::from([(CharacterId("c-namu".to_string()), 3)]),
                ..EventEffects::default()
            },
            allow_relationship_skip: false,
            route_lock: None,
        },
    );
    let err = apply_patch(&state, &rules, &patch_firing_axis_event(), GEN_ID).unwrap_err();
    assert_eq!(err.reason_code(), Some(PATCH_STAT_OUT_OF_RANGE));
}
