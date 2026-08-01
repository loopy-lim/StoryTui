//! Integration tests for the engine command/effect boundary (ST-020):
//! [`story_core::engine::reduce`] + [`story_core::engine_cmd`].
//!
//! Builds a hand-authored [`StoryPack`] + [`EngineState`] (no loader, no LLM)
//! and pins the five invariants:
//!
//! 1. Late/mismatched `AcceptGeneration` never mutates `game`.
//! 2. A failed `apply_patch` restores the checkpoint byte-identically.
//! 3. `CancelGeneration` restores the checkpoint.
//! 4. A second `SubmitChoice` while a generation is pending is rejected.
//! 5. Turn increments by exactly 1 per accepted generation.

use std::collections::BTreeMap;
use std::path::PathBuf;

use story_core::engine::{EngineState, Transition, commit_turn, reduce};
use story_core::engine_cmd::{EngineCommand, EngineEffect};
use story_core::reducer::ApplyResult;
use story_model::{
    AppViewModel, CharacterDef, CharacterId, Choice, ChoiceView, ConditionExpr, DomainErrorView,
    Emotion, GeneratedTurnDraft, LocationDef, LocationId, PackContentPolicy, PackInitialState,
    PackMetadata, PackRating, PromptFragments, RelationshipStage, RiskLevel, SAVE_SCHEMA_VERSION,
    Screen, StatePatch, StoryPack, StoryRules, TimeSlot, TurnSummary,
};

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

const NAMU: &str = "c-namu";

fn cid() -> CharacterId {
    CharacterId(NAMU.to_string())
}

fn sample_pack() -> StoryPack {
    let metadata = PackMetadata {
        schema_version: semver::Version::parse("1.0.0").unwrap(),
        story_id: "demo".to_string(),
        story_version: semver::Version::new(0, 1, 0),
        min_engine_version: semver::Version::new(0, 1, 0),
        title: "데모 이야기".to_string(),
        language: "ko-KR".to_string(),
        genres: vec!["drama".to_string()],
        rating: PackRating::General,
        all_major_characters_safe: true,
        default_choice_count: 3,
        initial_state: PackInitialState {
            chapter: "chapter-1".to_string(),
            scene: "scene-1".to_string(),
            location: LocationId("loc-classroom".to_string()),
            time_slot: TimeSlot::Morning,
        },
        content: PackContentPolicy {
            safe_content: "none".to_string(),
            violence: "none".to_string(),
        },
    };

    let character = CharacterDef {
        id: cid(),
        name: "나무".to_string(),
        age: Some(28),
        pronouns: None,
        personality: vec![],
        appearance: vec![],
        values: vec![],
        likes: vec![],
        dislikes: vec![],
        secrets: vec![],
        initial_player_relationship: RelationshipStage::Acquaintance,
        speech_style: String::new(),
        honorific_rule: String::new(),
        frequent_phrases: vec![],
        avoid_phrases: vec![],
        known_facts: vec![],
        unknown_facts: vec![],
        current_goal: String::new(),
        long_term_desire: String::new(),
    };

    let mut characters = BTreeMap::new();
    characters.insert(cid(), character);

    let mut rules = StoryRules::minimal();
    rules.locations.insert(
        LocationId("loc-classroom".to_string()),
        LocationDef {
            id: LocationId("loc-classroom".to_string()),
            display_name: "교실".to_string(),
            adjacent: Default::default(),
        },
    );
    rules
        .allowed_flags
        .extend(["f-brave", "f-met"].map(String::from));

    StoryPack {
        metadata,
        characters,
        rules,
        world_md: String::new(),
        style_md: String::new(),
        opening_md: "오프닝: 비가 그쳤다.".to_string(),
        prompts: PromptFragments {
            system: "sys".to_string(),
            director: "d".to_string(),
            writer: "w".to_string(),
        },
        pack_root: PathBuf::from("/tmp/demo"),
    }
}

/// A draft whose patch applies cleanly against the fixture rules
/// (`affection +1` for the known character).
fn valid_draft(generation_id: uuid::Uuid, affection_delta: i16) -> GeneratedTurnDraft {
    let mut patch = StatePatch::default();
    patch.affection_delta.insert(cid(), affection_delta);
    GeneratedTurnDraft {
        generation_id,
        narration: format!("유나가 고개를 끄덕인다 (affection {affection_delta:+})."),
        choices: vec![
            choice("choice-0", "다시 만난다", RiskLevel::Low),
            choice("choice-1", "돌아선다", RiskLevel::Medium),
            choice("choice-2", "기다린다", RiskLevel::Safe),
        ],
        proposed_patch: patch,
        summary: TurnSummary {
            facts: vec![],
            unresolved: vec![],
            relationship_changes: vec![],
            knowledge_changes: vec![],
            interpretation: None,
            address_form_changes: vec![],
            important_player_actions: vec![],
            source_history_ids: vec![],
        },
        usage: None,
    }
}

/// A draft whose patch references an unknown flag → rejected by the validator.
fn malformed_draft(generation_id: uuid::Uuid) -> GeneratedTurnDraft {
    let mut patch = StatePatch::default();
    patch.set_flags.insert("nonexistent_flag".to_string(), true);
    GeneratedTurnDraft {
        generation_id,
        narration: "버려진 장면.".to_string(),
        choices: vec![choice("choice-0", "계속", RiskLevel::Safe)],
        proposed_patch: patch,
        summary: TurnSummary {
            facts: vec![],
            unresolved: vec![],
            relationship_changes: vec![],
            knowledge_changes: vec![],
            interpretation: None,
            address_form_changes: vec![],
            important_player_actions: vec![],
            source_history_ids: vec![],
        },
        usage: None,
    }
}

fn choice(id: &str, label: &str, risk: RiskLevel) -> Choice {
    Choice {
        id: id.to_string(),
        label: label.to_string(),
        intent: String::new(),
        tone: None,
        target: None,
        risk,
        hidden_requirements: vec![],
        preview: None,
    }
}

/// Drive `StartNewGame`, fish out the `RequestGeneration` id, return it.
fn start_and_pending_id(pack: &StoryPack) -> (EngineState, uuid::Uuid) {
    let init = EngineState::initial(pack);
    let t = reduce(&init, EngineCommand::StartNewGame, pack).expect("start");
    let gid = t
        .effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::RequestGeneration { generation_id, .. } => Some(*generation_id),
            _ => None,
        })
        .expect("StartNewGame emits RequestGeneration");
    (t.state, gid)
}

fn assert_publish_view_present(t: &Transition) {
    assert!(
        t.effects
            .iter()
            .any(|e| matches!(e, EngineEffect::PublishView(_))),
        "expected a PublishView effect, got {:?}",
        t.effects
    );
}

// ---------------------------------------------------------------------------
// happy path + invariant 5 (turn bump)
// ---------------------------------------------------------------------------

#[test]
fn start_new_game_publishes_opening_and_requests_generation() {
    let pack = sample_pack();
    let (state, _gid) = start_and_pending_id(&pack);
    assert!(state.pending.is_some(), "pending set after StartNewGame");
    assert_eq!(state.game.turn, 0, "turn starts at 0");
    assert_eq!(
        state
            .game
            .characters
            .get(&cid())
            .unwrap()
            .relationship_stage,
        RelationshipStage::Acquaintance,
        "initial relationship stage from character def"
    );
}

#[test]
fn accepted_opening_generation_bumps_turn_and_offers_choices() {
    let pack = sample_pack();
    let (state, gid) = start_and_pending_id(&pack);

    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid, 1)),
        },
        &pack,
    )
    .expect("accept opening");

    assert!(t.state.pending.is_none(), "pending cleared on accept");
    assert_eq!(t.state.game.turn, 1, "turn bumped to 1 (invariant 5)");
    assert_eq!(
        t.state.game.characters.get(&cid()).unwrap().affection,
        1,
        "patch applied"
    );
    assert_eq!(
        t.state.game.history.len(),
        1,
        "exactly one history entry after the opening turn"
    );
    let view = published_view(&t);
    assert_eq!(view.choices.len(), 3, "opening offers 3 choices");
    assert_eq!(view.narration, "유나가 고개를 끄덕인다 (affection +1).");
    assert_eq!(view.status.turn, 1);
    assert_eq!(view.status.affection, Some(1));
    assert_eq!(view.screen, Screen::Story);
    assert_eq!(view.story_title, "데모 이야기");
    assert_eq!(view.location_display, "교실");
}

#[test]
fn full_three_command_cycle_advances_turn_exactly_once_per_generation() {
    let pack = sample_pack();
    let (mut state, gid) = start_and_pending_id(&pack);

    // Opening accept → turn 1.
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid, 1)),
        },
        &pack,
    )
    .expect("accept opening");
    state = t.state;
    assert_eq!(state.game.turn, 1);

    // SubmitChoice(choice-1) → pending, RequestGeneration.
    let t = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-1".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let gid2 = match &t.effects[0] {
        EngineEffect::RequestGeneration { generation_id, .. } => *generation_id,
        _ => panic!("expected RequestGeneration"),
    };
    state = t.state;
    assert!(state.pending.is_some());

    // Accept → turn 2, affection now 2.
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid2, 1)),
        },
        &pack,
    )
    .expect("accept turn 2");
    state = t.state;
    assert_eq!(state.game.turn, 2, "second accepted generation bumps to 2");
    assert_eq!(state.game.characters.get(&cid()).unwrap().affection, 2);
    assert_eq!(
        state.game.last_choice.as_ref().unwrap().choice_id,
        "choice-1",
        "last_choice records the player's pick"
    );
    assert_eq!(state.game.last_choice.as_ref().unwrap().turn, 1);
}

// ---------------------------------------------------------------------------
// invariant 1: late/mismatched AcceptGeneration never mutates game
// ---------------------------------------------------------------------------

#[test]
fn late_accept_with_no_pending_does_not_mutate_game() {
    let pack = sample_pack();
    let (mut state, gid) = start_and_pending_id(&pack);
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid, 1)),
        },
        &pack,
    )
    .expect("accept opening");
    state = t.state;
    assert!(state.pending.is_none());

    let before = serde_json::to_vec(&state.game).unwrap();
    let stale_gid = gid; // matches nothing now
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(stale_gid, 5)),
        },
        &pack,
    )
    .expect("late accept is Ok (discarded)");
    let after = serde_json::to_vec(&t.state.game).unwrap();
    assert_eq!(
        before, after,
        "late accept must not mutate game (invariant 1)"
    );
    assert!(t.effects.is_empty(), "late accept emits no effects");
    assert!(t.audit.is_empty());
}

#[test]
fn mismatched_accept_with_wrong_generation_id_does_not_mutate_game() {
    let pack = sample_pack();
    let (state, _gid) = start_and_pending_id(&pack);
    // pending is Some; feed a draft with a *different* generation id.
    let before = serde_json::to_vec(&state.game).unwrap();
    let other = uuid::Uuid::new_v4();
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(other, 3)),
        },
        &pack,
    )
    .expect("mismatched accept is Ok (discarded)");
    let after = serde_json::to_vec(&t.state.game).unwrap();
    assert_eq!(
        before, after,
        "mismatched accept must not mutate game (invariant 1)"
    );
    assert!(t.state.pending.is_some(), "pending preserved");
    assert!(t.effects.is_empty());
}

// ---------------------------------------------------------------------------
// invariant 2: failed apply_patch restores checkpoint byte-identically
// ---------------------------------------------------------------------------

#[test]
fn failed_apply_restores_checkpoint_byte_identically() {
    let pack = sample_pack();
    let (mut state, gid) = start_and_pending_id(&pack);
    // Get to turn 1 first so the checkpoint has real content to protect.
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid, 1)),
        },
        &pack,
    )
    .expect("accept opening");
    state = t.state;

    // SubmitChoice → pending with checkpoint = state.game (turn 1).
    let t = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let checkpoint = t.state.pending.as_ref().unwrap().checkpoint.clone();
    let checkpoint_bytes = serde_json::to_vec(&checkpoint).unwrap();
    let pending_gid = t.state.pending.as_ref().unwrap().generation_id;
    state = t.state;

    // Feed a malformed draft → apply_patch fails.
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(malformed_draft(pending_gid)),
        },
        &pack,
    )
    .expect("failed accept is Ok with error effect");

    let restored_bytes = serde_json::to_vec(&t.state.game).unwrap();
    assert_eq!(
        restored_bytes, checkpoint_bytes,
        "failed apply must restore checkpoint byte-identically (invariant 2)"
    );
    assert!(t.state.pending.is_none(), "pending cleared on failure");
    let has_error = t
        .effects
        .iter()
        .any(|e| matches!(e, EngineEffect::ShowRecoverableError(_)));
    assert!(has_error, "expected ShowRecoverableError effect");
    // The committed stable state never carries a partial commit: history length
    // and revision match the checkpoint.
    assert_eq!(t.state.game.history.len(), checkpoint.history.len());
    assert_eq!(t.state.game.revision, checkpoint.revision);
}

#[test]
fn provider_failure_exposes_retry_from_same_checkpoint() {
    let pack = sample_pack();
    let (state, opening_gid) = start_and_pending_id(&pack);
    let state = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(opening_gid, 1)),
        },
        &pack,
    )
    .expect("accept opening")
    .state;
    let submitted = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit choice");
    let failed_gid = submitted.state.pending.as_ref().unwrap().generation_id;
    let checkpoint = submitted.state.pending.as_ref().unwrap().checkpoint.clone();

    let failed = reduce(
        &submitted.state,
        EngineCommand::ProviderFailed {
            generation_id: failed_gid,
            error: DomainErrorView {
                code: "invalid_generation".to_string(),
                title_ko: "생성 실패".to_string(),
                message_ko: "다시 시도할 수 있습니다".to_string(),
                recoverable: true,
            },
        },
        &pack,
    )
    .expect("provider failure")
    .state;
    assert!(failed.pending.is_none());
    assert_eq!(
        serde_json::to_vec(&failed.retry_pending.as_ref().unwrap().checkpoint).unwrap(),
        serde_json::to_vec(&checkpoint).unwrap()
    );

    let retried = reduce(&failed, EngineCommand::RegenerateCurrentTurn, &pack)
        .expect("retry from failed generation");
    let retry_gid = retried.state.pending.as_ref().unwrap().generation_id;
    assert_ne!(retry_gid, failed_gid);
    assert!(retried.state.retry_pending.is_none());
    assert_eq!(
        serde_json::to_vec(&retried.state.pending.as_ref().unwrap().checkpoint).unwrap(),
        serde_json::to_vec(&checkpoint).unwrap()
    );
    assert!(
        retried
            .effects
            .iter()
            .any(|e| matches!(e, EngineEffect::RequestGeneration { .. }))
    );
}

// ---------------------------------------------------------------------------
// invariant 3: CancelGeneration restores checkpoint
// ---------------------------------------------------------------------------

#[test]
fn cancel_restores_checkpoint_and_clears_pending() {
    let pack = sample_pack();
    let (mut state, gid) = start_and_pending_id(&pack);
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid, 1)),
        },
        &pack,
    )
    .expect("accept opening");
    state = t.state;

    let before = serde_json::to_vec(&state.game).unwrap();

    let t = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let pending_gid = t.state.pending.as_ref().unwrap().generation_id;
    state = t.state;
    assert!(state.pending.is_some());

    let t = reduce(
        &state,
        EngineCommand::CancelGeneration {
            generation_id: pending_gid,
        },
        &pack,
    )
    .expect("cancel");

    let after = serde_json::to_vec(&t.state.game).unwrap();
    assert_eq!(
        after, before,
        "cancel must restore checkpoint byte-identically (invariant 3)"
    );
    assert!(t.state.pending.is_none(), "pending cleared on cancel");
    assert_publish_view_present(&t);
}

#[test]
fn cancel_with_wrong_generation_id_is_noop() {
    let pack = sample_pack();
    let (state, _gid) = start_and_pending_id(&pack);
    let before = serde_json::to_vec(&state.game).unwrap();
    let other = uuid::Uuid::new_v4();
    let t = reduce(
        &state,
        EngineCommand::CancelGeneration {
            generation_id: other,
        },
        &pack,
    )
    .expect("cancel");
    let after = serde_json::to_vec(&t.state.game).unwrap();
    assert_eq!(before, after);
    assert!(
        t.state.pending.is_some(),
        "pending preserved on no-op cancel"
    );
}

// ---------------------------------------------------------------------------
// invariant 4: no two generations in flight
// ---------------------------------------------------------------------------

#[test]
fn second_submit_while_pending_is_rejected() {
    let pack = sample_pack();
    let (state, _gid) = start_and_pending_id(&pack);
    // StartNewGame already set pending (opening generation in flight).
    let err = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect_err("submit during opening generation must Err");
    assert_eq!(
        err.reason_code(),
        Some("engine_generation_in_flight"),
        "invariant 4: one generation in flight"
    );
}

#[test]
fn submit_with_unknown_choice_id_is_rejected() {
    let pack = sample_pack();
    let (mut state, gid) = start_and_pending_id(&pack);
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid, 1)),
        },
        &pack,
    )
    .expect("accept opening");
    state = t.state;

    let err = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "no-such-choice".to_string(),
        },
        &pack,
    )
    .expect_err("unknown choice id must Err");
    assert_eq!(err.reason_code(), Some("engine_unknown_choice"),);
}

// ---------------------------------------------------------------------------
// invariant 5: turn +1 per accepted generation (covered above; isolated assert)
// ---------------------------------------------------------------------------

#[test]
fn turn_increments_exactly_once_per_accepted_generation() {
    let pack = sample_pack();
    let (mut state, gid) = start_and_pending_id(&pack);
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid, 1)),
        },
        &pack,
    )
    .expect("accept");
    state = t.state;
    assert_eq!(state.game.turn, 1);
    // revision also bumped exactly once by apply_patch.
    assert_eq!(state.game.revision, 1);
}

// ---------------------------------------------------------------------------
// misc: DismissError, Quit, opening choice set on StartNewGame
// ---------------------------------------------------------------------------

#[test]
fn dismiss_error_publishes_view_without_error() {
    let pack = sample_pack();
    let (mut state, gid) = start_and_pending_id(&pack);
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid, 1)),
        },
        &pack,
    )
    .expect("accept opening");
    state = t.state;
    let t = reduce(&state, EngineCommand::DismissError, &pack).expect("dismiss");
    let view = published_view(&t);
    assert!(view.error.is_none());
}

#[test]
fn quit_returns_state_unchanged() {
    let pack = sample_pack();
    let (state, _gid) = start_and_pending_id(&pack);
    let before = serde_json::to_vec(&state.game).unwrap();
    let t = reduce(&state, EngineCommand::Quit, &pack).expect("quit");
    let after = serde_json::to_vec(&t.state.game).unwrap();
    assert_eq!(before, after);
    assert!(t.effects.is_empty());
}

fn published_view(t: &Transition) -> AppViewModel {
    t.effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::PublishView(v) => Some((**v).clone()),
            _ => None,
        })
        .expect("a PublishView was emitted")
}

// ---------------------------------------------------------------------------
// sanity: commit_turn still works (kept for backward compat with W3)
// ---------------------------------------------------------------------------

#[test]
fn commit_turn_still_applies_patch() {
    // Smoke-test the retained W3 primitive so it does not silently rot.
    let _ = ApplyResult {
        state: story_model::GameState {
            save_schema_version: SAVE_SCHEMA_VERSION,
            story_id: String::new(),
            story_version: semver::Version::new(0, 1, 0),
            pack_schema_version: semver::Version::new(1, 0, 0),
            session_id: uuid::Uuid::nil(),
            current_chapter: String::new(),
            current_scene_id: String::new(),
            turn: 0,
            location: LocationId("loc-classroom".to_string()),
            time_slot: TimeSlot::Morning,
            active_route: None,
            characters: BTreeMap::from([(
                cid(),
                story_model::CharacterState {
                    affection: 0,
                    trust: 0,
                    tension: 0,
                    relationship_stage: RelationshipStage::Acquaintance,
                    current_emotion: Emotion::Neutral,
                    known_facts: Default::default(),
                    address_form_overrides: Default::default(),
                },
            )]),
            flags: BTreeMap::new(),
            numbers: BTreeMap::new(),
            strings: BTreeMap::new(),
            inventory: BTreeMap::new(),
            occurred_events: Default::default(),
            recent_summaries: Default::default(),
            memories: Default::default(),
            history: Vec::new(),
            last_choice: None,
            revision: 0,
            stable_generation_id: None,
        },
        change_log: Vec::new(),
        warnings: Vec::new(),
        fired_events: Vec::new(),
        ending: None,
    };
    let pack = sample_pack();
    let mut patch = StatePatch::default();
    patch.affection_delta.insert(cid(), 1);
    let init = EngineState::initial(&pack);
    let res = commit_turn(&init.game, &pack.rules, &patch, uuid::Uuid::nil()).expect("commit_turn");
    assert_eq!(res.state.characters.get(&cid()).unwrap().affection, 1);
    let _: ConditionExpr = ConditionExpr::FlagEquals {
        id: "x".to_string(),
        value: true,
    };
    let _: ChoiceView = ChoiceView {
        id: "x".into(),
        label: "y".into(),
        risk: RiskLevel::Safe,
        preview: None,
    };
}
