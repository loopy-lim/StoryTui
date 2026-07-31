//! Round-trip tests for `Choice` (exercising `ConditionExpr`), `CharacterState`,
//! and `ChoiceRef`. The full `GameState` keystone is tested in W2.4 once its
//! memory/history dependencies exist.

use std::collections::{BTreeMap, BTreeSet};

use story_model::{
    CharacterId, CharacterState, Choice, ChoiceRef, ConditionExpr, Emotion, RelationshipStage,
    RiskLevel,
};

/// Serialize -> deserialize -> re-serialize and assert the two JSON forms are
/// identical. Lets us verify round-trip fidelity without requiring `PartialEq`
/// on the type under test.
fn round_trip_stable<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let s1 = serde_json::to_string_pretty(value).expect("serialize (first)");
    let back: T = serde_json::from_str(&s1).expect("deserialize");
    let s2 = serde_json::to_string_pretty(&back).expect("serialize (second)");
    assert_eq!(s1, s2, "round-trip changed the JSON form");
}

#[test]
fn character_state_round_trips() {
    let mut known = BTreeSet::new();
    known.insert("player_is_transfer".to_string());
    known.insert("met_namu".to_string());
    let mut overrides = BTreeMap::new();
    overrides.insert(
        CharacterId("c-namu".to_string()),
        "transfer-student".to_string(),
    );

    let cs = CharacterState {
        affection: 42,
        trust: -10,
        tension: 5,
        relationship_stage: RelationshipStage::CloseFriend,
        current_emotion: Emotion::Affectionate,
        known_facts: known,
        address_form_overrides: overrides,
    };
    round_trip_stable(&cs);

    // Spot-check a couple of fields survive with exact values.
    let json = serde_json::to_string(&cs).unwrap();
    let back: CharacterState = serde_json::from_str(&json).unwrap();
    assert_eq!(back.affection, 42);
    assert_eq!(back.trust, -10);
    assert_eq!(back.relationship_stage, RelationshipStage::CloseFriend);
    assert_eq!(back.current_emotion, Emotion::Affectionate);
    assert!(back.known_facts.contains("met_namu"));
    assert_eq!(
        back.address_form_overrides
            .get(&CharacterId("c-namu".to_string())),
        Some(&"transfer-student".to_string()),
    );
}

#[test]
fn choice_ref_round_trips() {
    let cr = ChoiceRef {
        choice_id: "ch-001".to_string(),
        label: "Greet Namu".to_string(),
        turn: 7,
    };
    round_trip_stable(&cr);
    let json = serde_json::to_string(&cr).unwrap();
    let back: ChoiceRef = serde_json::from_str(&json).unwrap();
    assert_eq!(back.choice_id, "ch-001");
    assert_eq!(back.label, "Greet Namu");
    assert_eq!(back.turn, 7);
}

#[test]
fn choice_round_trips_and_embeds_condition_expr() {
    let choice = Choice {
        id: "ch-tease".to_string(),
        label: "Tease Namu about her notes".to_string(),
        intent: "lighten the mood with a joke".to_string(),
        tone: Some("playful".to_string()),
        target: Some(CharacterId("c-namu".to_string())),
        risk: RiskLevel::Low,
        hidden_requirements: vec![
            ConditionExpr::AffectionAtLeast {
                character: CharacterId("c-namu".to_string()),
                value: 20,
            },
            ConditionExpr::All {
                children: vec![
                    ConditionExpr::FlagEquals {
                        id: "met_namu".to_string(),
                        value: true,
                    },
                    ConditionExpr::Not {
                        child: Box::new(ConditionExpr::EventOccurred {
                            id: "e-namu-angry".to_string(),
                        }),
                    },
                ],
            },
        ],
        preview: Some("Might make her laugh. Or scowl.".to_string()),
    };
    round_trip_stable(&choice);

    // The embedded ConditionExpr must serialize as a tagged object keyed by
    // `kind` (verified explicitly in W2.3 too).
    let json = serde_json::to_string(&choice).unwrap();
    assert!(
        json.contains("\"kind\":\"affection_at_least\""),
        "expected tagged condition in: {json}"
    );
    assert!(
        json.contains("\"kind\":\"all\""),
        "expected `all` tag in: {json}"
    );
}

#[test]
fn choice_with_all_optional_none_round_trips() {
    let choice = Choice {
        id: "ch-silent".to_string(),
        label: "Say nothing".to_string(),
        intent: "observe".to_string(),
        tone: None,
        target: None,
        risk: RiskLevel::Safe,
        hidden_requirements: Vec::new(),
        preview: None,
    };
    round_trip_stable(&choice);
    let json = serde_json::to_string(&choice).unwrap();
    let back: Choice = serde_json::from_str(&json).unwrap();
    assert!(back.hidden_requirements.is_empty());
    assert!(back.target.is_none());
}
