//! Director plan validation + reducer dry-run (ST-033, W10 Part B).
//!
//! [`validate_plan`] is the ONLY entry point that converts a raw
//! Director-emitted [`DirectorPlan`] into a [`ValidatedPlan`] safe to feed to
//! the Writer stage. It runs five families of checks, ANY of which rejects the
//! whole plan and prevents the Writer from being called (ADR-005):
//!
//! 1. **Patch validation** — `proposed_patch` passes the W3 eight-stage
//!    validator (reference/range/permission/transition) via
//!    [`crate::validator::validate`].
//! 2. **Patch dry-run** — the sanitized patch is applied to a CLONE of the
//!    state via [`crate::reducer::apply_patch`]; any apply-time failure (e.g.
//!    an event effect whose delta goes out of range) rejects the plan. The
//!    resulting [`ApplyResult`] is returned to the engine so the Writer stage
//!    never re-judges the patch.
//! 3. **Reveal disjoint + knowledge scope** — `must_not_reveal_fact_ids` and
//!    `reveal_fact_ids` are disjoint; additionally `reveal_fact_ids` must not
//!    target any character's declared `unknown_facts` (a character must not
//!    voice a secret the Pack says they do not know).
//! 4. **Emotion arcs** — every arc references a character present in
//!    `state.characters`.
//! 5. **Choices + end condition** — choice count is in `[1, 6]` with non-empty
//!    `id`/`label`/`intent`; `end_condition` is non-empty.
//!
//! # ADR-005 (load-bearing)
//!
//! A rejected plan means the Writer is NEVER called for this turn — the host
//! surfaces a recoverable error and the committed stable state is left
//! byte-identical. This is the contract that keeps the optional 2-stage path
//! at least as safe as the MVP single-call path.

use story_model::uuid::Uuid;
use story_model::{DirectorPlan, GameState, StoryPack};

use crate::error::{
    CoreError, DIRECTOR_DRY_RUN_FAILED, DIRECTOR_EMPTY_END_CONDITION, DIRECTOR_INVALID_CHOICE,
    DIRECTOR_REVEAL_CONFLICT, DIRECTOR_UNKNOWN_CHARACTER,
};
use crate::reducer::{ApplyResult, apply_patch};
use crate::validator::validate;

/// A Director plan that has passed ALL validation + dry-run checks.
///
/// Carries the sanitized plan plus the confirmed [`ApplyResult`] from the
/// reducer dry-run, so the engine can commit it directly without re-applying
/// (the patch has already been confirmed to apply cleanly to the checkpoint
/// state). The Writer stage receives a digest of `plan` and emits narration
/// ONLY — it can never alter the confirmed patch or choices.
#[derive(Clone, Debug)]
pub struct ValidatedPlan {
    /// The sanitized Director plan (unchanged on success). Load-bearing for
    /// the engine's commit step, which uses `plan.proposed_patch` and
    /// `plan.choices` rather than any Writer-emitted shape.
    pub plan: DirectorPlan,
    /// The dry-run apply result against the live `GameState`. The engine uses
    /// `dry_run.state` as the post-turn committed state — no second apply.
    pub dry_run: ApplyResult,
}

/// Validate + dry-run a Director plan.
///
/// On `Err`, the caller's `GameState` is byte-identical to its pre-call self
/// (the dry-run mutates only a clone), and the host MUST NOT invoke the
/// Writer. On `Ok`, the returned [`ValidatedPlan`] is safe to commit.
pub fn validate_plan(
    plan: &DirectorPlan,
    state: &GameState,
    pack: &StoryPack,
) -> Result<ValidatedPlan, CoreError> {
    let rules = &pack.rules;
    // The validator does not consult the generation_id (it is `_generation_id`
    // in the signature); the engine stamps the real id at commit time. We use
    // a stable nil id so the dry-run change-log entries are deterministic.
    let dry_id = Uuid::nil();

    // (1) W3 eight-stage validator: structural / reference / range /
    //     permission / transition. Hard failure ⇒ reject the plan.
    validate(state, rules, &plan.proposed_patch, dry_id)?;

    // (2) Reducer dry-run on a clone (atomicity: caller's state untouched).
    //     Catches apply-time failures the static validator cannot, e.g. an
    //     event effect pushing a stat out of range.
    let dry_run = apply_patch(state, rules, &plan.proposed_patch, dry_id).map_err(|e| {
        CoreError::rule_violation(
            DIRECTOR_DRY_RUN_FAILED,
            format!("reducer dry-run rejected the plan: {e}"),
        )
    })?;

    // (3) Reveal disjoint + knowledge scope.
    check_reveal_disjoint_and_scope(plan, pack)?;

    // (4) Emotion arcs reference existing characters.
    check_emotion_arcs(plan, state)?;

    // (5) Choices + end-condition shape.
    check_choices_and_end(plan)?;

    Ok(ValidatedPlan {
        plan: plan.clone(),
        dry_run,
    })
}

// ---------------------------------------------------------------------------
// Individual check families
// ---------------------------------------------------------------------------

/// Disjoint reveals + knowledge-scope: a fact the Director authorizes revealing
/// must not also be forbidden, and must not be a declared `unknown_fact` of any
/// character (no voicing secrets the character does not know).
fn check_reveal_disjoint_and_scope(plan: &DirectorPlan, pack: &StoryPack) -> Result<(), CoreError> {
    // Disjoint: reveal_fact_ids ∩ must_not_reveal_fact_ids == ∅.
    for forbidden in &plan.must_not_reveal_fact_ids {
        if plan.reveal_fact_ids.contains(forbidden) {
            return Err(CoreError::rule_violation(
                DIRECTOR_REVEAL_CONFLICT,
                format!(
                    "fact {forbidden} appears in both reveal_fact_ids and must_not_reveal_fact_ids"
                ),
            ));
        }
    }
    // Knowledge scope: a revealed fact must not be a declared unknown_fact of
    // any character — otherwise the narration would voice a secret the
    // character does not know.
    for fact in &plan.reveal_fact_ids {
        for cdef in pack.characters.values() {
            if cdef.unknown_facts.iter().any(|f| f == fact) {
                return Err(CoreError::rule_violation(
                    DIRECTOR_REVEAL_CONFLICT,
                    format!(
                        "reveal_fact_ids contains {fact} which is declared unknown to character {} — knowledge-scope violation",
                        cdef.id.0
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Every emotion arc references a character present in the live state.
fn check_emotion_arcs(plan: &DirectorPlan, state: &GameState) -> Result<(), CoreError> {
    for ch in plan.emotion_arcs.keys() {
        if !state.characters.contains_key(ch) {
            return Err(CoreError::rule_violation(
                DIRECTOR_UNKNOWN_CHARACTER,
                format!("emotion_arcs references unknown character {}", ch.0),
            ));
        }
    }
    Ok(())
}

/// Choices count in `[1, 6]` with non-empty id/label/intent; `end_condition`
/// non-empty.
fn check_choices_and_end(plan: &DirectorPlan) -> Result<(), CoreError> {
    let n = plan.choices.len();
    if !(1..=6).contains(&n) {
        return Err(CoreError::rule_violation(
            DIRECTOR_INVALID_CHOICE,
            format!("choices count out of range [1, 6]: got {n}"),
        ));
    }
    for (i, c) in plan.choices.iter().enumerate() {
        if c.id.trim().is_empty() || c.label.trim().is_empty() || c.intent.trim().is_empty() {
            return Err(CoreError::rule_violation(
                DIRECTOR_INVALID_CHOICE,
                format!("choice[{i}] has empty id/label/intent"),
            ));
        }
    }
    if plan.end_condition.trim().is_empty() {
        return Err(CoreError::rule_violation(
            DIRECTOR_EMPTY_END_CONDITION,
            "end_condition must be non-empty",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_support::sample_pack;
    use story_model::{
        CharacterState, Choice, Emotion, EmotionArc, RelationshipStage, RiskLevel, StatePatch,
    };

    fn namu_state() -> GameState {
        // Build a live game state mirroring the demo pack: one character,
        // zeroed affection/trust/tension at Acquaintance.
        let mut chars = std::collections::BTreeMap::new();
        chars.insert(
            story_model::CharacterId("c-namu".to_string()),
            CharacterState {
                affection: 0,
                trust: 0,
                tension: 0,
                relationship_stage: RelationshipStage::Acquaintance,
                current_emotion: Emotion::Neutral,
                known_facts: Default::default(),
                address_form_overrides: Default::default(),
            },
        );
        GameState {
            save_schema_version: story_model::SAVE_SCHEMA_VERSION,
            story_id: "demo".to_string(),
            story_version: semver::Version::new(0, 1, 0),
            pack_schema_version: semver::Version::new(1, 0, 0),
            session_id: uuid::Uuid::nil(),
            current_chapter: "chapter-1".to_string(),
            current_scene_id: "scene-1".to_string(),
            turn: 0,
            location: story_model::LocationId("loc-classroom".to_string()),
            time_slot: story_model::TimeSlot::Morning,
            active_route: None,
            characters: chars,
            flags: Default::default(),
            numbers: Default::default(),
            strings: Default::default(),
            inventory: Default::default(),
            occurred_events: Default::default(),
            recent_summaries: Default::default(),
            memories: Vec::new(),
            history: Vec::new(),
            last_choice: None,
            revision: 0,
            stable_generation_id: None,
            lorebook_activations: Vec::new(),
        }
    }

    fn choice(id: &str) -> Choice {
        Choice {
            id: id.to_string(),
            label: "label".to_string(),
            intent: "intent".to_string(),
            tone: None,
            target: None,
            risk: RiskLevel::Safe,
            hidden_requirements: vec![],
            preview: None,
        }
    }

    fn valid_plan() -> DirectorPlan {
        let mut patch = StatePatch::default();
        patch
            .affection_delta
            .insert(story_model::CharacterId("c-namu".to_string()), 3);
        let mut arcs = std::collections::BTreeMap::new();
        arcs.insert(
            story_model::CharacterId("c-namu".to_string()),
            EmotionArc {
                character: story_model::CharacterId("c-namu".to_string()),
                start: Emotion::Neutral,
                end: Emotion::Happy,
                reason: None,
            },
        );
        DirectorPlan {
            scene_goal: "scene".to_string(),
            required_beats: vec!["beat".to_string()],
            emotion_arcs: arcs,
            character_outfits: Default::default(),
            ambient_situation: None,
            reveal_fact_ids: vec!["fact-a".to_string()],
            must_not_reveal_fact_ids: vec!["fact-b".to_string()],
            proposed_patch: patch,
            choices: vec![choice("c1"), choice("c2")],
            conflict_checks: vec![],
            end_condition: "continue".to_string(),
        }
    }

    #[test]
    fn valid_plan_dry_runs_and_returns_apply_result() {
        let state = namu_state();
        let pack = sample_pack();
        let plan = valid_plan();
        let v = validate_plan(&plan, &state, &pack).expect("plan should validate");
        // Dry-run bumped revision exactly once on the cloned state.
        assert_eq!(v.dry_run.state.revision, state.revision + 1);
        // Affection delta was applied.
        let applied = v
            .dry_run
            .state
            .characters
            .get(&story_model::CharacterId("c-namu".to_string()))
            .expect("character present")
            .affection;
        assert_eq!(applied, 3);
        // Caller's state is byte-identical (untouched clone).
        assert_eq!(
            state
                .characters
                .get(&story_model::CharacterId("c-namu".to_string()))
                .expect("present")
                .affection,
            0
        );
    }

    #[test]
    fn unknown_patch_reference_is_dropped_so_writer_still_called() {
        // affection_delta on an unknown character: dropped (with audit), the
        // same policy as the single-pass path — one stray key must not kill
        // the whole two-stage pipeline.
        let state = namu_state();
        let pack = sample_pack();
        let mut plan = valid_plan();
        plan.proposed_patch = StatePatch::default();
        plan.proposed_patch
            .affection_delta
            .insert(story_model::CharacterId("c-ghost".to_string()), 5);
        let validated = validate_plan(&plan, &state, &pack).expect("plan survives the drop");
        let warning = validated
            .dry_run
            .warnings
            .iter()
            .find(|w| w.code == "patch_unknown_character")
            .expect("drop is audited as a warning");
        assert!(
            warning.detail.contains("c-ghost"),
            "detail names the dropped key: {}",
            warning.detail
        );
        assert!(
            !validated
                .dry_run
                .state
                .characters
                .contains_key(&story_model::CharacterId("c-ghost".to_string())),
            "no character was invented by the dropped delta"
        );
    }

    #[test]
    fn forbidden_reveal_overlap_is_rejected() {
        let state = namu_state();
        let pack = sample_pack();
        let mut plan = valid_plan();
        // Same fact id in both lists.
        plan.reveal_fact_ids = vec!["fact-shared".to_string()];
        plan.must_not_reveal_fact_ids = vec!["fact-shared".to_string()];
        let err = validate_plan(&plan, &state, &pack).expect_err("must reject");
        assert_eq!(err.reason_code(), Some(DIRECTOR_REVEAL_CONFLICT));
    }

    #[test]
    fn reveal_targetting_unknown_fact_is_rejected() {
        let state = namu_state();
        let mut pack = sample_pack();
        // Declare a secret the character does not know.
        let cdef = pack
            .characters
            .get_mut(&story_model::CharacterId("c-namu".to_string()))
            .expect("character");
        cdef.unknown_facts.push("fact-secret".to_string());

        let mut plan = valid_plan();
        plan.reveal_fact_ids = vec!["fact-secret".to_string()];
        plan.must_not_reveal_fact_ids = vec![];
        let err = validate_plan(&plan, &state, &pack).expect_err("must reject");
        assert_eq!(
            err.reason_code(),
            Some(DIRECTOR_REVEAL_CONFLICT),
            "knowledge-scope violation must be rejected"
        );
    }

    #[test]
    fn emotion_arc_unknown_character_is_rejected() {
        let state = namu_state();
        let pack = sample_pack();
        let mut plan = valid_plan();
        plan.emotion_arcs.insert(
            story_model::CharacterId("c-ghost".to_string()),
            EmotionArc {
                character: story_model::CharacterId("c-ghost".to_string()),
                start: Emotion::Neutral,
                end: Emotion::Happy,
                reason: None,
            },
        );
        let err = validate_plan(&plan, &state, &pack).expect_err("must reject");
        assert_eq!(err.reason_code(), Some(DIRECTOR_UNKNOWN_CHARACTER));
    }

    #[test]
    fn empty_end_condition_is_rejected() {
        let state = namu_state();
        let pack = sample_pack();
        let mut plan = valid_plan();
        plan.end_condition = "   ".to_string();
        let err = validate_plan(&plan, &state, &pack).expect_err("must reject");
        assert_eq!(err.reason_code(), Some(DIRECTOR_EMPTY_END_CONDITION));
    }

    #[test]
    fn choices_count_out_of_range_is_rejected() {
        let state = namu_state();
        let pack = sample_pack();
        let mut plan = valid_plan();
        plan.choices = vec![]; // 0 choices — out of range
        let err = validate_plan(&plan, &state, &pack).expect_err("must reject");
        assert_eq!(err.reason_code(), Some(DIRECTOR_INVALID_CHOICE));
    }

    #[test]
    fn choice_with_empty_label_is_rejected() {
        let state = namu_state();
        let pack = sample_pack();
        let mut plan = valid_plan();
        plan.choices = vec![choice("c1")];
        plan.choices[0].label = "  ".to_string();
        let err = validate_plan(&plan, &state, &pack).expect_err("must reject");
        assert_eq!(err.reason_code(), Some(DIRECTOR_INVALID_CHOICE));
    }
}
