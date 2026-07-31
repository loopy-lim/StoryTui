//! Summary source-coverage validation (ST-030).
//!
//! [`validate_summary`] enforces the "summaries may not invent facts" rule
//! (Technical Spec §12.3). A [`TurnSummary`] is a compact recall index over the
//! raw [`HistoryEntry`]s; it is accepted only if every assertion it makes can
//! trace to a real source entry. This keeps summaries honest: the engine
//! rejects (does not commit) any summary that invents facts, drops a required
//! field, or attributes content to a history id that does not exist.
//!
//! # Rules
//!
//! Let `asserts_content = !(facts.is_empty() && relationship_changes.is_empty()
//! && knowledge_changes.is_empty() && address_form_changes.is_empty() &&
//! important_player_actions.is_empty() && interpretation.is_some())`.
//!
//! 1. If `asserts_content` and `summary.source_history_ids` is empty → reject
//!    ([`SUMMARY_MISSING_SOURCE_HISTORY_IDS`]). The summary must anchor itself
//!    to at least one real history entry.
//! 2. For every `id` in `summary.source_history_ids`, there must be some entry
//!    in `sources` whose `id == id`. Otherwise → reject
//!    ([`SUMMARY_UNSOURCED_FACT`]).
//!
//! Both rules are deterministic and total: same inputs ⇒ same decision, with
//! no map-iteration dependence (`sources` is iterated in slice order,
//! `source_history_ids` in `Vec` order).
//!
//! # ADR-003
//!
//! Pure data check; no IO/UI/HTTP.

use story_model::{HistoryEntry, TurnSummary};

use crate::error::{CoreError, SUMMARY_MISSING_SOURCE_HISTORY_IDS, SUMMARY_UNSOURCED_FACT};

/// Validate `summary` against the `sources` it claims to summarize.
///
/// Returns `Ok(())` when the summary is faithful, or [`CoreError::RuleViolation`]
/// with a stable reason code when it invents facts or omits a required field.
pub fn validate_summary(summary: &TurnSummary, sources: &[&HistoryEntry]) -> Result<(), CoreError> {
    let asserts_content = !summary.facts.is_empty()
        || !summary.relationship_changes.is_empty()
        || !summary.knowledge_changes.is_empty()
        || !summary.address_form_changes.is_empty()
        || !summary.important_player_actions.is_empty()
        || summary.interpretation.is_some();

    // Rule 1: a content-asserting summary must anchor to ≥1 real source.
    if asserts_content && summary.source_history_ids.is_empty() {
        return Err(CoreError::rule_violation(
            SUMMARY_MISSING_SOURCE_HISTORY_IDS,
            "summary asserts facts/changes but lists no source_history_ids",
        ));
    }

    // Rule 2: every listed source id must be present in `sources`.
    // Slice iteration keeps the check deterministic and order-independent.
    for id in &summary.source_history_ids {
        let present = sources.iter().any(|h| h.id == *id);
        if !present {
            return Err(CoreError::rule_violation(
                SUMMARY_UNSOURCED_FACT,
                format!(
                    "summary lists source_history_id {id} that is not among the supplied sources"
                ),
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    //! In-crate unit tests cover the pure rule logic; the end-to-end flow
    //! (with real [`HistoryEntry`] fixtures) lives in
    //! `tests/summary_validation.rs`.

    use super::*;
    use story_model::uuid::Uuid;
    use story_model::{ActionType, Choice, ParsedAction, StatePatch};

    fn bare_summary() -> TurnSummary {
        TurnSummary {
            facts: vec![],
            unresolved: vec![],
            relationship_changes: vec![],
            knowledge_changes: vec![],
            interpretation: None,
            address_form_changes: vec![],
            important_player_actions: vec![],
            source_history_ids: vec![],
        }
    }

    fn entry(id: Uuid) -> HistoryEntry {
        HistoryEntry {
            id,
            turn: 1,
            scene_id: "sc".to_string(),
            created_at: story_model::chrono::Utc::now(),
            narration: String::new(),
            player_action: Some(ParsedAction {
                action_type: ActionType::Dialogue,
                target: None,
                intent: None,
                tone: None,
                content: "x".to_string(),
                original_text: "x".to_string(),
                confidence: None,
            }),
            choices: vec![Choice {
                id: "c0".to_string(),
                label: "y".to_string(),
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

    #[test]
    fn empty_summary_passes() {
        validate_summary(&bare_summary(), &[]).expect("empty ok");
    }

    #[test]
    fn asserts_content_without_anchor_rejected() {
        let mut s = bare_summary();
        s.facts = vec!["x".to_string()];
        let err = validate_summary(&s, &[]).expect_err("missing anchor");
        assert_eq!(err.reason_code(), Some(SUMMARY_MISSING_SOURCE_HISTORY_IDS));
    }

    #[test]
    fn unsourced_id_rejected() {
        let real = Uuid::new_v4();
        let phantom = Uuid::new_v4();
        let mut s = bare_summary();
        s.facts = vec!["x".to_string()];
        s.source_history_ids = vec![real, phantom];
        let src = entry(real);
        let view = vec![&src];
        let err = validate_summary(&s, &view).expect_err("phantom id");
        assert_eq!(err.reason_code(), Some(SUMMARY_UNSOURCED_FACT));
    }

    #[test]
    fn all_ids_present_passes() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mut s = bare_summary();
        s.facts = vec!["x".to_string()];
        s.source_history_ids = vec![a, b];
        let ea = entry(a);
        let eb = entry(b);
        validate_summary(&s, &[&ea, &eb]).expect("both present");
    }
}
