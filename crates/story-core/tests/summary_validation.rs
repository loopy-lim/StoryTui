//! Integration tests for ST-030: summary source-coverage validation.
//!
//! [`story_core::summary::validate_summary`] enforces the "summaries may not
//! invent facts" rule: every fact/relationship-change/knowledge-change in a
//! [`TurnSummary`] must trace to a `source_history_id` that is present in the
//! supplied source [`HistoryEntry`]s. Missing required fields and unsourced
//! assertions are rejected.

use story_core::summary::validate_summary;
use story_model::chrono::Utc;
use story_model::uuid::Uuid;
use story_model::{
    Choice, HistoryEntry, ParsedAction, StatePatch, TurnSummary, ActionType,
};

fn entry(id: Uuid, turn: u32) -> HistoryEntry {
    HistoryEntry {
        id,
        turn,
        scene_id: "sc-1".to_string(),
        created_at: Utc::now(),
        narration: "장면".to_string(),
        player_action: Some(ParsedAction {
            action_type: ActionType::Dialogue,
            target: None,
            intent: None,
            tone: None,
            content: "말한다".to_string(),
            original_text: "말한다".to_string(),
            confidence: None,
        }),
        choices: vec![Choice {
            id: "c0".to_string(),
            label: "계속".to_string(),
            intent: String::new(),
            tone: None,
            target: None,
            risk: story_model::RiskLevel::Safe,
            hidden_requirements: vec![],
            preview: None,
        }],
        applied_patch: StatePatch::default(),
        generation_id: Uuid::nil(),
    }
}

fn summary_with(sources: Vec<Uuid>) -> TurnSummary {
    TurnSummary {
        facts: vec!["유나가 편지를 인정했다".to_string()],
        unresolved: vec!["왜 떠났는지 아직 모른다".to_string()],
        relationship_changes: vec!["유나 → 친구".to_string()],
        knowledge_changes: vec!["player가 사실을 앎".to_string()],
        interpretation: Some("재회의 전환점".to_string()),
        address_form_changes: vec![],
        important_player_actions: vec![],
        source_history_ids: sources,
    }
}

#[test]
fn faithful_summary_with_valid_sources_passes() {
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let sources = [entry(a, 1), entry(b, 2)];
    let view: Vec<&HistoryEntry> = sources.iter().collect();
    let s = summary_with(vec![a, b]);
    validate_summary(&s, &view).expect("a faithful summary with both sources present passes");
}

#[test]
fn summary_with_unsourced_fact_is_rejected() {
    let a = Uuid::new_v4();
    let unknown = Uuid::new_v4(); // not in sources
    let sources = [entry(a, 1)];
    let view: Vec<&HistoryEntry> = sources.iter().collect();
    let s = summary_with(vec![a, unknown]);
    let err = validate_summary(&s, &view).expect_err("unsourced id must be rejected");
    assert_eq!(err.reason_code(), Some("summary_unsourced_fact"));
}

#[test]
fn summary_with_no_sources_but_asserting_facts_is_rejected() {
    // The summary asserts facts/relationship-change/knowledge-change but lists
    // no source history ids at all -> missing required anchor.
    let s = summary_with(vec![]);
    let err = validate_summary(&s, &[]).expect_err("empty source_history_ids must be rejected");
    assert_eq!(err.reason_code(), Some("summary_missing_source_history_ids"));
}

#[test]
fn empty_summary_with_no_assertions_and_no_sources_is_accepted() {
    // A calm turn that asserts nothing needs no source anchor.
    let s = TurnSummary {
        facts: vec![],
        unresolved: vec![],
        relationship_changes: vec![],
        knowledge_changes: vec![],
        interpretation: None,
        address_form_changes: vec![],
        important_player_actions: vec![],
        source_history_ids: vec![],
    };
    validate_summary(&s, &[]).expect("an empty summary asserts nothing and is accepted");
}

#[test]
fn address_form_change_cause_preserved_when_sourced() {
    let a = Uuid::new_v4();
    let sources = [entry(a, 1)];
    let view: Vec<&HistoryEntry> = sources.iter().collect();
    let mut s = summary_with(vec![a]);
    s.address_form_changes = vec!["유나가 이름+씨 → 반말로 전환".to_string()];
    validate_summary(&s, &view).expect("sourced address-form change passes");
}

#[test]
fn subset_of_sources_is_accepted_as_long_as_all_listed_ids_resolve() {
    // The summary lists one of the available sources; that's fine — every
    // listed id resolves.
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let sources = [entry(a, 1), entry(b, 2)];
    let view: Vec<&HistoryEntry> = sources.iter().collect();
    let s = summary_with(vec![a]);
    validate_summary(&s, &view).expect("subset of sources still covered");
}
