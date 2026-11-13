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

use story_core::engine::{
    AUTOSAVE_SLOT, Checkpoint, EngineState, NarrationRepair, SideGenerationKind, Transition,
    VARIANT_CAP, build_view, commit_turn, reduce,
};
use story_core::engine_cmd::{EngineCommand, EngineEffect};
use story_core::reducer::ApplyResult;
use story_model::{
    AppViewModel, CharacterDef, CharacterId, Choice, ChoiceView, ConditionExpr, DomainErrorView,
    Emotion, GeneratedTurnDraft, LocationDef, LocationId, Lorebook, LorebookEntry, LorebookLogic,
    LorebookPosition, PackContentPolicy, PackInitialState, PackMetadata, PackRating,
    PromptFragments, RelationshipStage, RiskLevel, SAVE_SCHEMA_VERSION, SavePayload, Screen,
    StatePatch, StoryPack, StoryRules, TimeSlot, TurnRequest, TurnSummary,
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
        default_choice_count: 3,
        initial_state: PackInitialState {
            chapter: "chapter-1".to_string(),
            scene: "scene-1".to_string(),
            location: LocationId("loc-classroom".to_string()),
            time_slot: TimeSlot::Morning,
        },
        content: PackContentPolicy {
            violence: "none".to_string(),
        },
    };

    let character = CharacterDef {
        id: cid(),
        name: "나무".to_string(),
        age: Some(28),
        pronouns: None,
        description_md: None,
        dialogue_examples: Vec::new(),
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
        lorebook: story_model::Lorebook::default(),
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
        narration: format!(
            "유나가 고개를 끄덕인다 (affection {affection_delta:+}, generation {generation_id})."
        ),
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

/// A draft whose patch violates a still-hard stage-3 rule (+5 exceeds the ±3
/// single-turn delta limit) → rejected by the validator. Unknown references
/// are NOT hard failures anymore (they drop with an audit entry), so they
/// cannot stand in for a malformed draft here.
fn malformed_draft(generation_id: uuid::Uuid) -> GeneratedTurnDraft {
    let mut patch = StatePatch::default();
    patch.affection_delta.insert(cid(), 5);
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

fn designed_choice(id: &str, label: &str, intent: &str, risk: RiskLevel, preview: &str) -> Choice {
    Choice {
        id: id.to_string(),
        label: label.to_string(),
        intent: intent.to_string(),
        tone: None,
        target: None,
        risk,
        hidden_requirements: vec![],
        preview: Some(preview.to_string()),
    }
}

fn interesting_choices() -> Vec<Choice> {
    vec![
        designed_choice(
            "fresh-relationship",
            "지금 떠났던 이유를 솔직하게 묻는다",
            "거절 가능성을 감수하고 관계의 핵심 질문에 답을 요구한다",
            RiskLevel::High,
            "회피하던 이별의 이유와 두 사람의 거리를 시험한다",
        ),
        designed_choice(
            "fresh-evidence",
            "찢긴 승차권의 날짜를 함께 확인한다",
            "말과 맞지 않는 구체적인 시간 단서를 확인한다",
            RiskLevel::Low,
            "과거의 설명에서 어긋난 날짜 하나를 확인한다",
        ),
        designed_choice(
            "fresh-situation",
            "막차 전에 강변 전시로 함께 이동하자고 제안한다",
            "남은 시간을 걸고 장소와 대화 조건을 바꾼다",
            RiskLevel::Medium,
            "제안을 받아들이는지에 따라 다음 장소와 대화 시간이 달라진다",
        ),
    ]
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
fn generation_request_carries_structured_output_schema() {
    let pack = sample_pack();
    let init = EngineState::initial(&pack);
    let transition = reduce(&init, EngineCommand::StartNewGame, &pack).expect("start");
    let request = transition
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestGeneration { request, .. } => Some(request),
            _ => None,
        })
        .expect("StartNewGame emits RequestGeneration");
    assert!(
        request.json_schema.is_some(),
        "generation requests must carry the structured-output schema"
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
    assert!(
        view.narration
            .starts_with("유나가 고개를 끄덕인다 (affection +1")
    );
    assert_eq!(view.status.turn, 1);
    assert_eq!(view.status.affection, Some(1));
    assert_eq!(view.screen, Screen::Story);
    assert_eq!(view.story_title, "데모 이야기");
    assert_eq!(view.location_display, "교실");
    assert_eq!(
        t.state.game.strings.get("__storytui.narrative.phase"),
        Some(&"pressure".to_string()),
        "accepted turns, not requests, advance the engine-owned story clock"
    );
    assert_eq!(
        t.state.game.numbers.get("__storytui.narrative.due_turn"),
        Some(&2),
        "the next pressure is persisted for save/resume"
    );
}

#[test]
fn accepted_patch_updates_all_relationship_meters_and_published_status() {
    let pack = sample_pack();
    let (state, generation_id) = start_and_pending_id(&pack);
    let mut draft = valid_draft(generation_id, 2);
    draft.proposed_patch.trust_delta.insert(cid(), -1);
    draft.proposed_patch.tension_delta.insert(cid(), 3);

    let accepted = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(draft),
        },
        &pack,
    )
    .expect("accept relationship patch");

    let character = accepted.state.game.characters.get(&cid()).unwrap();
    assert_eq!(
        (character.affection, character.trust, character.tension),
        (2, -1, 3)
    );
    let view = published_view(&accepted);
    assert_eq!(
        (
            view.status.affection,
            view.status.trust,
            view.status.tension
        ),
        (Some(2), Some(-1), Some(3))
    );
}

#[test]
fn accepted_numbered_choice_is_recorded_in_history() {
    let pack = sample_pack();
    let (state, opening_gid) = start_and_pending_id(&pack);
    let opened = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(opening_gid, 1)),
        },
        &pack,
    )
    .expect("accept opening")
    .state;
    let submitted = reduce(
        &opened,
        EngineCommand::SubmitChoice {
            choice_id: "choice-1".to_string(),
        },
        &pack,
    )
    .expect("submit choice");
    let generation_id = submitted
        .state
        .pending
        .as_ref()
        .expect("pending")
        .generation_id;
    let accepted = reduce(
        &submitted.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(generation_id, 1)),
        },
        &pack,
    )
    .expect("accept choice")
    .state;

    let action = accepted
        .game
        .history
        .last()
        .and_then(|entry| entry.player_action.as_ref())
        .expect("numbered choice recorded");
    assert_eq!(action.original_text, "돌아선다");
    assert_eq!(action.confidence, Some(1.0));
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

#[test]
fn latest_live_uninteresting_fixture_is_refreshed_before_display() {
    let pack = sample_pack();
    let (pending, generation_id) = start_and_pending_id(&pack);
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/uninteresting_choices_latest.json"))
            .expect("live choice fixture");
    let mut draft = valid_draft(generation_id, 1);
    draft.choices = serde_json::from_value(fixture["choices"].clone()).expect("fixture choices");
    let accepted_narration = draft.narration.clone();

    let transition = reduce(
        &pending,
        EngineCommand::AcceptGeneration {
            draft: Box::new(draft),
        },
        &pack,
    )
    .expect("accept then refresh weak choices");

    assert_eq!(transition.state.game.turn, 1, "the scene still commits");
    assert_eq!(
        transition.state.game.history.last().unwrap().narration,
        accepted_narration,
        "choice repair cannot rewrite accepted prose"
    );
    assert!(transition.state.pending.as_ref().unwrap().choice_refresh);
    let repair_prompt = transition
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestGeneration { request, .. } => request.messages.last(),
            _ => None,
        })
        .expect("choice-only request")
        .content
        .as_str();
    assert!(repair_prompt.contains("# 세 슬롯 (이 순서 고정)"));
    assert!(repair_prompt.contains("단서/low"));
    assert!(repair_prompt.contains("품질 규칙"));
    assert!(
        !repair_prompt.contains(
            fixture["choices"][0]["label"]
                .as_str()
                .expect("fixture label")
        )
    );
    let repair_request = transition
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestGeneration { request, .. } => Some(request),
            _ => None,
        })
        .expect("choice-only request");
    assert_eq!(repair_request.max_tokens, 900);
    assert_eq!(repair_request.messages.len(), 2);
    assert!(repair_request.messages[0].content.contains("생각"));
    assert!(
        repair_request.messages[0]
            .content
            .contains("label, intent, risk, preview만")
    );
    assert!(repair_request.messages[0].content.contains("전체 이용가"));
    assert!(
        repair_request
            .messages
            .iter()
            .map(|message| message.content.len())
            .sum::<usize>()
            < 6_000,
        "choice repair prompt must remain compact"
    );
}

#[test]
fn just_selected_action_is_rejected_even_before_last_choice_commits() {
    let pack = sample_pack();
    let (opening_pending, opening_id) = start_and_pending_id(&pack);
    let mut opening = valid_draft(opening_id, 1);
    opening.choices[0].label = "유나의 손목 시계와 코트 상태를 살핀다".to_string();
    let opening = reduce(
        &opening_pending,
        EngineCommand::AcceptGeneration {
            draft: Box::new(opening),
        },
        &pack,
    )
    .expect("opening");
    assert!(
        opening.state.game.last_choice.is_none(),
        "opening does not have a historical last choice"
    );
    let submitted = reduce(
        &opening.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit current choice");
    let generation_id = submitted.state.pending.as_ref().unwrap().generation_id;
    let mut draft = valid_draft(generation_id, 1);
    draft.choices = interesting_choices();
    draft.choices[0] = designed_choice(
        "echo",
        "시계와 코트 상태를 언급해 변화를 확인한다",
        "손목 시계와 코트 상태의 차이를 다시 살핀다",
        RiskLevel::High,
        "시계와 옷차림의 변화를 확인한다",
    );

    let transition = reduce(
        &submitted.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(draft),
        },
        &pack,
    )
    .expect("accept echoing set");

    assert!(transition.state.pending.as_ref().unwrap().choice_refresh);
    let repair_prompt = transition
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestGeneration { request, .. } => request.messages.last(),
            _ => None,
        })
        .expect("choice repair prompt")
        .content
        .as_str();
    assert!(repair_prompt.contains("문장은 보여 주지 않으니 복사하지 말고"));
    assert!(!repair_prompt.contains("시계와 코트 상태를 언급해 변화를 확인한다"));
}

#[test]
fn manual_choice_refresh_preserves_turn_and_accepts_a_new_designed_set() {
    let pack = sample_pack();
    let (opening_pending, opening_id) = start_and_pending_id(&pack);
    let stable = reduce(
        &opening_pending,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(opening_id, 1)),
        },
        &pack,
    )
    .expect("opening")
    .state;
    let before = serde_json::to_vec(&stable.game).unwrap();

    let refreshing =
        reduce(&stable, EngineCommand::RegenerateChoices, &pack).expect("manual choice refresh");
    assert_eq!(serde_json::to_vec(&refreshing.state.game).unwrap(), before);
    assert!(refreshing.state.pending.as_ref().unwrap().choice_refresh);
    assert!(refreshing.effects.iter().any(|effect| matches!(
        effect,
        EngineEffect::PersistStableState { slot_id, .. } if slot_id == AUTOSAVE_SLOT
    )));

    let refresh_id = refreshing.state.pending.as_ref().unwrap().generation_id;
    let mut replacement = valid_draft(refresh_id, 0);
    replacement.choices = interesting_choices();
    for choice in &mut replacement.choices {
        choice.risk = RiskLevel::Safe;
    }
    let complete = reduce(
        &refreshing.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(replacement),
        },
        &pack,
    )
    .expect("accept new choices");

    assert!(complete.state.pending.is_none());
    assert_eq!(complete.state.game.turn, stable.game.turn);
    assert_eq!(
        complete.state.game.history.last().unwrap().choices,
        interesting_choices()
    );
}

#[test]
fn failed_choice_refresh_keeps_the_old_offers_without_blocking_the_player() {
    let pack = sample_pack();
    let (opening_pending, opening_id) = start_and_pending_id(&pack);
    let stable = reduce(
        &opening_pending,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(opening_id, 1)),
        },
        &pack,
    )
    .expect("opening")
    .state;
    let before = serde_json::to_vec(&stable.game).unwrap();
    let refreshing =
        reduce(&stable, EngineCommand::RegenerateChoices, &pack).expect("manual choice refresh");
    let refresh_id = refreshing.state.pending.as_ref().unwrap().generation_id;
    let mut repeated = valid_draft(refresh_id, 0);
    repeated.choices = stable.game.history.last().unwrap().choices.clone();
    for choice in &mut repeated.choices {
        choice.intent = choice.label.clone();
        choice.preview = Some("기존 제안을 표현만 바꿔 다시 시도한다".to_string());
    }

    let retrying = reduce(
        &refreshing.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(repeated),
        },
        &pack,
    )
    .expect("retry repeated refresh once");

    assert_eq!(serde_json::to_vec(&retrying.state.game).unwrap(), before);
    let retry_pending = retrying
        .state
        .pending
        .as_ref()
        .expect("second refresh pending");
    assert_eq!(retry_pending.choice_refresh_attempt, 2);
    assert_eq!(retry_pending.choice_refresh_rejected.len(), 3);
    let retry_request = retrying
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestGeneration { request, .. } => Some(request),
            _ => None,
        })
        .expect("second choice request");
    let retry_prompt = &retry_request.messages.last().unwrap().content;
    assert!(retry_prompt.contains("문장은 보여 주지 않으니 복사하지 말고"));
    assert!(!retry_prompt.contains(&stable.game.history.last().unwrap().choices[0].label));

    let retry_id = retry_pending.generation_id;
    let mut repeated_again = valid_draft(retry_id, 0);
    repeated_again.choices = stable.game.history.last().unwrap().choices.clone();
    for choice in &mut repeated_again.choices {
        choice.intent = choice.label.clone();
        choice.preview = Some("첫 실패와 같은 제안을 또 반복한다".to_string());
    }
    let finished = reduce(
        &retrying.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(repeated_again),
        },
        &pack,
    )
    .expect("finish after one repeated retry");

    assert_eq!(serde_json::to_vec(&finished.state.game).unwrap(), before);
    assert!(finished.state.pending.is_none());
    assert!(finished.state.retry_pending.is_none());
    assert!(
        !finished
            .effects
            .iter()
            .any(|effect| matches!(effect, EngineEffect::ShowRecoverableError(_)))
    );
    assert!(
        !finished
            .effects
            .iter()
            .any(|effect| matches!(effect, EngineEffect::RequestGeneration { .. }))
    );
}

#[test]
fn repeated_choice_set_is_refreshed_without_rewriting_the_accepted_turn() {
    let pack = sample_pack();
    let (opening_pending, opening_id) = start_and_pending_id(&pack);
    let mut opening = reduce(
        &opening_pending,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(opening_id, 1)),
        },
        &pack,
    )
    .expect("accept opening");
    for choice in &mut opening.state.game.history.last_mut().unwrap().choices {
        choice.intent = "ask about the letter".to_string();
    }

    let submitted = reduce(
        &opening.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit choice");
    let turn_id = submitted.state.pending.as_ref().unwrap().generation_id;
    let mut second_draft = valid_draft(turn_id, 1);
    for choice in &mut second_draft.choices {
        choice.intent = "ask about the letter".to_string();
    }
    let refreshed = reduce(
        &submitted.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(second_draft.clone()),
        },
        &pack,
    )
    .expect("accept repeated choices");

    assert_eq!(refreshed.state.game.turn, 2, "scene is committed once");
    assert!(refreshed.state.pending.as_ref().unwrap().choice_refresh);
    assert!(refreshed.effects.iter().any(|effect| matches!(
        effect,
        EngineEffect::PersistStableState { slot_id, .. } if slot_id == AUTOSAVE_SLOT
    )));
    assert_eq!(
        refreshed.state.game.history.last().unwrap().narration,
        second_draft.narration,
        "choice repair must not rewrite accepted narration"
    );

    let refresh_id = refreshed.state.pending.as_ref().unwrap().generation_id;
    let mut replacement = valid_draft(refresh_id, 0);
    replacement.choices[0].label = "창가에서 잠시 거리를 둔다".to_string();
    replacement.choices[1].label = "편지를 함께 확인한다".to_string();
    replacement.choices[2].label = "밖으로 나가 우산을 찾는다".to_string();
    let complete = reduce(
        &refreshed.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(replacement),
        },
        &pack,
    )
    .expect("accept replacement choices");

    assert!(complete.state.pending.is_none());
    assert_eq!(
        complete.state.game.turn, 2,
        "choice refresh does not add a turn"
    );
    assert_eq!(
        complete.state.game.history.last().unwrap().narration,
        second_draft.narration
    );
    assert_eq!(
        complete.state.game.history.last().unwrap().choices[1].label,
        "편지를 함께 확인한다"
    );
}

#[test]
fn repeated_narration_is_rewritten_before_commit_and_preserves_authoritative_fields() {
    let pack = sample_pack();
    let repeated = include_str!("fixtures/repeated_narration_ko.txt");
    let (opening_pending, opening_id) = start_and_pending_id(&pack);
    let mut opening = valid_draft(opening_id, 1);
    opening.narration = repeated.to_string();
    let opened = reduce(
        &opening_pending,
        EngineCommand::AcceptGeneration {
            draft: Box::new(opening),
        },
        &pack,
    )
    .expect("accept opening");

    let submitted = reduce(
        &opened.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit choice");
    let first_id = submitted.state.pending.as_ref().unwrap().generation_id;
    let mut rejected = valid_draft(first_id, 1);
    rejected.narration = repeated.to_string();
    let authoritative_choices = rejected.choices.clone();
    let repairing = reduce(
        &submitted.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(rejected),
        },
        &pack,
    )
    .expect("start narration repair");

    assert_eq!(
        repairing.state.game.turn, 1,
        "rejected prose is not committed"
    );
    assert_eq!(repairing.state.game.history.len(), 1);
    assert!(
        !repairing
            .effects
            .iter()
            .any(|effect| matches!(effect, EngineEffect::PersistStableState { .. }))
    );
    assert!(matches!(
        repairing
            .state
            .pending
            .as_ref()
            .and_then(|pending| pending.narration_repair.as_ref()),
        Some(NarrationRepair::SinglePass { attempt: 1, .. })
    ));
    let (repair_id, repair_request) = repairing
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestGeneration {
                generation_id,
                request,
            } => Some((*generation_id, request)),
            _ => None,
        })
        .expect("one automatic repair request");
    assert!(
        repair_request
            .messages
            .last()
            .unwrap()
            .content
            .contains("Narration-only quality repair")
    );

    let repaired_text = "유나는 창가에서 몸을 일으켜 계산서 뒷면에 전시회 주소를 적었다. 그리고 오늘 저녁 여섯 시까지 올 수 있는지 구체적으로 물었다.";
    let mut repaired = valid_draft(repair_id, -3);
    repaired.narration = repaired_text.to_string();
    repaired.choices[0].label = "모델이 바꾸려 한 선택지".to_string();
    repaired
        .proposed_patch
        .set_flags
        .insert("nonexistent_flag".to_string(), true);
    let completed = reduce(
        &repairing.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(repaired),
        },
        &pack,
    )
    .expect("accept repaired narration");

    assert_eq!(completed.state.game.turn, 2);
    let entry = completed.state.game.history.last().unwrap();
    assert_eq!(entry.narration, repaired_text);
    assert_eq!(entry.choices, authoritative_choices);
    assert_eq!(
        entry.applied_patch.affection_delta.get(&cid()),
        Some(&1),
        "repair response cannot replace the first draft's patch"
    );
    assert!(
        !entry
            .applied_patch
            .set_flags
            .contains_key("nonexistent_flag")
    );
    assert_eq!(completed.state.quality_pressure, 0);
    assert!(completed.state.last_narration_quality.is_none());
}

#[test]
fn second_repetitive_narration_is_not_committed_or_repaired_again() {
    let pack = sample_pack();
    let repeated = include_str!("fixtures/repeated_narration_ko.txt");
    let (opening_pending, opening_id) = start_and_pending_id(&pack);
    let mut opening = valid_draft(opening_id, 1);
    opening.narration = repeated.to_string();
    let opened = reduce(
        &opening_pending,
        EngineCommand::AcceptGeneration {
            draft: Box::new(opening),
        },
        &pack,
    )
    .expect("accept opening");
    let submitted = reduce(
        &opened.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let first_id = submitted.state.pending.as_ref().unwrap().generation_id;
    let mut rejected = valid_draft(first_id, 1);
    rejected.narration = repeated.to_string();
    let repairing = reduce(
        &submitted.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(rejected),
        },
        &pack,
    )
    .expect("repair attempt 1");
    assert!(matches!(
        repairing
            .state
            .pending
            .as_ref()
            .and_then(|pending| pending.narration_repair.as_ref()),
        Some(NarrationRepair::SinglePass { attempt: 1, .. })
    ));
    let repair_id = repairing.state.pending.as_ref().unwrap().generation_id;
    let mut repeated_again = valid_draft(repair_id, 1);
    repeated_again.narration = repeated.to_string();
    let repairing_again = reduce(
        &repairing.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(repeated_again),
        },
        &pack,
    )
    .expect("repair attempt 2");
    assert!(matches!(
        repairing_again
            .state
            .pending
            .as_ref()
            .and_then(|pending| pending.narration_repair.as_ref()),
        Some(NarrationRepair::SinglePass { attempt: 2, .. })
    ));
    let (second_repair_id, second_request) = repairing_again
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestGeneration {
                generation_id,
                request,
            } => Some((*generation_id, request)),
            _ => None,
        })
        .expect("second automatic repair request");
    let second_prompt = &second_request.messages.last().unwrap().content;
    assert!(
        second_prompt.contains("강제 장면 전진"),
        "attempt-2 repair must force a new beat: {second_prompt}"
    );

    let mut repeated_third = valid_draft(second_repair_id, 1);
    repeated_third.narration = repeated.to_string();
    let failed = reduce(
        &repairing_again.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(repeated_third),
        },
        &pack,
    )
    .expect("bounded repair failure after attempt 2");

    assert_eq!(failed.state.game.turn, 1);
    assert_eq!(failed.state.game.history.len(), 1);
    assert!(failed.state.pending.is_none());
    assert!(failed.state.retry_pending.is_some());
    assert_eq!(failed.state.quality_pressure, 1);
    assert!(failed.state.last_narration_quality.is_some());
    assert!(failed.effects.iter().any(|effect| matches!(
        effect,
        EngineEffect::ShowRecoverableError(error)
            if error.code == "narration_quality_failed"
    )));
    assert!(
        !failed
            .effects
            .iter()
            .any(|effect| matches!(effect, EngineEffect::RequestGeneration { .. })),
        "no third automatic repair after attempt 2"
    );
}

#[test]
fn attempt_two_repair_prompt_includes_forced_beat_directive() {
    let pack = sample_pack();
    let repeated = include_str!("fixtures/repeated_narration_ko.txt");
    let (opening_pending, opening_id) = start_and_pending_id(&pack);
    let mut opening = valid_draft(opening_id, 1);
    opening.narration = repeated.to_string();
    let opened = reduce(
        &opening_pending,
        EngineCommand::AcceptGeneration {
            draft: Box::new(opening),
        },
        &pack,
    )
    .expect("accept opening");
    let submitted = reduce(
        &opened.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let first_id = submitted.state.pending.as_ref().unwrap().generation_id;
    let mut rejected = valid_draft(first_id, 1);
    rejected.narration = repeated.to_string();
    let attempt1 = reduce(
        &submitted.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(rejected),
        },
        &pack,
    )
    .expect("attempt 1");
    let repair_id = attempt1.state.pending.as_ref().unwrap().generation_id;
    let mut still_bad = valid_draft(repair_id, 1);
    still_bad.narration = repeated.to_string();
    let attempt2 = reduce(
        &attempt1.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(still_bad),
        },
        &pack,
    )
    .expect("attempt 2");
    let prompt = attempt2
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestGeneration { request, .. } => request.messages.last(),
            _ => None,
        })
        .expect("attempt-2 repair prompt");
    assert!(
        prompt.content.contains("강제 장면 전진")
            || prompt.content.contains("Banned recurring motifs"),
        "attempt-2 prompt must strengthen escape: {}",
        prompt.content
    );
    assert!(
        (prompt.content.contains("HighSimilarity") || prompt.content.contains("ExactDuplicate"))
            || prompt.content.contains("Banned recurring motifs"),
        "attempt-2 should cite hard issues or banned motifs"
    );
}

#[test]
fn second_consecutive_no_progress_turn_is_repaired_before_commit() {
    let pack = sample_pack();
    let (opening_pending, opening_id) = start_and_pending_id(&pack);
    let opening = valid_draft(opening_id, 0);
    let opened = reduce(
        &opening_pending,
        EngineCommand::AcceptGeneration {
            draft: Box::new(opening),
        },
        &pack,
    )
    .expect("quiet opening is allowed");
    let submitted = reduce(
        &opened.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let generation_id = submitted.state.pending.as_ref().unwrap().generation_id;
    let quiet_again = valid_draft(generation_id, 0);
    let repairing = reduce(
        &submitted.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(quiet_again),
        },
        &pack,
    )
    .expect("repair stalled turn");

    assert_eq!(repairing.state.game.turn, 1);
    let instruction = repairing
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestGeneration { request, .. } => request.messages.last(),
            _ => None,
        })
        .expect("repair prompt");
    assert!(instruction.content.contains("직전 턴에 이어 변화가 없다"));
}

#[test]
fn empty_narration_gets_one_repair_then_the_quality_overlay() {
    let pack = sample_pack();
    let (state, gid) = start_and_pending_id(&pack);
    let mut empty = valid_draft(gid, 1);
    empty.narration = "   ".to_string();
    let repairing = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(empty),
        },
        &pack,
    )
    .expect("empty narration starts the bounded repair");

    assert_eq!(repairing.state.game.turn, 0, "nothing is committed");
    assert!(matches!(
        repairing
            .state
            .pending
            .as_ref()
            .and_then(|pending| pending.narration_repair.as_ref()),
        Some(NarrationRepair::SinglePass { attempt: 1, .. })
    ));
    let (repair_id, repair_request) = repairing
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestGeneration {
                generation_id,
                request,
            } => Some((*generation_id, request)),
            _ => None,
        })
        .expect("attempt-1 repair request");
    assert!(
        repair_request
            .messages
            .last()
            .unwrap()
            .content
            .contains("본문이 비어 있다"),
        "the repair prompt must carry the Empty instruction"
    );

    // Attempt 1 also comes back empty → second automatic repair.
    let mut still_empty = valid_draft(repair_id, 1);
    still_empty.narration = String::new();
    let repairing_again = reduce(
        &repairing.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(still_empty),
        },
        &pack,
    )
    .expect("empty repair attempt 2");
    assert!(matches!(
        repairing_again
            .state
            .pending
            .as_ref()
            .and_then(|pending| pending.narration_repair.as_ref()),
        Some(NarrationRepair::SinglePass { attempt: 2, .. })
    ));
    let second_repair_id = repairing_again
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestGeneration { generation_id, .. } => Some(*generation_id),
            _ => None,
        })
        .expect("attempt-2 repair request");

    // Attempt 2 still empty → overlay, no third automatic repair.
    let mut empty_again = valid_draft(second_repair_id, 1);
    empty_again.narration = String::new();
    let failed = reduce(
        &repairing_again.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(empty_again),
        },
        &pack,
    )
    .expect("bounded repair failure");

    assert_eq!(failed.state.game.turn, 0);
    assert_eq!(failed.state.game.history.len(), 0);
    assert!(failed.state.pending.is_none());
    assert!(failed.state.retry_pending.is_some());
    assert_eq!(failed.state.quality_pressure, 1);
    assert!(failed.effects.iter().any(|effect| matches!(
        effect,
        EngineEffect::ShowRecoverableError(error)
            if error.code == "narration_quality_failed"
    )));
    assert!(
        !failed
            .effects
            .iter()
            .any(|effect| matches!(effect, EngineEffect::RequestGeneration { .. })),
        "the repair is attempted at most twice"
    );
}

// ---------------------------------------------------------------------------
// one-key undo (RestoreCheckpoint): the player recovers from a bad decision
// ---------------------------------------------------------------------------

/// 오프닝 수락 → 선택 확정 → 수락까지 진행한 상태(턴 2)와 함께 오프닝 턴의
/// 체크포인트 id를 돌려준다.
fn two_accepted_turns(pack: &StoryPack) -> (EngineState, uuid::Uuid, uuid::Uuid) {
    let (state, opening_gid) = start_and_pending_id(pack);
    let opened = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(opening_gid, 1)),
        },
        pack,
    )
    .expect("accept opening");
    let submitted = reduce(
        &opened.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        pack,
    )
    .expect("submit choice");
    let turn_gid = submitted.state.pending.as_ref().unwrap().generation_id;
    let accepted = reduce(
        &submitted.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(turn_gid, -1)),
        },
        pack,
    )
    .expect("accept turn");
    let undo_id = published_view(&accepted)
        .undo_checkpoint_id
        .expect("undo target offered after two accepted turns");
    (accepted.state, undo_id, opening_gid)
}

#[test]
fn undo_target_appears_only_after_a_committed_turn() {
    let pack = sample_pack();
    let (state, opening_gid) = start_and_pending_id(&pack);
    let opened = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(opening_gid, 1)),
        },
        &pack,
    )
    .expect("accept opening");
    // 오프닝 직후: 같은 턴의 체크포인트만 있어 되돌릴 대상이 없다.
    assert_eq!(opened.state.game.turn, 1);
    assert!(published_view(&opened).undo_checkpoint_id.is_none());
    // 두 번째 턴 확정 후: 직전 턴(오프닝)으로 되돌릴 수 있다.
    let submitted = reduce(
        &opened.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let gid = submitted.state.pending.as_ref().unwrap().generation_id;
    let accepted = reduce(
        &submitted.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid, 1)),
        },
        &pack,
    )
    .expect("accept turn");
    assert_eq!(accepted.state.game.turn, 2);
    assert!(published_view(&accepted).undo_checkpoint_id.is_some());
}

#[test]
fn restore_checkpoint_rewinds_one_turn_and_persists() {
    let pack = sample_pack();
    let (state, undo_id, _opening_gid) = two_accepted_turns(&pack);
    assert_eq!(state.game.turn, 2);

    let restored = reduce(
        &state,
        EngineCommand::RestoreCheckpoint {
            checkpoint_id: undo_id,
        },
        &pack,
    )
    .expect("restore");

    assert_eq!(restored.state.game.turn, 1, "rewound to the opening turn");
    assert_eq!(
        restored.state.game.history.len(),
        1,
        "the undone turn's history entry is gone"
    );
    assert!(restored.state.pending.is_none());
    assert!(
        restored.state.retry_pending.is_none(),
        "stale retry cleared"
    );
    let view = published_view(&restored);
    assert_eq!(view.status.turn, 1);
    assert_eq!(view.choices.len(), 3, "the rewound turn's choices are back");
    assert!(
        view.undo_checkpoint_id.is_none(),
        "only same-turn checkpoints remain after undo"
    );
    // 되돌린 상태가 자동 저장된다: 종료→이어하기가 되돌린 지점에서 시작.
    let persist = restored
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::PersistStableState { slot_id, .. } => Some(slot_id.clone()),
            _ => None,
        })
        .expect("restore persists the rewound state");
    assert_eq!(persist, AUTOSAVE_SLOT);
}

#[test]
fn undo_after_recommit_still_walks_back_one_turn() {
    let pack = sample_pack();
    let (rewound, undo_id, _gid) = two_accepted_turns(&pack);
    let restored = reduce(
        &rewound,
        EngineCommand::RestoreCheckpoint {
            checkpoint_id: undo_id,
        },
        &pack,
    )
    .expect("restore");
    assert_eq!(restored.state.game.turn, 1);

    // 되돌린 지점에서 같은 선택을 다시 확정해도 되돌리기 대상은 여전히
    // 직전 턴이다 — 오래된 동일 턴 체크포인트가 redo처럼 잡히지 않는다.
    let resubmitted = reduce(
        &restored.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("resubmit");
    let gid = resubmitted.state.pending.as_ref().unwrap().generation_id;
    let recommitted = reduce(
        &resubmitted.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid, 2)),
        },
        &pack,
    )
    .expect("recommit turn");
    assert_eq!(recommitted.state.game.turn, 2);
    let view = published_view(&recommitted);
    let again = view.undo_checkpoint_id.expect("undo offered again");
    let restored_again = reduce(
        &recommitted.state,
        EngineCommand::RestoreCheckpoint {
            checkpoint_id: again,
        },
        &pack,
    )
    .expect("restore again");
    assert_eq!(restored_again.state.game.turn, 1);
}

#[test]
fn start_new_game_clears_the_undo_ring() {
    // 방어선: 새 세션의 게임에서 이전 플레이의 체크포인트로 되돌릴 경로가
    // 없어야 한다(선택 술어가 보호하지만 링 자체를 비운다).
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    assert!(!state.checkpoints.is_empty());

    let restarted = reduce(&state, EngineCommand::StartNewGame, &pack).expect("restart");
    assert!(
        restarted.state.checkpoints.is_empty(),
        "a fresh game starts with no rewind targets"
    );
}

#[test]
fn restore_with_unknown_checkpoint_id_is_noop() {
    let pack = sample_pack();
    let (state, undo_id, _gid) = two_accepted_turns(&pack);
    let before = state.game.clone();
    let noop = reduce(
        &state,
        EngineCommand::RestoreCheckpoint {
            checkpoint_id: uuid::Uuid::new_v4(),
        },
        &pack,
    )
    .expect("noop restore");
    assert_eq!(noop.state.game.turn, before.turn);
    assert!(
        noop.effects.is_empty()
            || !noop
                .effects
                .iter()
                .any(|e| matches!(e, EngineEffect::PersistStableState { .. })),
        "a no-op restore must not overwrite the autosave"
    );
    let _ = undo_id;
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
            lorebook_activations: Vec::new(),
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

// ---------------------------------------------------------------------------
// side-generation family (continue / suggest) + lorebook hot-replace +
// undo-ring persistence (AC-3 / AC-4 / AC-5, plan G3·G4·G9)
// ---------------------------------------------------------------------------

/// A narration-only draft for side-generation accepts. choices / patch /
/// summary are deliberately junk: the engine must discard them.
fn side_draft(generation_id: uuid::Uuid, narration: &str) -> GeneratedTurnDraft {
    GeneratedTurnDraft {
        generation_id,
        narration: narration.to_string(),
        choices: Vec::new(),
        proposed_patch: StatePatch::default(),
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

fn generation_request(t: &Transition) -> TurnRequest {
    t.effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::RequestGeneration { request, .. } => Some(request.clone()),
            _ => None,
        })
        .expect("a RequestGeneration effect")
}

fn constant_lore_entry(id: &str, content: &str) -> LorebookEntry {
    LorebookEntry {
        id: id.to_string(),
        keys: Vec::new(),
        secondary_keys: Vec::new(),
        logic: LorebookLogic::AndAny,
        content: content.to_string(),
        constant: true,
        enabled: true,
        insertion_order: 100,
        position: LorebookPosition::BeforeChar,
        case_sensitive: false,
        probability: 100,
        priority: None,
        comment: None,
        prevent_recursion: false,
        exclude_recursion: false,
        sticky: 0,
        cooldown: 0,
        delay: 0,
        vectorized: false,
    }
}

#[test]
fn continue_narration_appends_and_keeps_choices_and_state() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    let before = state.game.clone();

    let requested = reduce(&state, EngineCommand::ContinueNarration, &pack).expect("continue");
    let pending = requested.state.pending.as_ref().expect("continue pending");
    assert_eq!(
        pending.side_kind,
        Some(SideGenerationKind::Continue),
        "continue rides the side-generation discriminator"
    );
    let gid = pending.generation_id;
    assert_publish_view_present(&requested);

    let accepted = reduce(
        &requested.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(side_draft(gid, "복도 끝에서 유나가 다시 돌아섰다.")),
        },
        &pack,
    )
    .expect("accept continuation");

    let after = &accepted.state.game;
    assert_eq!(after.turn, before.turn, "no new turn");
    assert_eq!(after.history.len(), before.history.len(), "no new entry");
    assert_eq!(after.revision, before.revision + 1);
    let last = after.history.last().expect("committed turn");
    let before_last = before.history.last().expect("committed turn before");
    assert!(
        last.narration
            .contains("유나가 고개를 끄덕인다 (affection -1")
    );
    assert!(
        last.narration
            .ends_with("복도 끝에서 유나가 다시 돌아섰다."),
        "the continuation is appended: {:?}",
        last.narration
    );
    assert_eq!(last.choices, before_last.choices, "choices stay as-is");
    assert_eq!(
        after.characters.get(&cid()).unwrap().affection,
        before.characters.get(&cid()).unwrap().affection,
        "the side draft's patch is discarded"
    );
    assert!(accepted.audit.is_empty(), "no state patch was applied");
    assert_eq!(
        after.recent_summaries, before.recent_summaries,
        "no new arc digest without a new turn"
    );
}

#[test]
fn continue_accept_flows_through_autosave_and_keeps_the_ring_aligned() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    let current_turn = state.game.turn;

    let requested = reduce(&state, EngineCommand::ContinueNarration, &pack).expect("continue");
    let gid = requested.state.pending.as_ref().unwrap().generation_id;
    let accepted = reduce(
        &requested.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(side_draft(gid, "이어지는 문장이다.")),
        },
        &pack,
    )
    .expect("accept continuation");

    // Normal accept flow: the extended narration must be autosaved.
    assert!(
        accepted
            .effects
            .iter()
            .any(|e| matches!(e, EngineEffect::PersistStableState { .. })),
        "continue accept emits the autosave effect: {:?}",
        accepted.effects
    );

    // No divergence between the rewind ring and the committed state: the
    // current-turn checkpoint sees the extension (but never becomes the
    // one-key undo target).
    let ring_current = accepted
        .state
        .checkpoints
        .iter()
        .find(|cp| cp.game.turn == current_turn)
        .expect("current-turn checkpoint kept");
    assert!(
        ring_current
            .game
            .history
            .last()
            .unwrap()
            .narration
            .contains("이어지는 문장이다."),
        "ring snapshot sees the extended narration"
    );
    let view = published_view(&accepted);
    let undo_id = view.undo_checkpoint_id.expect("undo still offered");
    let undo_target = accepted
        .state
        .checkpoints
        .iter()
        .find(|cp| cp.id == undo_id)
        .unwrap();
    assert!(
        undo_target.game.turn < current_turn,
        "the undo target is still the previous turn, not the extension"
    );
}

#[test]
fn continue_is_ignored_while_a_generation_is_in_flight() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    let submitted = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let in_flight = submitted.state.pending.as_ref().unwrap().generation_id;

    let continue_noop = reduce(&submitted.state, EngineCommand::ContinueNarration, &pack)
        .expect("continue ignored");
    assert!(continue_noop.effects.is_empty());
    assert_eq!(
        continue_noop.state.pending.as_ref().unwrap().generation_id,
        in_flight,
        "the in-flight turn is untouched"
    );

    let suggest_noop =
        reduce(&submitted.state, EngineCommand::SuggestAction, &pack).expect("suggest ignored");
    assert!(suggest_noop.effects.is_empty());
}

#[test]
fn continue_is_ignored_while_an_error_overlay_is_up() {
    let pack = sample_pack();
    let (state, gid) = start_and_pending_id(&pack);
    let failed = reduce(
        &state,
        EngineCommand::ProviderFailed {
            generation_id: gid,
            error: DomainErrorView {
                code: "test".to_string(),
                title_ko: "t".to_string(),
                message_ko: "m".to_string(),
                recoverable: true,
            },
        },
        &pack,
    )
    .expect("provider failed");
    assert!(failed.state.retry_pending.is_some(), "overlay state");

    let noop =
        reduce(&failed.state, EngineCommand::ContinueNarration, &pack).expect("continue ignored");
    assert!(noop.effects.is_empty());
    assert!(noop.state.retry_pending.is_some());
}

#[test]
fn continue_requires_a_committed_turn() {
    let pack = sample_pack();
    let init = EngineState::initial(&pack);
    let noop = reduce(&init, EngineCommand::ContinueNarration, &pack).expect("noop");
    assert!(noop.effects.is_empty(), "nothing to extend before turn 1");
    assert!(noop.state.pending.is_none());
}

#[test]
fn suggest_stores_one_sentence_until_taken() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);

    let requested = reduce(&state, EngineCommand::SuggestAction, &pack).expect("suggest");
    let pending = requested.state.pending.as_ref().expect("suggest pending");
    assert_eq!(pending.side_kind, Some(SideGenerationKind::Suggest));
    let gid = pending.generation_id;

    let accepted = reduce(
        &requested.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(side_draft(gid, "먼저 창문을 닫는다")),
        },
        &pack,
    )
    .expect("accept suggestion");

    let mut state = accepted.state;
    assert!(state.pending.is_none());
    assert_eq!(
        state.take_suggested_action().as_deref(),
        Some("먼저 창문을 닫는다"),
        "the host collects the suggestion"
    );
    assert!(
        state.take_suggested_action().is_none(),
        "reading clears the buffer; the next suggestion overwrites"
    );
}

#[test]
fn suggest_accept_leaves_game_state_untouched() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    let before = state.game.clone();

    let requested = reduce(&state, EngineCommand::SuggestAction, &pack).expect("suggest");
    let gid = requested.state.pending.as_ref().unwrap().generation_id;
    let accepted = reduce(
        &requested.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(side_draft(gid, "자리를 일어난다")),
        },
        &pack,
    )
    .expect("accept suggestion");

    assert_eq!(
        serde_json_comparable(&accepted.state.game),
        serde_json_comparable(&before),
        "suggest commits nothing"
    );
    assert!(accepted.audit.is_empty());
    assert!(
        !accepted
            .effects
            .iter()
            .any(|e| matches!(e, EngineEffect::PersistStableState { .. })),
        "nothing changed, nothing to autosave"
    );
}

fn serde_json_comparable<T: serde::Serialize>(value: &T) -> story_model::serde_json::Value {
    story_model::serde_json::to_value(value).expect("value serializes")
}

#[test]
fn empty_continue_result_sets_the_flag_and_a_non_empty_one_clears_it() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    let before = state.game.clone();

    let requested = reduce(&state, EngineCommand::ContinueNarration, &pack).expect("continue");
    let gid = requested.state.pending.as_ref().unwrap().generation_id;
    let empty = reduce(
        &requested.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(side_draft(gid, "   \n")),
        },
        &pack,
    )
    .expect("accept empty continuation");

    // The empty result is flagged for the host (re-dispatch + toast) while
    // state and turn stay untouched.
    assert!(empty.state.empty_side_result, "empty side result flagged");
    assert!(
        published_view(&empty).empty_side_result,
        "the flag reaches the view"
    );
    assert_eq!(empty.state.game.turn, before.turn, "turn invariant kept");
    assert_eq!(
        empty.state.game.history.last().unwrap().narration,
        before.history.last().unwrap().narration,
        "no prose appended"
    );
    assert_eq!(
        empty.state.game.history.last().unwrap().choices,
        before.history.last().unwrap().choices,
        "choices untouched"
    );

    // The host re-dispatches the same side command once; a non-empty result
    // clears the flag and appends.
    let retried = reduce(&empty.state, EngineCommand::ContinueNarration, &pack)
        .expect("re-continue after empty result");
    let retry_gid = retried.state.pending.as_ref().unwrap().generation_id;
    let ok = reduce(
        &retried.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(side_draft(retry_gid, "복도 끝에서 유나가 다시 돌아섰다.")),
        },
        &pack,
    )
    .expect("accept retried continuation");

    assert!(
        !ok.state.empty_side_result,
        "a non-empty side accept clears the flag"
    );
    assert!(!published_view(&ok).empty_side_result);
    assert!(
        ok.state
            .game
            .history
            .last()
            .unwrap()
            .narration
            .ends_with("복도 끝에서 유나가 다시 돌아섰다.")
    );
}

#[test]
fn a_new_regular_turn_request_retires_the_empty_side_flag() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);

    let requested = reduce(&state, EngineCommand::ContinueNarration, &pack).expect("continue");
    let gid = requested.state.pending.as_ref().unwrap().generation_id;
    let empty = reduce(
        &requested.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(side_draft(gid, "")),
        },
        &pack,
    )
    .expect("accept empty continuation");
    assert!(empty.state.empty_side_result);

    let submitted = reduce(
        &empty.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit next turn");
    assert!(
        !submitted.state.empty_side_result,
        "a new regular turn retires the stale flag"
    );
}

#[test]
fn empty_scene_and_suggest_results_flag_without_touching_state() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);

    let scene = reduce(&state, EngineCommand::GroupScene, &pack).expect("scene");
    let scene_gid = scene.state.pending.as_ref().unwrap().generation_id;
    let empty_scene = reduce(
        &scene.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(side_draft(scene_gid, "")),
        },
        &pack,
    )
    .expect("accept empty scene");
    assert!(empty_scene.state.empty_side_result, "empty scene flagged");

    let suggest = reduce(&state, EngineCommand::SuggestAction, &pack).expect("suggest");
    let suggest_gid = suggest.state.pending.as_ref().unwrap().generation_id;
    let empty_suggest = reduce(
        &suggest.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(side_draft(suggest_gid, "  ")),
        },
        &pack,
    )
    .expect("accept empty suggestion");
    assert!(
        empty_suggest.state.empty_side_result,
        "empty suggestion flagged"
    );
    let mut suggested_state = empty_suggest.state;
    assert!(
        suggested_state.take_suggested_action().is_none(),
        "an empty suggestion is not stored"
    );
}

#[test]
fn set_lorebook_entries_swaps_entries_for_the_next_turn() {
    let mut pack = sample_pack();
    pack.lorebook = Lorebook {
        name: "test".to_string(),
        entries: vec![constant_lore_entry("old", "옛 로어 내용")],
        ..Lorebook::default()
    };
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    let activations_before = state.game.lorebook_activations.clone();

    // Baseline: the next turn's prompt carries the pack's entry.
    let baseline = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("baseline submit");
    assert!(
        generation_request(&baseline).messages[1]
            .content
            .contains("옛 로어 내용")
    );

    // Hot-replace: the swap itself changes no game state and emits nothing.
    let swapped = reduce(
        &state,
        EngineCommand::SetLorebookEntries {
            entries: vec![constant_lore_entry("new", "새 로어 내용")],
        },
        &pack,
    )
    .expect("swap");
    assert!(swapped.effects.is_empty(), "the view stays unchanged");
    assert_eq!(
        swapped.state.game.turn, state.game.turn,
        "the command does not touch game state"
    );
    assert_eq!(
        swapped.state.game.lorebook_activations, activations_before,
        "the swap alone records no activations"
    );

    // The NEXT turn matches against the new entries only.
    let resubmitted = reduce(
        &swapped.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit after swap");
    let request = generation_request(&resubmitted);
    assert!(request.messages[1].content.contains("새 로어 내용"));
    assert!(
        !request.messages[1].content.contains("옛 로어 내용"),
        "the old entry is gone"
    );
    let upcoming = swapped.state.game.turn + 1;
    assert!(
        resubmitted
            .state
            .game
            .lorebook_activations
            .iter()
            .any(|record| record.entry_id == "new" && record.turn == upcoming),
        "the activation ledger uses the overridden entries"
    );
}

#[test]
fn set_lorebook_entries_is_ignored_while_loading() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    let submitted = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let gid = submitted.state.pending.as_ref().unwrap().generation_id;

    let noop = reduce(
        &submitted.state,
        EngineCommand::SetLorebookEntries {
            entries: vec![constant_lore_entry("new", "새 로어 내용")],
        },
        &pack,
    )
    .expect("swap ignored while loading");
    assert!(noop.state.lorebook_override.is_none());
    assert_eq!(
        noop.state.pending.as_ref().unwrap().generation_id,
        gid,
        "the in-flight generation is untouched"
    );
}

#[test]
fn start_new_game_resets_the_lorebook_override_and_suggestion() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    let swapped = reduce(
        &state,
        EngineCommand::SetLorebookEntries {
            entries: vec![constant_lore_entry("new", "새 로어 내용")],
        },
        &pack,
    )
    .expect("swap");
    let mut with_suggestion = swapped.state;
    assert!(with_suggestion.lorebook_override.is_some());
    with_suggestion.suggested_action = Some("남아 있는다".to_string());

    let restarted = reduce(&with_suggestion, EngineCommand::StartNewGame, &pack).expect("restart");
    assert!(
        restarted.state.lorebook_override.is_none(),
        "a fresh game reverts to pack canon"
    );
    assert!(restarted.state.suggested_action.is_none());
}

#[test]
fn undo_ring_round_trips_through_the_save_payload() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    assert_eq!(state.checkpoints.len(), 2);

    let payload: SavePayload = state.snapshot_payload();
    assert_eq!(payload.undo_stack.len(), 2);
    assert_eq!(
        serde_json_comparable(payload.undo_stack.last().unwrap()),
        serde_json_comparable(&state.game),
        "the newest ring entry is the committed state"
    );

    let bytes = story_model::serde_json::to_vec(&payload).expect("serialize payload");
    let back: SavePayload = serde_json_parse(&bytes);
    let restored = EngineState::from_snapshot_payload(&pack, back);
    assert_eq!(restored.checkpoints.len(), 2);
    assert!(restored.pending.is_none());

    // Undo works straight after a load (AC-4).
    let view = build_view(&restored, &pack, None, None);
    let undo_id = view.undo_checkpoint_id.expect("undo offered after restore");
    let rewound = reduce(
        &restored,
        EngineCommand::RestoreCheckpoint {
            checkpoint_id: undo_id,
        },
        &pack,
    )
    .expect("restore after load");
    assert_eq!(rewound.state.game.turn, 1);
}

fn serde_json_parse(bytes: &[u8]) -> SavePayload {
    story_model::serde_json::from_slice(bytes).expect("payload deserializes")
}

#[test]
fn undo_ring_restore_enforces_the_eight_entry_bound() {
    let pack = sample_pack();
    let mut state = EngineState::initial(&pack);
    state.checkpoints = (1..=10)
        .map(|turn| {
            let mut game = state.game.clone();
            game.turn = turn;
            Checkpoint {
                id: uuid::Uuid::new_v4(),
                game,
            }
        })
        .collect();

    let payload = state.snapshot_payload();
    assert_eq!(payload.undo_stack.len(), 10);
    let restored = EngineState::from_snapshot_payload(&pack, payload);
    assert_eq!(
        restored.checkpoints.len(),
        8,
        "the ring keeps its in-memory bound on restore"
    );
    assert_eq!(
        restored.checkpoints.first().unwrap().game.turn,
        3,
        "the OLDEST entries were dropped, newest kept"
    );
}

#[test]
fn old_save_without_undo_stack_loads_with_an_empty_ring() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    let mut value = serde_json_comparable(&state.snapshot_payload());
    value
        .as_object_mut()
        .expect("payload is an object")
        .remove("undo_stack");

    let legacy: SavePayload =
        story_model::serde_json::from_value(value).expect("legacy payload loads");
    assert!(legacy.undo_stack.is_empty());
    let restored = EngineState::from_snapshot_payload(&pack, legacy);
    assert!(
        restored.checkpoints.is_empty(),
        "an old save behaves exactly as before: no rewind targets"
    );
    let view = build_view(&restored, &pack, None, None);
    assert!(view.undo_checkpoint_id.is_none());
}

#[test]
fn continue_request_records_no_lorebook_activations() {
    let mut pack = sample_pack();
    pack.lorebook = Lorebook {
        name: "test".to_string(),
        entries: vec![constant_lore_entry("always", "상시 로어.")],
        ..Lorebook::default()
    };
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    // Seed the ledger with the upcoming turn's record so the guard is
    // observable: a genuine turn request would retain-and-append for
    // turn + 1, a continuation must change nothing.
    let upcoming = state.game.turn + 1;
    let mut seeded = state;
    seeded.game.lorebook_activations = vec![story_model::LorebookActivation {
        turn: upcoming,
        entry_id: "always".to_string(),
    }];

    let requested = reduce(&seeded, EngineCommand::ContinueNarration, &pack).expect("continue");
    assert!(
        !requested.effects.is_empty(),
        "the continuation request is emitted"
    );
    assert_eq!(
        requested.state.game.lorebook_activations.len(),
        1,
        "a continuation opens no turn, so the ledger stays untouched: {:?}",
        requested.state.game.lorebook_activations
    );
}

#[test]
fn continue_forces_the_single_pass_path_even_on_the_two_stage_setting() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    let mut two_stage = state;
    two_stage.use_director_writer = true;

    let requested = reduce(&two_stage, EngineCommand::ContinueNarration, &pack).expect("continue");
    assert!(
        requested
            .effects
            .iter()
            .any(|e| matches!(e, EngineEffect::RequestGeneration { .. })),
        "side generations never ride the Director→Writer path"
    );
    assert!(
        !requested
            .effects
            .iter()
            .any(|e| matches!(e, EngineEffect::RequestDirector { .. })),
    );
}

// ---------------------------------------------------------------------------
// group scene (SideGenerationKind::Scene, AC-6 group slice)
// ---------------------------------------------------------------------------

#[test]
fn group_scene_attaches_a_conversation_bit_without_a_new_turn() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    let before = state.game.clone();
    let checkpoints_before = state.checkpoints.len();

    let requested = reduce(&state, EngineCommand::GroupScene, &pack).expect("group scene");
    let pending = requested.state.pending.as_ref().expect("scene pending");
    assert_eq!(
        pending.side_kind,
        Some(SideGenerationKind::Scene),
        "the scene rides the side-generation discriminator"
    );
    let gid = pending.generation_id;
    assert_publish_view_present(&requested);

    // The draft's choices and patch are format-preserving junk: the accept
    // path must discard both.
    let mut draft = side_draft(gid, "나무: \"먼저 가볼게.\"\n유나: \"…그래, 따라갈게.\"");
    draft.choices = vec![choice("junk", "버려질 선택지", RiskLevel::Safe)];
    let accepted = reduce(
        &requested.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(draft),
        },
        &pack,
    )
    .expect("accept scene");

    let after = &accepted.state.game;
    assert_eq!(after.turn, before.turn, "no new turn");
    assert_eq!(after.history.len(), before.history.len(), "no new entry");
    assert_eq!(after.revision, before.revision + 1);
    let last = after.history.last().expect("committed turn");
    let before_last = before.history.last().expect("committed turn before");
    assert!(
        last.narration
            .ends_with("나무: \"먼저 가볼게.\"\n유나: \"…그래, 따라갈게.\""),
        "the scene bit is appended: {:?}",
        last.narration
    );
    assert_eq!(
        last.choices, before_last.choices,
        "the scene draft's choices are discarded"
    );
    assert_eq!(
        after.characters.get(&cid()).unwrap().affection,
        before.characters.get(&cid()).unwrap().affection,
        "the scene draft's patch is discarded — a scene never moves the meters"
    );
    assert!(accepted.audit.is_empty(), "no state patch was applied");
    assert_eq!(
        accepted.state.checkpoints.len(),
        checkpoints_before,
        "same-identity upsert: no new rewind target"
    );
    assert!(
        accepted
            .effects
            .iter()
            .any(|e| matches!(e, EngineEffect::PersistStableState { .. })),
        "the extended narration is autosaved like a continue accept"
    );
}

#[test]
fn group_scene_is_ignored_while_pending_or_errored_and_before_the_first_turn() {
    let pack = sample_pack();
    let init = EngineState::initial(&pack);
    let noop = reduce(&init, EngineCommand::GroupScene, &pack).expect("noop");
    assert!(
        noop.effects.is_empty(),
        "nothing to attach to before the opening is accepted"
    );
    assert!(noop.state.pending.is_none());

    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    let submitted = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let in_flight = submitted.state.pending.as_ref().unwrap().generation_id;
    let scene_noop = reduce(&submitted.state, EngineCommand::GroupScene, &pack).expect("ignored");
    assert!(scene_noop.effects.is_empty());
    assert_eq!(
        scene_noop.state.pending.as_ref().unwrap().generation_id,
        in_flight,
        "the in-flight turn is untouched"
    );

    let failed = reduce(
        &submitted.state,
        EngineCommand::ProviderFailed {
            generation_id: in_flight,
            error: DomainErrorView {
                code: "test".to_string(),
                title_ko: "t".to_string(),
                message_ko: "m".to_string(),
                recoverable: true,
            },
        },
        &pack,
    )
    .expect("provider failed");
    assert!(failed.state.retry_pending.is_some(), "overlay state");
    let errored_noop = reduce(&failed.state, EngineCommand::GroupScene, &pack).expect("ignored");
    assert!(errored_noop.effects.is_empty());
    assert!(errored_noop.state.retry_pending.is_some());
}

#[test]
fn group_scene_forces_the_single_pass_path_even_on_the_two_stage_setting() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    let mut two_stage = state;
    two_stage.use_director_writer = true;

    let requested = reduce(&two_stage, EngineCommand::GroupScene, &pack).expect("group scene");
    assert!(
        requested
            .effects
            .iter()
            .any(|e| matches!(e, EngineEffect::RequestGeneration { .. })),
        "the scene never rides the Director→Writer path"
    );
    assert!(
        !requested
            .effects
            .iter()
            .any(|e| matches!(e, EngineEffect::RequestDirector { .. })),
    );
}

#[test]
fn build_view_publishes_the_cast_roster_from_pack_and_game_characters() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);

    let view = build_view(&state, &pack, None, None);
    assert_eq!(view.cast.len(), 1, "one declared character");
    let member = &view.cast[0];
    assert_eq!(member.id.0, NAMU);
    assert_eq!(member.name, "나무", "display name joined from the pack");
    assert_eq!(member.relationship_stage, RelationshipStage::Acquaintance);
    assert_eq!(member.current_emotion, Emotion::Neutral);
    assert_eq!(
        (member.affection, member.trust, member.tension),
        (state.game.characters.get(&cid()).unwrap().affection, 0, 0)
    );
}

// ---------------------------------------------------------------------------
// F-3: the activation ledger must survive accept (sticky/cooldown timers)
// ---------------------------------------------------------------------------

/// Keyword entry with sticky/cooldown timers.
fn timed_lore_entry(
    id: &str,
    key: &str,
    content: &str,
    sticky: u32,
    cooldown: u32,
) -> LorebookEntry {
    LorebookEntry {
        id: id.to_string(),
        keys: vec![key.to_string()],
        secondary_keys: Vec::new(),
        logic: LorebookLogic::AndAny,
        content: content.to_string(),
        constant: false,
        enabled: true,
        insertion_order: 100,
        position: LorebookPosition::BeforeChar,
        case_sensitive: false,
        probability: 100,
        priority: None,
        comment: None,
        prevent_recursion: false,
        exclude_recursion: false,
        sticky,
        cooldown,
        delay: 0,
        vectorized: false,
    }
}

fn accept_draft(
    state: &EngineState,
    pack: &StoryPack,
    draft: GeneratedTurnDraft,
) -> Result<Transition, story_core::error::CoreError> {
    reduce(
        state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(draft),
        },
        pack,
    )
}

/// Opening(턴 1) 수락까지 진행한 상태. `opening_narration`이 스캔 키를 갖는다.
fn opening_with_narration(pack: &StoryPack, opening_narration: &str) -> EngineState {
    let (state, gid) = start_and_pending_id(pack);
    let mut draft = valid_draft(gid, 1);
    draft.narration = opening_narration.to_string();
    accept_draft(&state, pack, draft)
        .expect("accept opening")
        .state
}

fn ledger_has(state: &EngineState, turn: u32, entry_id: &str) -> bool {
    state
        .game
        .lorebook_activations
        .iter()
        .any(|record| record.turn == turn && record.entry_id == entry_id)
}

#[test]
fn activation_ledger_survives_accept_and_keeps_sticky_hot() {
    let mut pack = sample_pack();
    pack.lorebook = Lorebook {
        name: "test".to_string(),
        scan_depth: 1,
        entries: vec![timed_lore_entry("expo", "전시회", "전시회의 기억.", 2, 0)],
        ..Lorebook::default()
    };
    // 턴 1 내레이션이 키를 갖는다 → 턴 2 요청에서 발화.
    let opened = opening_with_narration(&pack, "그날 전시회를 함께 다녀왔다.");
    assert!(
        opened.game.lorebook_activations.is_empty(),
        "the opening request had no scan corpus"
    );

    let submitted = reduce(
        &opened,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    assert!(
        submitted
            .state
            .pending
            .as_ref()
            .unwrap()
            .lorebook_activations
            .iter()
            .any(|record| record.turn == 2 && record.entry_id == "expo"),
        "the request-time batch rides the pending generation"
    );
    let turn_gid = submitted.state.pending.as_ref().unwrap().generation_id;

    // F-3 본체: 수락이 기록 이전 체크포인트에서 게임을 재구성해도 원장이
    // 남아 있어야 한다.
    let accepted =
        accept_draft(&submitted.state, &pack, valid_draft(turn_gid, 1)).expect("accept turn 2");
    assert_eq!(accepted.state.game.turn, 2);
    assert!(
        ledger_has(&accepted.state, 2, "expo"),
        "the activation batch recorded at request time survives the accept: {:?}",
        accepted.state.game.lorebook_activations
    );

    // Sticky 유지: 턴 2 내레이션엔 키가 없지만(스캔 depth 1) 원장의 기록이
    // 턴 3에서 강제 발화시킨다.
    let resubmitted = reduce(
        &accepted.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("resubmit");
    assert!(
        generation_request(&resubmitted).messages[1]
            .content
            .contains("전시회의 기억"),
        "sticky keeps the entry hot after the accept"
    );
}

#[test]
fn activation_ledger_survives_accept_and_keeps_cooldown_cold() {
    let mut pack = sample_pack();
    pack.lorebook = Lorebook {
        name: "test".to_string(),
        scan_depth: 1,
        entries: vec![timed_lore_entry("gated", "전시회", "전시회의 기억.", 0, 3)],
        ..Lorebook::default()
    };
    // 턴 1·2 내레이션 모두 키를 갖는다 → 쿨다운이 없다면 턴 3에서도 발화한다.
    let opened = opening_with_narration(&pack, "그날 전시회를 함께 다녀왔다.");
    let submitted = reduce(
        &opened,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let turn_gid = submitted.state.pending.as_ref().unwrap().generation_id;
    let mut turn2 = valid_draft(turn_gid, 1);
    turn2.narration = "다음 날 전시회 포스터 앞에서 다시 만났다.".to_string();
    let accepted = accept_draft(&submitted.state, &pack, turn2).expect("accept turn 2");
    assert!(
        ledger_has(&accepted.state, 2, "gated"),
        "the ledger survives the accept"
    );

    // 쿨다운 유지: 키가 살아 있는데도(턴 2 내레이션) 원장 기록이 막는다.
    let resubmitted = reduce(
        &accepted.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("resubmit inside cooldown");
    assert!(
        !generation_request(&resubmitted).messages[1]
            .content
            .contains("전시회의 기억"),
        "cooldown stays cold across the accept"
    );
}

#[test]
fn cooldown_survives_undo_and_recommit_without_refiring() {
    let mut pack = sample_pack();
    pack.lorebook = Lorebook {
        name: "test".to_string(),
        scan_depth: 1,
        entries: vec![timed_lore_entry("gated", "전시회", "전시회의 기억.", 0, 3)],
        ..Lorebook::default()
    };
    let opened = opening_with_narration(&pack, "그날 전시회를 함께 다녀왔다.");
    let submitted = reduce(
        &opened,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let turn_gid = submitted.state.pending.as_ref().unwrap().generation_id;
    let mut turn2 = valid_draft(turn_gid, 1);
    turn2.narration = "다음 날 전시회 포스터 앞에서 다시 만났다.".to_string();
    let accepted = accept_draft(&submitted.state, &pack, turn2).expect("accept turn 2");
    let undo_id = published_view(&accepted)
        .undo_checkpoint_id
        .expect("undo offered");

    let restored = reduce(
        &accepted.state,
        EngineCommand::RestoreCheckpoint {
            checkpoint_id: undo_id,
        },
        &pack,
    )
    .expect("restore");
    assert_eq!(restored.state.game.turn, 1);
    assert!(
        restored
            .state
            .game
            .lorebook_activations
            .iter()
            .all(|record| record.turn < 2),
        "the undone turn's records stay out of the rewound state: {:?}",
        restored.state.game.lorebook_activations
    );

    // 같은 선택으로 턴 2를 재확정한다(내레이션에 키 유지).
    let resubmitted = reduce(
        &restored.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("resubmit");
    let retry_gid = resubmitted.state.pending.as_ref().unwrap().generation_id;
    let mut retry = valid_draft(retry_gid, 1);
    retry.narration = "다시 그 전시회 앞에 섰다.".to_string();
    let recommitted = accept_draft(&resubmitted.state, &pack, retry).expect("recommit turn 2");
    assert_eq!(recommitted.state.game.turn, 2);
    assert!(
        ledger_has(&recommitted.state, 2, "gated"),
        "the re-committed attempt re-records and survives its accept"
    );

    // 되돌리기+재확정 직후 턴 3: 키가 살아 있어도 쿨다운이 막는다.
    let after = reduce(
        &recommitted.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit after undo+recommit");
    assert!(
        !generation_request(&after).messages[1]
            .content
            .contains("전시회의 기억"),
        "a cooldown entry must not instantly re-fire after undo+re-commit"
    );
}

// ---------------------------------------------------------------------------
// 변형 순환 (AC-2): undo 시드 → 커밋 목록 → CycleTurnVariant
// ---------------------------------------------------------------------------

/// 턴 2 확정 → undo까지 진행: (되돌려진 상태, 버려진 턴 2 내레이션).
fn undone_second_turn(pack: &StoryPack) -> (EngineState, String) {
    let (state, undo_id, _gid) = two_accepted_turns(pack);
    let discarded = state.game.history.last().unwrap().narration.clone();
    let restored = reduce(
        &state,
        EngineCommand::RestoreCheckpoint {
            checkpoint_id: undo_id,
        },
        pack,
    )
    .expect("restore");
    (restored.state, discarded)
}

/// undo로 시딩된 풀 위에 턴 2를 새 내레이션으로 재확정한다.
fn recommitted_with_variants(pack: &StoryPack) -> (EngineState, String, String) {
    let (restored, discarded) = undone_second_turn(pack);
    let resubmitted = reduce(
        &restored,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        pack,
    )
    .expect("resubmit");
    let gid = resubmitted.state.pending.as_ref().unwrap().generation_id;
    let transition =
        accept_draft(&resubmitted.state, pack, valid_draft(gid, 2)).expect("recommit turn 2");
    let accepted = transition
        .state
        .game
        .history
        .last()
        .unwrap()
        .narration
        .clone();
    (transition.state, accepted, discarded)
}

#[test]
fn undo_seeds_the_variant_pool_and_commit_builds_the_list() {
    let pack = sample_pack();
    let (restored, discarded) = undone_second_turn(&pack);
    assert_eq!(restored.game.turn, 1);
    assert_eq!(
        restored.variant_pool.get(&2).map(Vec::as_slice),
        Some(&[discarded.clone()][..]),
        "the undone narration is pooled for its turn"
    );
    assert_eq!(restored.current_variants, None, "cycle resets on undo");
    assert_eq!(build_view(&restored, &pack, None, None).variant_label, None);

    let (recommitted, accepted, discarded) = recommitted_with_variants(&pack);
    assert_ne!(accepted, discarded, "the retry produced different prose");
    assert_eq!(
        recommitted.current_variants,
        Some((2, 0, vec![accepted.clone(), discarded.clone()])),
        "[accepted, ...undone attempts], accepted first"
    );
    let view = build_view(&recommitted, &pack, None, None);
    assert_eq!(view.variant_label.as_deref(), Some("변형 1/2"));
    assert_eq!(view.narration, accepted);
}

#[test]
fn cycle_rotates_narration_badge_and_persists() {
    let pack = sample_pack();
    let (state, accepted, discarded) = recommitted_with_variants(&pack);

    let cycled =
        reduce(&state, EngineCommand::CycleTurnVariant { delta: 1 }, &pack).expect("cycle forward");
    assert_eq!(
        cycled.state.game.history.last().unwrap().narration,
        discarded,
        "the displayed narration is the next variant"
    );
    assert_eq!(
        cycled.state.current_variants.as_ref().map(|(_, i, _)| *i),
        Some(1)
    );
    let view = published_view(&cycled);
    assert_eq!(view.narration, discarded);
    assert_eq!(view.variant_label.as_deref(), Some("변형 2/2"));
    assert!(
        cycled.effects.iter().any(|effect| matches!(
            effect,
            EngineEffect::PersistStableState { slot_id, .. } if slot_id == AUTOSAVE_SLOT
        )),
        "the shown variant is autosaved: {:?}",
        cycled.effects
    );
    let ring = cycled
        .state
        .checkpoints
        .iter()
        .find(|checkpoint| checkpoint.game.turn == 2)
        .expect("current-turn snapshot kept");
    assert_eq!(
        ring.game.history.last().unwrap().narration,
        discarded,
        "the ring snapshot sees the rotated narration"
    );

    // 앞으로 감기: 끝에서 첫 변형(수용본)으로 되돌아온다.
    let wrapped = reduce(
        &cycled.state,
        EngineCommand::CycleTurnVariant { delta: 1 },
        &pack,
    )
    .expect("wrap forward");
    assert_eq!(
        wrapped.state.current_variants.as_ref().map(|(_, i, _)| *i),
        Some(0)
    );
    assert_eq!(
        wrapped.state.game.history.last().unwrap().narration,
        accepted
    );

    // 뒤로 감기: 0에서 마지막 변형으로 넘어간다.
    let back = reduce(
        &wrapped.state,
        EngineCommand::CycleTurnVariant { delta: -1 },
        &pack,
    )
    .expect("wrap backward");
    assert_eq!(
        back.state.current_variants.as_ref().map(|(_, i, _)| *i),
        Some(1)
    );
    assert_eq!(back.state.game.history.last().unwrap().narration, discarded);
}

#[test]
fn cycle_is_identity_while_loading_or_errored() {
    let pack = sample_pack();
    let (state, accepted, _discarded) = recommitted_with_variants(&pack);

    let submitted = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let gid = submitted.state.pending.as_ref().unwrap().generation_id;
    let loading_noop = reduce(
        &submitted.state,
        EngineCommand::CycleTurnVariant { delta: 1 },
        &pack,
    )
    .expect("cycle ignored while loading");
    assert!(loading_noop.effects.is_empty());
    assert_eq!(
        loading_noop.state.pending.as_ref().unwrap().generation_id,
        gid,
        "the in-flight generation is untouched"
    );
    assert_eq!(
        loading_noop.state.game.history.last().unwrap().narration,
        accepted,
        "identity while loading keeps the displayed narration"
    );

    let failed = reduce(
        &submitted.state,
        EngineCommand::ProviderFailed {
            generation_id: gid,
            error: DomainErrorView {
                code: "test".to_string(),
                title_ko: "t".to_string(),
                message_ko: "m".to_string(),
                recoverable: true,
            },
        },
        &pack,
    )
    .expect("provider failed");
    assert!(failed.state.retry_pending.is_some(), "error overlay state");
    let error_noop = reduce(
        &failed.state,
        EngineCommand::CycleTurnVariant { delta: 1 },
        &pack,
    )
    .expect("cycle ignored while errored");
    assert!(error_noop.effects.is_empty());
    assert!(error_noop.state.retry_pending.is_some());
}

#[test]
fn cycle_is_noop_without_multiple_variants() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    // 풀 시딩 없는 평범한 커밋: 순환할 것이 없다.
    let before = state.game.history.last().unwrap().narration.clone();
    let noop = reduce(&state, EngineCommand::CycleTurnVariant { delta: 1 }, &pack).expect("noop");
    assert!(noop.effects.is_empty());
    assert_eq!(noop.state.game.history.last().unwrap().narration, before);

    // 요소 1개짜리 목록도 그대로(n <= 1 no-op).
    let mut single = state;
    single.current_variants = Some((single.game.turn, 0, vec!["유일한 본문".to_string()]));
    let single_noop = reduce(&single, EngineCommand::CycleTurnVariant { delta: 1 }, &pack)
        .expect("noop for a single variant");
    assert!(single_noop.effects.is_empty());
    assert_eq!(
        single_noop.state.current_variants,
        Some((single.game.turn, 0, vec!["유일한 본문".to_string()]))
    );

    // 턴이 어긋난 낡은 목록도 무시한다.
    single.current_variants = Some((
        single.game.turn + 1,
        0,
        vec!["지난 턴".into(), "또 다른".into()],
    ));
    let stale_noop = reduce(&single, EngineCommand::CycleTurnVariant { delta: 1 }, &pack)
        .expect("noop for a stale list");
    assert!(stale_noop.effects.is_empty());
}

#[test]
fn start_new_game_clears_the_variant_pool() {
    let pack = sample_pack();
    let (restored, _discarded) = undone_second_turn(&pack);
    assert!(!restored.variant_pool.is_empty());

    let restarted = reduce(&restored, EngineCommand::StartNewGame, &pack).expect("restart");
    assert!(
        restarted.state.variant_pool.is_empty(),
        "variants belong to the session that generated them"
    );
    assert_eq!(restarted.state.current_variants, None);
}

#[test]
fn variant_pool_and_list_cap_at_eight_entries_per_turn() {
    let pack = sample_pack();
    let (state, undo_id, _gid) = two_accepted_turns(&pack);
    let discarded = state.game.history.last().unwrap().narration.clone();
    let mut state = state;
    // 지난 undo 사이클들을 직접 시딩으로 대신한다(원장 시딩 테스트와 같은
    // 관례). 한 체인으로는 링 상한 때문에 8번의 undo를 쌓을 수 없다.
    state.variant_pool = BTreeMap::from([(
        2,
        (0..8).map(|index| format!("오래된 변형 {index}")).collect(),
    )]);

    let restored = reduce(
        &state,
        EngineCommand::RestoreCheckpoint {
            checkpoint_id: undo_id,
        },
        &pack,
    )
    .expect("restore");
    let pool = restored.state.variant_pool.get(&2).expect("pool");
    assert_eq!(pool.len(), 8, "the seeding push respects the per-turn cap");
    assert_eq!(
        pool.last().unwrap(),
        &discarded,
        "the newest attempt is kept, the oldest seeded entry dropped"
    );

    // 재확정: [수용본 + 풀 8개]도 같은 상한 8으로 잘린다.
    let resubmitted = reduce(
        &restored.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("resubmit");
    let gid = resubmitted.state.pending.as_ref().unwrap().generation_id;
    let recommitted =
        accept_draft(&resubmitted.state, &pack, valid_draft(gid, 2)).expect("recommit turn 2");
    let Some((_, 0, variants)) = recommitted.state.current_variants.as_ref() else {
        panic!("a pooled commit builds a variant list");
    };
    assert_eq!(variants.len(), VARIANT_CAP, "the list is capped too");
    assert_eq!(
        variants[0],
        recommitted.state.game.history.last().unwrap().narration
    );
}

#[test]
fn commit_dedupes_the_variant_list_against_the_accepted_narration() {
    let pack = sample_pack();
    let fixed = "유나는 창가에 서서 편지를 다시 읽었다.".to_string();

    let (state, gid) = start_and_pending_id(&pack);
    let opened = accept_draft(&state, &pack, valid_draft(gid, 1)).expect("opening");
    let submitted = reduce(
        &opened.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let turn_gid = submitted.state.pending.as_ref().unwrap().generation_id;
    let mut draft = valid_draft(turn_gid, -1);
    draft.narration = fixed.clone();
    let accepted = accept_draft(&submitted.state, &pack, draft).expect("accept turn 2");
    let undo_id = published_view(&accepted)
        .undo_checkpoint_id
        .expect("undo offered");
    let restored = reduce(
        &accepted.state,
        EngineCommand::RestoreCheckpoint {
            checkpoint_id: undo_id,
        },
        &pack,
    )
    .expect("restore");
    assert_eq!(
        restored.state.variant_pool.get(&2),
        Some(&vec![fixed.clone()]),
        "the discarded narration is pooled"
    );

    // 같은 내레이션으로 재확정하면 목록이 1개뿐이라 순환하지 않는다.
    let resubmitted = reduce(
        &restored.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("resubmit");
    let retry_gid = resubmitted.state.pending.as_ref().unwrap().generation_id;
    let mut retry = valid_draft(retry_gid, 1);
    retry.narration = fixed.clone();
    let recommitted = accept_draft(&resubmitted.state, &pack, retry).expect("recommit turn 2");
    assert_eq!(
        recommitted.state.current_variants, None,
        "a single distinct narration has nothing to cycle"
    );
    assert_eq!(published_view(&recommitted).variant_label, None);
}

// ---------------------------------------------------------------------------
// History 내레이션 편집·삭제 (이슈 #3)
// ---------------------------------------------------------------------------

#[test]
fn edit_history_narration_rewrites_prose_and_persists() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    let entry_id = state.game.history.last().unwrap().id;
    let before_rev = state.game.revision;

    let edited = reduce(
        &state,
        EngineCommand::EditHistoryNarration {
            entry_id,
            narration: "  교정된 내레이션입니다.  ".to_string(),
        },
        &pack,
    )
    .expect("edit");

    assert_eq!(
        edited.state.game.history.last().unwrap().narration,
        "교정된 내레이션입니다."
    );
    assert_eq!(
        published_view(&edited).narration,
        "교정된 내레이션입니다.",
        "last-turn edit republishes the story view"
    );
    assert!(edited.state.game.revision > before_rev);
    assert!(
        edited
            .effects
            .iter()
            .any(|effect| matches!(effect, EngineEffect::PersistStableState { .. })),
        "autosave must follow a successful edit"
    );
}

#[test]
fn edit_history_narration_rejects_empty_and_unknown_ids() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    let entry_id = state.game.history.last().unwrap().id;
    let original = state.game.history.last().unwrap().narration.clone();

    let empty = reduce(
        &state,
        EngineCommand::EditHistoryNarration {
            entry_id,
            narration: "   \n\t  ".to_string(),
        },
        &pack,
    )
    .expect("empty");
    assert_eq!(empty.state.game.history.last().unwrap().narration, original);
    assert!(empty.effects.is_empty());

    let missing = reduce(
        &state,
        EngineCommand::EditHistoryNarration {
            entry_id: uuid::Uuid::nil(),
            narration: "없는 턴".to_string(),
        },
        &pack,
    )
    .expect("missing");
    assert_eq!(
        missing.state.game.history.last().unwrap().narration,
        original
    );
    assert!(missing.effects.is_empty());
}

#[test]
fn edit_history_narration_keeps_choices_and_patches() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    let entry = state.game.history.last().unwrap().clone();

    let edited = reduce(
        &state,
        EngineCommand::EditHistoryNarration {
            entry_id: entry.id,
            narration: "패치는 그대로".to_string(),
        },
        &pack,
    )
    .expect("edit");
    let after = edited.state.game.history.last().unwrap();
    assert_eq!(after.choices, entry.choices);
    assert_eq!(after.applied_patch, entry.applied_patch);
    assert_eq!(after.player_action, entry.player_action);
}

#[test]
fn delete_history_from_truncates_suffix_via_checkpoint() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    assert_eq!(state.game.turn, 2);
    let second_id = state.game.history.last().unwrap().id;

    let deleted = reduce(
        &state,
        EngineCommand::DeleteHistoryFrom {
            entry_id: second_id,
        },
        &pack,
    )
    .expect("delete");

    assert_eq!(deleted.state.game.turn, 1);
    assert_eq!(deleted.state.game.history.len(), 1);
    assert!(
        deleted
            .effects
            .iter()
            .any(|effect| matches!(effect, EngineEffect::PersistStableState { .. })),
        "delete must rewrite autosave like undo"
    );
}

#[test]
fn delete_history_from_refuses_opening_turn() {
    let pack = sample_pack();
    let (state, _undo_id, _gid) = two_accepted_turns(&pack);
    let opening_id = state.game.history.first().unwrap().id;

    let refused = reduce(
        &state,
        EngineCommand::DeleteHistoryFrom {
            entry_id: opening_id,
        },
        &pack,
    )
    .expect("refuse opening");
    assert_eq!(refused.state.game.turn, 2);
    assert_eq!(refused.state.game.history.len(), 2);
    assert!(refused.effects.is_empty());
}

#[test]
fn delete_history_from_is_noop_without_matching_checkpoint() {
    let pack = sample_pack();
    let (mut state, _undo_id, _gid) = two_accepted_turns(&pack);
    let second_id = state.game.history.last().unwrap().id;
    // 링을 비우면 접미 절단에 필요한 turn-1 체크포인트가 없다.
    state.checkpoints.clear();

    let refused = reduce(
        &state,
        EngineCommand::DeleteHistoryFrom {
            entry_id: second_id,
        },
        &pack,
    )
    .expect("no checkpoint");
    assert_eq!(refused.state.game.turn, 2);
    assert!(refused.effects.is_empty());
}

#[test]
fn build_view_extracts_relationship_deltas_from_latest_history_entry() {
    let pack = sample_pack();

    // 1. Fresh game (Turn 0): no history, deltas must be None
    let initial = EngineState::initial(&pack);
    let view_initial = build_view(&initial, &pack, None, None);
    assert_eq!(view_initial.status.affection_delta, None);
    assert_eq!(view_initial.status.trust_delta, None);
    assert_eq!(view_initial.status.tension_delta, None);

    // 2. Two accepted turns: first had affection delta +1, second had affection delta -1
    let (state, undo_id, _gid) = two_accepted_turns(&pack);
    let view_turn2 = build_view(&state, &pack, None, None);
    assert_eq!(view_turn2.status.turn, 2);
    assert_eq!(view_turn2.status.affection_delta, Some(-1));

    // 3. Rewind to turn 1: delta must now reflect turn 1 (+1)
    let rewound = reduce(
        &state,
        EngineCommand::RestoreCheckpoint {
            checkpoint_id: undo_id,
        },
        &pack,
    )
    .expect("restore checkpoint");
    let view_rewound = build_view(&rewound.state, &pack, None, None);
    assert_eq!(view_rewound.status.turn, 1);
    assert_eq!(view_rewound.status.affection_delta, Some(1));
}
