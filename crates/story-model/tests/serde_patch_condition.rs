//! Policy tests for [`StatePatch`] (`deny_unknown_fields`, `is_empty`) and
//! [`ConditionExpr`] (internally-tagged, snake_case, nesting).

use std::collections::BTreeMap;

use story_model::{
    CharacterId, CompareOp, ConditionExpr, LocationId, RelationshipStage, StatePatch, TimeSlot,
};

// ---------------------------------------------------------------------------
// StatePatch
// ---------------------------------------------------------------------------

#[test]
fn state_patch_default_is_empty() {
    let patch = StatePatch::default();
    assert!(patch.is_empty());
}

#[test]
fn state_patch_with_any_field_is_not_empty() {
    // Exercise each field individually to make sure is_empty() respects all of
    // them — a future field added without updating the Default comparison would
    // otherwise silently break this contract.
    let base = StatePatch::default();

    let mut p = base.clone();
    p.affection_delta
        .insert(CharacterId("c-namu".to_string()), 5);
    assert!(!p.is_empty(), "affection_delta should make patch non-empty");

    let mut p = base.clone();
    p.trust_delta.insert(CharacterId("c-namu".to_string()), -3);
    assert!(!p.is_empty());

    let mut p = base.clone();
    p.tension_delta.insert(CharacterId("c-namu".to_string()), 1);
    assert!(!p.is_empty());

    let mut p = base.clone();
    p.relationship_transitions
        .insert(CharacterId("c-namu".to_string()), RelationshipStage::Friend);
    assert!(!p.is_empty());

    let mut p = base.clone();
    p.set_flags.insert("met_namu".to_string(), true);
    assert!(!p.is_empty());

    let mut p = base.clone();
    p.number_delta.insert("score".to_string(), 10);
    assert!(!p.is_empty());

    let mut p = base.clone();
    p.set_strings
        .insert("address_form".to_string(), "senpai".to_string());
    assert!(!p.is_empty());

    let mut p = base.clone();
    p.inventory_delta.insert("i-key".to_string(), 1);
    assert!(!p.is_empty());

    let mut p = base.clone();
    p.add_events.insert("e-arrival".to_string());
    assert!(!p.is_empty());

    let mut p = base.clone();
    p.next_location = Some(LocationId("loc-roof".to_string()));
    assert!(!p.is_empty());

    let mut p = base.clone();
    p.next_time_slot = Some(TimeSlot::Evening);
    assert!(!p.is_empty());

    let mut p = base.clone();
    p.proposed_route = Some("route-roof".to_string());
    assert!(!p.is_empty());
}

#[test]
fn state_patch_round_trips() {
    let mut patch = StatePatch::default();
    patch
        .affection_delta
        .insert(CharacterId("c-namu".to_string()), 8);
    patch
        .trust_delta
        .insert(CharacterId("c-namu".to_string()), 4);
    patch.relationship_transitions.insert(
        CharacterId("c-namu".to_string()),
        RelationshipStage::CloseFriend,
    );
    patch.set_flags.insert("f-brave".to_string(), true);
    patch.number_delta.insert("n-courage".to_string(), 1);
    patch
        .set_strings
        .insert("address_form".to_string(), "선배".to_string());
    patch.inventory_delta.insert("i-letter".to_string(), 1);
    patch.add_events.insert("e-confession".to_string());
    patch.next_location = Some(LocationId("loc-roof".to_string()));
    patch.next_time_slot = Some(TimeSlot::Evening);
    patch.proposed_route = Some("route-confession".to_string());

    let json = serde_json::to_string_pretty(&patch).expect("serialize");
    let back: StatePatch = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, patch, "round-trip not equal");
    assert!(!back.is_empty());
}

#[test]
fn state_patch_deny_unknown_fields_rejects_unknown_key() {
    // The patch is the model-output contract: an unknown key MUST be a hard
    // serde error, not a silently-dropped field. This test would FAIL if
    // `deny_unknown_fields` were removed from StatePatch.
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
        "THIS_FIELD_DOES_NOT_EXIST": 42
    }"#;
    let result: Result<StatePatch, serde_json::Error> = serde_json::from_str(json);
    let err = result.expect_err("unknown field must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown field") || msg.contains("THIS_FIELD_DOES_NOT_EXIST"),
        "expected an unknown-field error, got: {msg}"
    );
}

#[test]
fn state_patch_requires_all_known_fields() {
    // StatePatch is the strict model-output contract: the model MUST emit every
    // field (empty collections / null Options are fine), so an object missing a
    // known field is rejected. This is the flip side of `deny_unknown_fields`.
    let json = "{}";
    let result: Result<StatePatch, serde_json::Error> = serde_json::from_str(json);
    let err = result.expect_err("missing-field object must be rejected");
    assert!(
        err.to_string().contains("missing field"),
        "expected a missing-field error, got: {err}"
    );
}

#[test]
fn state_patch_known_keys_still_accepted_after_policy_check() {
    // Sanity: a JSON using every known field (with nulls for Options) loads.
    let json = r#"{
        "affection_delta": {"c-namu": 5},
        "trust_delta": {},
        "tension_delta": {},
        "relationship_transitions": {},
        "set_flags": {"f-x": true},
        "number_delta": {},
        "set_strings": {},
        "inventory_delta": {},
        "add_events": [],
        "next_location": "loc-roof",
        "next_time_slot": "night",
        "proposed_route": null
    }"#;
    let patch: StatePatch = serde_json::from_str(json).expect("known fields load");
    assert_eq!(
        patch.next_location.as_ref().map(|l| l.0.as_str()),
        Some("loc-roof")
    );
    assert_eq!(patch.next_time_slot, Some(TimeSlot::Night));
    let mut expected_aff = BTreeMap::new();
    expected_aff.insert(CharacterId("c-namu".to_string()), 5);
    assert_eq!(patch.affection_delta, expected_aff);
}

// ---------------------------------------------------------------------------
// ConditionExpr
// ---------------------------------------------------------------------------

#[test]
fn condition_expr_tagged_round_trip_simple() {
    let cases = vec![
        ConditionExpr::FlagEquals {
            id: "met_namu".to_string(),
            value: true,
        },
        ConditionExpr::NumberCompare {
            id: "score".to_string(),
            op: CompareOp::Ge,
            value: 10,
        },
        ConditionExpr::AffectionAtLeast {
            character: CharacterId("c-namu".to_string()),
            value: 20,
        },
        ConditionExpr::TrustAtLeast {
            character: CharacterId("c-namu".to_string()),
            value: -5,
        },
        ConditionExpr::EventOccurred {
            id: "e-arrival".to_string(),
        },
        ConditionExpr::AtLocation {
            location: LocationId("loc-roof".to_string()),
        },
        ConditionExpr::TurnAtLeast { value: 3 },
    ];
    for expr in &cases {
        let json = serde_json::to_string(expr).expect("serialize");
        let back: ConditionExpr = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&back, expr, "round-trip not equal for {json}");
    }
}

#[test]
fn condition_expr_tag_is_snake_case() {
    // Policy §6: tags are snake_case, matching every other enum in the crate.
    // (The plan's literal code omitted rename_all; its policy example shows
    // lowercase "all"/"flag_equals". We enforce the policy example.)
    let checks = [
        (ConditionExpr::All { children: vec![] }, "\"kind\":\"all\""),
        (ConditionExpr::Any { children: vec![] }, "\"kind\":\"any\""),
        (
            ConditionExpr::Not {
                child: Box::new(ConditionExpr::All { children: vec![] }),
            },
            "\"kind\":\"not\"",
        ),
        (
            ConditionExpr::FlagEquals {
                id: "x".to_string(),
                value: true,
            },
            "\"kind\":\"flag_equals\"",
        ),
        (
            ConditionExpr::NumberCompare {
                id: "x".to_string(),
                op: CompareOp::Eq,
                value: 0,
            },
            "\"kind\":\"number_compare\"",
        ),
        (
            ConditionExpr::AffectionAtLeast {
                character: CharacterId("c".to_string()),
                value: 0,
            },
            "\"kind\":\"affection_at_least\"",
        ),
        (
            ConditionExpr::TrustAtLeast {
                character: CharacterId("c".to_string()),
                value: 0,
            },
            "\"kind\":\"trust_at_least\"",
        ),
        (
            ConditionExpr::EventOccurred {
                id: "x".to_string(),
            },
            "\"kind\":\"event_occurred\"",
        ),
        (
            ConditionExpr::AtLocation {
                location: LocationId("l".to_string()),
            },
            "\"kind\":\"at_location\"",
        ),
        (
            ConditionExpr::TurnAtLeast { value: 0 },
            "\"kind\":\"turn_at_least\"",
        ),
    ];
    for (expr, expected_fragment) in checks {
        let json = serde_json::to_string(&expr).unwrap();
        assert!(
            json.contains(expected_fragment),
            "expected `{expected_fragment}` in JSON, got: {json}"
        );
    }
}

#[test]
fn condition_expr_nested_all_any_not_round_trip() {
    let tree = ConditionExpr::All {
        children: vec![
            ConditionExpr::FlagEquals {
                id: "met_namu".to_string(),
                value: true,
            },
            ConditionExpr::Any {
                children: vec![
                    ConditionExpr::AtLocation {
                        location: LocationId("loc-roof".to_string()),
                    },
                    ConditionExpr::TurnAtLeast { value: 5 },
                ],
            },
            ConditionExpr::Not {
                child: Box::new(ConditionExpr::AffectionAtLeast {
                    character: CharacterId("c-namu".to_string()),
                    value: -10,
                }),
            },
        ],
    };

    let json = serde_json::to_string_pretty(&tree).expect("serialize");
    let back: ConditionExpr = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(&back, &tree, "nested round-trip not equal");

    // Confirm the tagged shape is human-readable and stable.
    assert!(json.contains("\"kind\": \"all\""));
    assert!(json.contains("\"kind\": \"any\""));
    assert!(json.contains("\"kind\": \"not\""));
    assert!(json.contains("\"children\""));
    assert!(json.contains("\"child\""));
}

#[test]
fn condition_expr_rejects_unknown_tag() {
    // An unknown `kind` must be a deserialization error.
    let json = r#"{"kind":"definitely_not_a_real_variant","id":"x"}"#;
    let result: Result<ConditionExpr, serde_json::Error> = serde_json::from_str(json);
    assert!(result.is_err(), "unknown kind must be rejected");
}
