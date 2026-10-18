//! Boundary-table tests for the 8-stage patch validator (W3.3 / ST-007).
//!
//! Stage 2 (reference): an unknown id is *dropped* — the sanitized patch
//! loses the entry, and each drop is audited as one warning plus one
//! `Rejected` change-log entry. Stages 3–5: a violation is rejected with the
//! documented reason code, and a just-in-bounds patch is accepted.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use story_core::error::{
    CoreError, PATCH_DELTA_OUT_OF_RANGE, PATCH_NEGATIVE_INVENTORY, PATCH_NONADJACENT_LOCATION,
    PATCH_NUMBER_OUT_OF_RANGE, PATCH_RELATIONSHIP_SKIP, PATCH_STAT_OUT_OF_RANGE,
    PATCH_STRING_VIOLATION, PATCH_UNKNOWN_CHARACTER, PATCH_UNKNOWN_EVENT, PATCH_UNKNOWN_FLAG,
    PATCH_UNKNOWN_ITEM, PATCH_UNKNOWN_LOCATION, PATCH_UNKNOWN_NUMBER, PATCH_UNKNOWN_STRING,
};
use story_core::validator::{ValidationOutcome, validate};
use story_model::{
    ChangeDecision, CharacterId, CharacterState, Emotion, EventDef, EventEffects, EventId,
    GameState, Int16Range, LocationDef, LocationId, NumberVarDef, RelationshipStage,
    SAVE_SCHEMA_VERSION, StatePatch, StoryRules, StringVarDef,
};

const GEN_ID: uuid::Uuid = uuid::Uuid::nil();

fn fresh_state() -> GameState {
    let mut characters = BTreeMap::new();
    characters.insert(
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
    let mut flags = BTreeMap::new();
    flags.insert("met_namu".to_string(), true);
    let mut numbers = BTreeMap::new();
    numbers.insert("score".to_string(), 7);
    let mut inventory = BTreeMap::new();
    inventory.insert("i-letter".to_string(), 2);

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
        characters,
        flags,
        numbers,
        strings: BTreeMap::new(),
        inventory,
        occurred_events: BTreeSet::new(),
        recent_summaries: VecDeque::new(),
        memories: Vec::new(),
        history: Vec::new(),
        last_choice: None,
        revision: 0,
        stable_generation_id: None,
        lorebook_activations: Vec::new(),
    }
}

fn fresh_rules() -> StoryRules {
    let mut rules = StoryRules::minimal();
    let mut adj = BTreeSet::new();
    adj.insert(LocationId("loc-hall".to_string()));
    rules.locations.insert(
        LocationId("loc-classroom".to_string()),
        LocationDef {
            id: LocationId("loc-classroom".to_string()),
            display_name: "Classroom".to_string(),
            adjacent: adj,
        },
    );
    rules.locations.insert(
        LocationId("loc-hall".to_string()),
        LocationDef {
            id: LocationId("loc-hall".to_string()),
            display_name: "Hall".to_string(),
            adjacent: BTreeSet::from([LocationId("loc-classroom".to_string())]),
        },
    );
    rules
        .allowed_flags
        .extend(["met_namu", "f-brave"].map(String::from));
    rules.numbers.insert(
        "score".to_string(),
        NumberVarDef {
            id: "score".to_string(),
            min: 0,
            max: 100,
            default: 50,
        },
    );
    let mut allowed_vals = BTreeSet::new();
    allowed_vals.insert("formal".to_string());
    allowed_vals.insert("casual".to_string());
    rules.strings.insert(
        "address_form".to_string(),
        StringVarDef {
            id: "address_form".to_string(),
            max_len: 8,
            allowed: Some(allowed_vals),
        },
    );
    rules.allowed_items.insert("i-letter".to_string());
    rules.events.insert(
        EventId("e-rooftop".to_string()),
        EventDef {
            id: EventId("e-rooftop".to_string()),
            trigger: story_model::ConditionExpr::FlagEquals {
                id: "met_namu".to_string(),
                value: true,
            },
            priority: 1,
            effects: EventEffects::default(),
            allow_relationship_skip: false,
            route_lock: None,
        },
    );
    rules.events.insert(
        EventId("e-skip".to_string()),
        EventDef {
            id: EventId("e-skip".to_string()),
            trigger: story_model::ConditionExpr::FlagEquals {
                id: "met_namu".to_string(),
                value: true,
            },
            priority: 1,
            effects: EventEffects::default(),
            allow_relationship_skip: true,
            route_lock: None,
        },
    );
    rules
}

fn run(
    state: &GameState,
    rules: &StoryRules,
    patch: &StatePatch,
) -> Result<ValidationOutcome, CoreError> {
    validate(state, rules, patch, GEN_ID)
}

fn reason(err: &CoreError) -> Option<&'static str> {
    err.reason_code()
}

// ---------------------------------------------------------------------------
// Happy paths
// ---------------------------------------------------------------------------

#[test]
fn empty_patch_validates() {
    let st = fresh_state();
    let rules = fresh_rules();
    let patch = StatePatch::default();
    let outcome = run(&st, &rules, &patch).expect("empty patch validates");
    assert!(outcome.warnings.is_empty());
    assert!(outcome.change_log.is_empty());
    assert!(outcome.sanitized.is_empty());
}

#[test]
fn in_bounds_delta_validates() {
    let mut st = fresh_state();
    // Push the character near the boundary to also exercise stat bounds.
    st.characters
        .get_mut(&CharacterId("c-namu".to_string()))
        .unwrap()
        .affection = 99;
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    patch
        .affection_delta
        .insert(CharacterId("c-namu".to_string()), 1);
    run(&st, &rules, &patch).expect("+1 delta at 99 → 100 within stat_limits");
}

// ---------------------------------------------------------------------------
// Stage 2: Reference — unknown references are dropped, not rejected
// ---------------------------------------------------------------------------

/// The full drop contract, exercised on `set_strings` (the shape that
/// triggered the live failure this policy fixes): the unknown key leaves the
/// sanitized patch, the known key survives, and the drop is audited as one
/// warning plus one `Rejected` change-log entry carrying the stage-2 code.
#[test]
fn unknown_string_var_is_dropped_with_audit() {
    let st = fresh_state();
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    patch
        .set_strings
        .insert("nonexistent".to_string(), "formal".to_string());
    patch
        .set_strings
        .insert("address_form".to_string(), "formal".to_string());
    let outcome = run(&st, &rules, &patch).expect("unknown reference drops the entry, not the patch");
    assert!(
        !outcome.sanitized.set_strings.contains_key("nonexistent"),
        "dropped key absent from sanitized patch"
    );
    assert_eq!(
        outcome.sanitized.set_strings.get("address_form").map(String::as_str),
        Some("formal"),
        "known key survives alongside the drop"
    );
    assert_eq!(outcome.warnings.len(), 1);
    assert_eq!(outcome.warnings[0].code, PATCH_UNKNOWN_STRING);
    assert!(
        outcome.warnings[0].detail.contains("\"nonexistent\""),
        "detail quotes the dropped key with its exact value: {}",
        outcome.warnings[0].detail
    );
    assert_eq!(outcome.change_log.len(), 1);
    let entry = &outcome.change_log[0];
    assert_eq!(entry.decision, ChangeDecision::Rejected);
    assert_eq!(entry.reason_code.as_deref(), Some(PATCH_UNKNOWN_STRING));
    assert!(entry.applied.is_none(), "dropped entries apply nothing");
    assert_eq!(entry.generation_id, GEN_ID);
    assert_eq!(entry.turn, st.turn, "audit entries stamp the current turn");
}

#[test]
fn unknown_character_is_dropped() {
    let st = fresh_state();
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    patch
        .affection_delta
        .insert(CharacterId("c-ghost".to_string()), 1);
    patch
        .trust_delta
        .insert(CharacterId("c-namu".to_string()), 1);
    let outcome = run(&st, &rules, &patch).expect("unknown character drops its entry");
    assert!(outcome.sanitized.affection_delta.is_empty());
    assert_eq!(
        outcome.sanitized.trust_delta.get(&CharacterId("c-namu".to_string())),
        Some(&1),
        "known character's delta survives"
    );
    assert_eq!(outcome.warnings[0].code, PATCH_UNKNOWN_CHARACTER);
}

#[test]
fn empty_string_key_is_dropped() {
    // Regression: a local model emitted a set_strings entry whose key was
    // effectively empty; before the drop policy the whole turn failed with
    // patch_unknown_string and only the [R] full re-roll could recover.
    let st = fresh_state();
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    patch.set_strings.insert(String::new(), "formal".to_string());
    let outcome = run(&st, &rules, &patch).expect("empty key drops, turn survives");
    assert!(outcome.sanitized.set_strings.is_empty());
    assert_eq!(outcome.warnings[0].code, PATCH_UNKNOWN_STRING);
}

#[test]
fn unknown_flag_is_dropped() {
    let st = fresh_state();
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    patch.set_flags.insert("f-mystery".to_string(), true);
    patch.set_flags.insert("met_namu".to_string(), true);
    let outcome = run(&st, &rules, &patch).expect("unknown flag drops its entry");
    assert!(!outcome.sanitized.set_flags.contains_key("f-mystery"));
    assert_eq!(outcome.sanitized.set_flags.get("met_namu"), Some(&true));
    assert_eq!(outcome.warnings[0].code, PATCH_UNKNOWN_FLAG);
}

#[test]
fn unknown_number_var_is_dropped() {
    let st = fresh_state();
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    patch.number_delta.insert("nonexistent".to_string(), 1);
    let outcome = run(&st, &rules, &patch).expect("unknown number var drops its entry");
    assert!(outcome.sanitized.number_delta.is_empty());
    assert_eq!(outcome.warnings[0].code, PATCH_UNKNOWN_NUMBER);
}

#[test]
fn unknown_item_is_dropped() {
    let st = fresh_state();
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    patch.inventory_delta.insert("i-sword".to_string(), 1);
    let outcome = run(&st, &rules, &patch).expect("unknown item drops its entry");
    assert!(outcome.sanitized.inventory_delta.is_empty());
    assert_eq!(outcome.warnings[0].code, PATCH_UNKNOWN_ITEM);
}

#[test]
fn unknown_event_is_dropped() {
    let st = fresh_state();
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    patch.add_events.insert("e-nope".to_string());
    patch.add_events.insert("e-rooftop".to_string());
    let outcome = run(&st, &rules, &patch).expect("unknown event drops its entry");
    assert!(!outcome.sanitized.add_events.contains("e-nope"));
    assert!(outcome.sanitized.add_events.contains("e-rooftop"));
    assert_eq!(outcome.warnings[0].code, PATCH_UNKNOWN_EVENT);
}

#[test]
fn unknown_location_is_dropped() {
    let st = fresh_state();
    let rules = fresh_rules();
    let patch = StatePatch {
        next_location: Some(LocationId("loc-mars".to_string())),
        ..StatePatch::default()
    };
    let outcome = run(&st, &rules, &patch).expect("unknown location drops the move");
    assert_eq!(outcome.sanitized.next_location, None);
    assert_eq!(outcome.warnings[0].code, PATCH_UNKNOWN_LOCATION);
}

#[test]
fn unknown_refs_across_maps_all_drop_and_known_parts_still_range_check() {
    // One stray key in every reference-checked field, plus valid entries that
    // must survive BOTH the drop and the subsequent range stages.
    let st = fresh_state();
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    patch
        .affection_delta
        .insert(CharacterId("c-ghost".to_string()), 1);
    patch
        .affection_delta
        .insert(CharacterId("c-namu".to_string()), 1);
    patch.set_strings.insert(String::new(), "x".to_string());
    patch.set_flags.insert("f-mystery".to_string(), true);
    patch.inventory_delta.insert("i-sword".to_string(), 1);
    patch.add_events.insert("e-nope".to_string());
    let outcome = run(&st, &rules, &patch).expect("turn survives a stray key in every map");
    assert_eq!(
        outcome.sanitized.affection_delta.get(&CharacterId("c-namu".to_string())),
        Some(&1)
    );
    assert!(outcome.sanitized.set_strings.is_empty());
    assert!(outcome.sanitized.set_flags.is_empty());
    assert!(outcome.sanitized.inventory_delta.is_empty());
    assert!(outcome.sanitized.add_events.is_empty());
    assert_eq!(outcome.warnings.len(), 5, "one warning per dropped key");
}

// ---------------------------------------------------------------------------
// Stage 3: Range
// ---------------------------------------------------------------------------

#[test]
fn delta_below_minus_3_is_rejected() {
    let st = fresh_state();
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    patch
        .affection_delta
        .insert(CharacterId("c-namu".to_string()), -4);
    let err = run(&st, &rules, &patch).unwrap_err();
    assert_eq!(reason(&err), Some(PATCH_DELTA_OUT_OF_RANGE));
}

#[test]
fn delta_above_plus_3_is_rejected() {
    let st = fresh_state();
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    patch
        .affection_delta
        .insert(CharacterId("c-namu".to_string()), 4);
    let err = run(&st, &rules, &patch).unwrap_err();
    assert_eq!(reason(&err), Some(PATCH_DELTA_OUT_OF_RANGE));
}

#[test]
fn projected_stat_above_plus_100_is_rejected() {
    let mut st = fresh_state();
    st.characters
        .get_mut(&CharacterId("c-namu".to_string()))
        .unwrap()
        .affection = 99;
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    // 99 + 3 = 102 > 100 → reject even though the delta magnitude is in range.
    patch
        .affection_delta
        .insert(CharacterId("c-namu".to_string()), 3);
    let err = run(&st, &rules, &patch).unwrap_err();
    assert_eq!(reason(&err), Some(PATCH_STAT_OUT_OF_RANGE));
}

#[test]
fn projected_stat_below_minus_100_is_rejected() {
    let mut st = fresh_state();
    st.characters
        .get_mut(&CharacterId("c-namu".to_string()))
        .unwrap()
        .affection = -98;
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    patch
        .affection_delta
        .insert(CharacterId("c-namu".to_string()), -3);
    let err = run(&st, &rules, &patch).unwrap_err();
    assert_eq!(reason(&err), Some(PATCH_STAT_OUT_OF_RANGE));
}

#[test]
fn number_delta_out_of_var_bounds_is_rejected() {
    let mut st = fresh_state();
    st.numbers.insert("score".to_string(), 99);
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    // 99 + 3 = 102 > 100 (var max).
    patch.number_delta.insert("score".to_string(), 3);
    let err = run(&st, &rules, &patch).unwrap_err();
    assert_eq!(reason(&err), Some(PATCH_NUMBER_OUT_OF_RANGE));
}

#[test]
fn string_too_long_is_rejected() {
    let st = fresh_state();
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    // max_len = 8; "way-too-long" exceeds it.
    patch
        .set_strings
        .insert("address_form".to_string(), "way-too-long".to_string());
    let err = run(&st, &rules, &patch).unwrap_err();
    assert_eq!(reason(&err), Some(PATCH_STRING_VIOLATION));
}

#[test]
fn string_not_in_allowed_set_is_rejected() {
    let st = fresh_state();
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    patch
        .set_strings
        .insert("address_form".to_string(), "rude".to_string());
    let err = run(&st, &rules, &patch).unwrap_err();
    assert_eq!(reason(&err), Some(PATCH_STRING_VIOLATION));
}

#[test]
fn negative_inventory_is_rejected() {
    let st = fresh_state();
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    // State has 2 letters; remove 3 → -1.
    patch.inventory_delta.insert("i-letter".to_string(), -3);
    let err = run(&st, &rules, &patch).unwrap_err();
    assert_eq!(reason(&err), Some(PATCH_NEGATIVE_INVENTORY));
}

// ---------------------------------------------------------------------------
// Stage 4: Permission (relationship transitions)
// ---------------------------------------------------------------------------

#[test]
fn relationship_skip_is_rejected_without_granting_event() {
    let st = fresh_state(); // Friend (2)
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    // Friend (2) → RomanticInterest (4) = 2 steps; e-rooftop has no skip grant.
    patch.relationship_transitions.insert(
        CharacterId("c-namu".to_string()),
        RelationshipStage::RomanticInterest,
    );
    patch.add_events.insert("e-rooftop".to_string());
    let err = run(&st, &rules, &patch).unwrap_err();
    assert_eq!(reason(&err), Some(PATCH_RELATIONSHIP_SKIP));
}

#[test]
fn relationship_skip_passes_with_granting_event() {
    let st = fresh_state();
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    patch.relationship_transitions.insert(
        CharacterId("c-namu".to_string()),
        RelationshipStage::RomanticInterest,
    );
    patch.add_events.insert("e-skip".to_string()); // grants allow_relationship_skip
    run(&st, &rules, &patch).expect("skip allowed when granting event is in add_events");
}

#[test]
fn relationship_to_estranged_always_passes() {
    let st = fresh_state();
    let rules = fresh_rules();
    let mut patch = StatePatch::default();
    patch.relationship_transitions.insert(
        CharacterId("c-namu".to_string()),
        RelationshipStage::Estranged,
    );
    run(&st, &rules, &patch).expect("Estranged is reachable from any stage");
}

// ---------------------------------------------------------------------------
// Stage 5: Transition (location adjacency)
// ---------------------------------------------------------------------------

#[test]
fn nonadjacent_location_is_rejected() {
    let st = fresh_state(); // loc-classroom; adjacent = {loc-hall}
    let rules = fresh_rules();
    let patch = StatePatch {
        next_location: Some(LocationId("loc-hallway".to_string())), // not in adjacency
        ..StatePatch::default()
    };
    // Add loc-hallway to locations so it passes reference check, then fails adjacency.
    let mut rules = rules;
    rules.locations.insert(
        LocationId("loc-hallway".to_string()),
        LocationDef {
            id: LocationId("loc-hallway".to_string()),
            display_name: "Hallway".to_string(),
            adjacent: BTreeSet::new(),
        },
    );
    let err = run(&st, &rules, &patch).unwrap_err();
    assert_eq!(reason(&err), Some(PATCH_NONADJACENT_LOCATION));
}

#[test]
fn adjacent_location_passes() {
    let st = fresh_state(); // loc-classroom
    let rules = fresh_rules();
    let patch = StatePatch {
        next_location: Some(LocationId("loc-hall".to_string())), // in adjacency
        ..StatePatch::default()
    };
    run(&st, &rules, &patch).expect("adjacent move accepted");
}

#[test]
fn same_location_is_accepted_as_noop() {
    let st = fresh_state(); // loc-classroom
    let rules = fresh_rules();
    let patch = StatePatch {
        next_location: Some(LocationId("loc-classroom".to_string())),
        ..StatePatch::default()
    };
    run(&st, &rules, &patch).expect("staying in the current location is valid");
}

#[test]
fn nonadjacent_forced_by_event_passes() {
    let st = fresh_state(); // loc-classroom
    let mut rules = fresh_rules();
    // Add a far-away location and an event that forces it.
    rules.locations.insert(
        LocationId("loc-roof".to_string()),
        LocationDef {
            id: LocationId("loc-roof".to_string()),
            display_name: "Rooftop".to_string(),
            adjacent: BTreeSet::new(),
        },
    );
    rules.events.insert(
        EventId("e-force-roof".to_string()),
        EventDef {
            id: EventId("e-force-roof".to_string()),
            trigger: story_model::ConditionExpr::FlagEquals {
                id: "met_namu".to_string(),
                value: true,
            },
            priority: 1,
            effects: EventEffects {
                force_location: Some(LocationId("loc-roof".to_string())),
                ..EventEffects::default()
            },
            allow_relationship_skip: false,
            route_lock: None,
        },
    );
    let patch = StatePatch {
        next_location: Some(LocationId("loc-roof".to_string())),
        add_events: BTreeSet::from(["e-force-roof".to_string()]),
        ..StatePatch::default()
    };
    run(&st, &rules, &patch).expect("non-adjacent move accepted when an event forces the location");
}

// ---------------------------------------------------------------------------
// Default limits sanity
// ---------------------------------------------------------------------------

#[test]
fn default_delta_range_is_minus_three_to_three() {
    let rules = StoryRules::minimal();
    assert_eq!(rules.delta_limits.affection, Int16Range { min: -3, max: 3 });
    assert_eq!(rules.delta_limits.trust, Int16Range { min: -3, max: 3 });
    assert_eq!(rules.delta_limits.tension, Int16Range { min: -3, max: 3 });
}
