//! Truth-table and determinism tests for the condition DSL evaluator (W3.2).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use story_core::eval::{EvalContext, evaluate};
use story_model::{
    CharacterId, CharacterState, CompareOp, ConditionExpr, Emotion, GameState, LocationId,
    NumberVarDef, RelationshipStage, SAVE_SCHEMA_VERSION, StoryRules,
};

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
    flags.insert("f-brave".to_string(), false);
    let mut numbers = BTreeMap::new();
    numbers.insert("score".to_string(), 7);
    let mut occurred = BTreeSet::new();
    occurred.insert("e-intro".to_string());

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
        inventory: BTreeMap::new(),
        occurred_events: occurred,
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
    rules.numbers.insert(
        "score".to_string(),
        NumberVarDef {
            id: "score".to_string(),
            min: 0,
            max: 100,
            default: 50,
        },
    );
    rules.numbers.insert(
        "absent_with_default".to_string(),
        NumberVarDef {
            id: "absent_with_default".to_string(),
            min: 0,
            max: 100,
            default: 42,
        },
    );
    rules
}

#[test]
fn all_requires_every_child_true_and_accepts_empty() {
    let st = fresh_state();
    let rules = fresh_rules();
    let ctx = EvalContext {
        state: &st,
        rules: &rules,
    };
    let t = ConditionExpr::All {
        children: vec![
            ConditionExpr::AffectionAtLeast {
                character: CharacterId("c-namu".to_string()),
                value: 20,
            },
            ConditionExpr::FlagEquals {
                id: "met_namu".to_string(),
                value: true,
            },
        ],
    };
    assert!(evaluate(&t, &ctx));

    let f = ConditionExpr::All {
        children: vec![
            ConditionExpr::AffectionAtLeast {
                character: CharacterId("c-namu".to_string()),
                value: 20,
            },
            ConditionExpr::FlagEquals {
                id: "met_namu".to_string(),
                value: false,
            },
        ],
    };
    assert!(!evaluate(&f, &ctx));

    // Empty conjunction is vacuously true.
    assert!(evaluate(&ConditionExpr::All { children: vec![] }, &ctx));
}

#[test]
fn any_requires_some_child_true_and_rejects_empty() {
    let st = fresh_state();
    let rules = fresh_rules();
    let ctx = EvalContext {
        state: &st,
        rules: &rules,
    };
    let t = ConditionExpr::Any {
        children: vec![
            ConditionExpr::AffectionAtLeast {
                character: CharacterId("c-namu".to_string()),
                value: 999,
            },
            ConditionExpr::TrustAtLeast {
                character: CharacterId("c-namu".to_string()),
                value: 5,
            },
        ],
    };
    assert!(evaluate(&t, &ctx));

    let f = ConditionExpr::Any {
        children: vec![
            ConditionExpr::AffectionAtLeast {
                character: CharacterId("c-namu".to_string()),
                value: 999,
            },
            ConditionExpr::TrustAtLeast {
                character: CharacterId("c-namu".to_string()),
                value: 999,
            },
        ],
    };
    assert!(!evaluate(&f, &ctx));

    // Empty disjunction is false.
    assert!(!evaluate(&ConditionExpr::Any { children: vec![] }, &ctx));
}

#[test]
fn not_inverts_child() {
    let st = fresh_state();
    let rules = fresh_rules();
    let ctx = EvalContext {
        state: &st,
        rules: &rules,
    };
    let n = ConditionExpr::Not {
        child: Box::new(ConditionExpr::AffectionAtLeast {
            character: CharacterId("c-namu".to_string()),
            value: 999,
        }),
    };
    assert!(evaluate(&n, &ctx));
}

#[test]
fn flag_equals_treats_absent_as_false() {
    let st = fresh_state();
    let rules = fresh_rules();
    let ctx = EvalContext {
        state: &st,
        rules: &rules,
    };
    // met_namu=true → true
    assert!(evaluate(
        &ConditionExpr::FlagEquals {
            id: "met_namu".to_string(),
            value: true,
        },
        &ctx,
    ));
    // f-brave=false → true
    assert!(evaluate(
        &ConditionExpr::FlagEquals {
            id: "f-brave".to_string(),
            value: false,
        },
        &ctx,
    ));
    // missing flag matches value==false
    assert!(evaluate(
        &ConditionExpr::FlagEquals {
            id: "never_set".to_string(),
            value: false,
        },
        &ctx,
    ));
    // missing flag does NOT match value==true
    assert!(!evaluate(
        &ConditionExpr::FlagEquals {
            id: "never_set".to_string(),
            value: true,
        },
        &ctx,
    ));
}

#[test]
fn number_compare_uses_state_or_rule_default_or_zero() {
    let st = fresh_state();
    let rules = fresh_rules();
    let ctx = EvalContext {
        state: &st,
        rules: &rules,
    };
    // score is 7 in state.
    assert!(evaluate(
        &ConditionExpr::NumberCompare {
            id: "score".to_string(),
            op: CompareOp::Eq,
            value: 7,
        },
        &ctx,
    ));
    assert!(evaluate(
        &ConditionExpr::NumberCompare {
            id: "score".to_string(),
            op: CompareOp::Gt,
            value: 6,
        },
        &ctx,
    ));
    // absent_with_default is missing in state; rule default = 42.
    assert!(evaluate(
        &ConditionExpr::NumberCompare {
            id: "absent_with_default".to_string(),
            op: CompareOp::Eq,
            value: 42,
        },
        &ctx,
    ));
    // totally unknown id → 0.
    assert!(evaluate(
        &ConditionExpr::NumberCompare {
            id: "totally_unknown".to_string(),
            op: CompareOp::Eq,
            value: 0,
        },
        &ctx,
    ));
}

#[test]
fn affection_and_trust_at_least_read_character_state_with_default_zero_for_missing() {
    let st = fresh_state();
    let rules = fresh_rules();
    let ctx = EvalContext {
        state: &st,
        rules: &rules,
    };
    // c-namu has affection=25, trust=10.
    assert!(evaluate(
        &ConditionExpr::AffectionAtLeast {
            character: CharacterId("c-namu".to_string()),
            value: 25,
        },
        &ctx,
    ));
    assert!(!evaluate(
        &ConditionExpr::AffectionAtLeast {
            character: CharacterId("c-namu".to_string()),
            value: 26,
        },
        &ctx,
    ));
    assert!(evaluate(
        &ConditionExpr::TrustAtLeast {
            character: CharacterId("c-namu".to_string()),
            value: 10,
        },
        &ctx,
    ));
    // Missing character is treated as 0.
    assert!(evaluate(
        &ConditionExpr::AffectionAtLeast {
            character: CharacterId("c-ghost".to_string()),
            value: 0,
        },
        &ctx,
    ));
    assert!(!evaluate(
        &ConditionExpr::AffectionAtLeast {
            character: CharacterId("c-ghost".to_string()),
            value: 1,
        },
        &ctx,
    ));
}

#[test]
fn event_occurred_and_location_and_turn_checks() {
    let st = fresh_state();
    let rules = fresh_rules();
    let ctx = EvalContext {
        state: &st,
        rules: &rules,
    };
    assert!(evaluate(
        &ConditionExpr::EventOccurred {
            id: "e-intro".to_string(),
        },
        &ctx,
    ));
    assert!(!evaluate(
        &ConditionExpr::EventOccurred {
            id: "e-missing".to_string(),
        },
        &ctx,
    ));
    assert!(evaluate(
        &ConditionExpr::AtLocation {
            location: LocationId("loc-classroom".to_string()),
        },
        &ctx,
    ));
    assert!(!evaluate(
        &ConditionExpr::AtLocation {
            location: LocationId("loc-roof".to_string()),
        },
        &ctx,
    ));
    assert!(evaluate(&ConditionExpr::TurnAtLeast { value: 6 }, &ctx,));
    assert!(!evaluate(&ConditionExpr::TurnAtLeast { value: 7 }, &ctx,));
}

#[test]
fn nested_all_any_not_evaluates() {
    let st = fresh_state();
    let rules = fresh_rules();
    let ctx = EvalContext {
        state: &st,
        rules: &rules,
    };
    // All(Any(FlagEquals met_namu=true, Not(TurnAtLeast 999)), AffectionAtLeast 25)
    let expr = ConditionExpr::All {
        children: vec![
            ConditionExpr::Any {
                children: vec![
                    ConditionExpr::FlagEquals {
                        id: "met_namu".to_string(),
                        value: true,
                    },
                    ConditionExpr::Not {
                        child: Box::new(ConditionExpr::TurnAtLeast { value: 999 }),
                    },
                ],
            },
            ConditionExpr::AffectionAtLeast {
                character: CharacterId("c-namu".to_string()),
                value: 25,
            },
        ],
    };
    assert!(evaluate(&expr, &ctx));
}

#[test]
fn evaluate_is_deterministic_across_repeated_calls() {
    let st = fresh_state();
    let rules = fresh_rules();
    let expr = ConditionExpr::All {
        children: vec![
            ConditionExpr::Any {
                children: vec![
                    ConditionExpr::NumberCompare {
                        id: "score".to_string(),
                        op: CompareOp::Ge,
                        value: 5,
                    },
                    ConditionExpr::Not {
                        child: Box::new(ConditionExpr::FlagEquals {
                            id: "f-brave".to_string(),
                            value: true,
                        }),
                    },
                ],
            },
            ConditionExpr::AtLocation {
                location: LocationId("loc-classroom".to_string()),
            },
        ],
    };
    let ctx = EvalContext {
        state: &st,
        rules: &rules,
    };
    let first = evaluate(&expr, &ctx);
    for _ in 0..100 {
        assert_eq!(
            evaluate(&expr, &ctx),
            first,
            "evaluator must be deterministic"
        );
    }
}
