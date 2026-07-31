//! Serde round-trip + golden fixture tests for the W3.1 rule structures and
//! change log (Part A of the W3 plan).

use std::collections::{BTreeMap, BTreeSet};

use story_model::{
    ChangeDecision, ChangeSource, CharacterId, ConditionExpr, DEFAULT_DELTA_MAX, DEFAULT_DELTA_MIN,
    DEFAULT_STAT_MAX, DEFAULT_STAT_MIN, EndingDef, EventDef, EventEffects, Int16Range, LocationDef,
    LocationId, NumberVarDef, RouteDef, StateChangeLog, StoryRules, StringVarDef,
};

fn sample_rules() -> StoryRules {
    let mut locations = BTreeMap::new();
    let mut adj = BTreeSet::new();
    adj.insert(LocationId("loc-hall".to_string()));
    locations.insert(
        LocationId("loc-classroom".to_string()),
        LocationDef {
            id: LocationId("loc-classroom".to_string()),
            display_name: "Classroom 2-B".to_string(),
            adjacent: adj,
        },
    );

    let mut allowed_flags = BTreeSet::new();
    allowed_flags.insert("met_namu".to_string());
    allowed_flags.insert("f-brave".to_string());

    let mut numbers = BTreeMap::new();
    numbers.insert(
        "score".to_string(),
        NumberVarDef {
            id: "score".to_string(),
            min: 0,
            max: 100,
            default: 50,
        },
    );

    let mut strings = BTreeMap::new();
    let mut allowed_vals = BTreeSet::new();
    allowed_vals.insert("formal".to_string());
    allowed_vals.insert("casual".to_string());
    strings.insert(
        "address_form".to_string(),
        StringVarDef {
            id: "address_form".to_string(),
            max_len: 32,
            allowed: Some(allowed_vals),
        },
    );

    let mut allowed_items = BTreeSet::new();
    allowed_items.insert("i-letter".to_string());

    let mut events = BTreeMap::new();
    events.insert(
        story_model::EventId("e-rooftop".to_string()),
        EventDef {
            id: story_model::EventId("e-rooftop".to_string()),
            trigger: ConditionExpr::AtLocation {
                location: LocationId("loc-roof".to_string()),
            },
            priority: 5,
            effects: EventEffects {
                affection_delta: BTreeMap::from([(CharacterId("c-namu".to_string()), 2)]),
                trust_delta: BTreeMap::new(),
                tension_delta: BTreeMap::new(),
                set_flags: BTreeMap::from([("f-brave".to_string(), true)]),
                number_delta: BTreeMap::new(),
                add_events: BTreeSet::new(),
                force_location: Some(LocationId("loc-roof".to_string())),
                force_relationship: BTreeMap::new(),
                ending: None,
            },
            allow_relationship_skip: false,
            route_lock: Some("route-roof".to_string()),
        },
    );

    let mut routes = BTreeMap::new();
    routes.insert(
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

    let mut endings = BTreeMap::new();
    endings.insert(
        "end-good".to_string(),
        EndingDef {
            id: "end-good".to_string(),
            condition: ConditionExpr::AffectionAtLeast {
                character: CharacterId("c-namu".to_string()),
                value: 100,
            },
            label: "Good ending".to_string(),
        },
    );

    StoryRules {
        locations,
        allowed_flags,
        numbers,
        strings,
        allowed_items,
        events,
        routes,
        endings,
        delta_limits: story_model::DeltaLimits::default(),
        stat_limits: Int16Range {
            min: DEFAULT_STAT_MIN,
            max: DEFAULT_STAT_MAX,
        },
        default_choice_count: 3,
    }
}

fn sample_change_log() -> StateChangeLog {
    StateChangeLog {
        id: uuid::Uuid::nil(),
        generation_id: uuid::Uuid::nil(),
        turn: 7,
        source: ChangeSource::LlmPatch,
        field_path: "characters.c-namu.affection".to_string(),
        before: serde_json::json!(25),
        proposed: serde_json::json!(31),
        applied: Some(serde_json::json!(31)),
        decision: ChangeDecision::Applied,
        rule_id: None,
        reason_code: None,
    }
}

// ---------------------------------------------------------------------------
// Round-trip tests
// ---------------------------------------------------------------------------

#[test]
fn story_rules_round_trip() {
    let rules = sample_rules();
    let json = serde_json::to_string(&rules).expect("serialize");
    let back: StoryRules = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(rules, back);
}

#[test]
fn story_rules_minimal_round_trip() {
    let rules = StoryRules::minimal();
    let json = serde_json::to_string(&rules).expect("serialize");
    let back: StoryRules = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(rules, back);
    // Const-derived defaults survive the round trip.
    assert_eq!(back.stat_limits.min, DEFAULT_STAT_MIN);
    assert_eq!(back.stat_limits.max, DEFAULT_STAT_MAX);
    assert_eq!(back.delta_limits.affection.min, DEFAULT_DELTA_MIN);
    assert_eq!(back.delta_limits.affection.max, DEFAULT_DELTA_MAX);
}

#[test]
fn change_log_round_trip() {
    let log = sample_change_log();
    let json = serde_json::to_string(&log).expect("serialize");
    let back: StateChangeLog = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(log, back);
}

#[test]
fn change_log_source_and_decision_are_snake_case() {
    let json = serde_json::to_string(&sample_change_log()).expect("serialize");
    assert!(json.contains("\"source\":\"llm_patch\""), "got: {json}");
    assert!(json.contains("\"decision\":\"applied\""), "got: {json}");
}

#[test]
fn int16_range_round_trip_and_contains() {
    let r = Int16Range { min: -3, max: 3 };
    let json = serde_json::to_string(&r).unwrap();
    let back: Int16Range = serde_json::from_str(&json).unwrap();
    assert_eq!(r, back);
    assert!(back.contains(0));
    assert!(!back.contains(4));
}

// ---------------------------------------------------------------------------
// Golden fixture
// ---------------------------------------------------------------------------

const RULES_GOLDEN: &str = include_str!("fixtures/golden/story_rules_v1.json");

#[test]
fn golden_story_rules_loads() {
    let rules: StoryRules = serde_json::from_str(RULES_GOLDEN).expect("golden story_rules loads");
    assert_eq!(rules.stat_limits.min, DEFAULT_STAT_MIN);
    assert_eq!(rules.default_choice_count, 3);
    assert!(
        rules
            .locations
            .contains_key(&LocationId("loc-classroom".to_string()))
    );
    assert!(
        rules
            .events
            .contains_key(&story_model::EventId("e-rooftop".to_string()))
    );
}

#[test]
fn story_rules_serialization_is_stable_against_golden() {
    let rules = sample_rules();
    let json = serde_json::to_string_pretty(&rules).expect("serialize");
    // The pretty JSON must match the committed golden byte-for-byte; if the
    // schema changes intentionally, refresh the fixture via the ignored test
    // below.
    assert_eq!(json, RULES_GOLDEN, "story_rules JSON drifted from golden");
}

/// Rewrite the committed golden fixture from [`sample_rules`]. Run on demand
/// with `cargo test -p story-model regenerate_story_rules_golden -- --ignored`
/// whenever the schema intentionally changes; commit the refreshed JSON.
#[test]
#[ignore]
fn regenerate_story_rules_golden() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/golden");
    let json = serde_json::to_string_pretty(&sample_rules()).expect("serialize");
    std::fs::write(format!("{dir}/story_rules_v1.json"), json).expect("write story_rules_v1.json");
}
