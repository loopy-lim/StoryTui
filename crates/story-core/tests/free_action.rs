//! Integration tests for W7 (ST-024): free-action turns, regenerate, and
//! generation lineage through [`story_core::engine::reduce`].
//!
//! Pins the player-sovereignty invariants:
//!
//! 1. [`EngineCommand::SubmitFreeAction`] drives a full turn; the accepted
//!    `HistoryEntry.player_action` carries the parsed action, and the request
//!    prompt includes the verbatim `original_text` + the Korean sovereignty
//!    clause.
//! 2. [`EngineCommand::RegenerateCurrentTurn`] re-requests from the SAME
//!    checkpoint with a fresh `generation_id`; `lineage` records every attempt
//!    and the committed history is untouched.
//! 3. **A failed regenerate does NOT overwrite the committed turn** — the
//!    pre-choice checkpoint is restored byte-identically.
//! 4. History grows only on accepted (committed) generations.
//! 5. [`EngineCommand::RestoreCheckpoint`] rewinds to a prior committed state.

use std::collections::BTreeMap;
use std::path::PathBuf;

use story_core::engine::{EngineState, Transition, reduce};
use story_core::engine_cmd::{EngineCommand, EngineEffect};
use story_model::{
    ActionType, AppViewModel, CharacterDef, CharacterId, Choice, GeneratedTurnDraft, LocationDef,
    LocationId, PackContentPolicy, PackInitialState, PackMetadata, PackRating, ParsedAction,
    PromptFragments, RelationshipStage, RiskLevel, StatePatch, StoryPack, StoryRules, TimeSlot,
    TurnSummary,
};

const NAMU: &str = "yuna";

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
        name: "한유나".to_string(),
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
        summary: empty_summary(),
        usage: None,
    }
}

fn malformed_draft(generation_id: uuid::Uuid) -> GeneratedTurnDraft {
    let mut patch = StatePatch::default();
    patch.set_flags.insert("nonexistent_flag".to_string(), true);
    GeneratedTurnDraft {
        generation_id,
        narration: "버려진 장면.".to_string(),
        choices: vec![choice("choice-0", "계속", RiskLevel::Safe)],
        proposed_patch: patch,
        summary: empty_summary(),
        usage: None,
    }
}

fn empty_summary() -> TurnSummary {
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

fn dialogue_action(original: &str) -> ParsedAction {
    ParsedAction {
        action_type: ActionType::Dialogue,
        target: Some(cid()),
        intent: None,
        tone: None,
        content: original.to_string(),
        original_text: original.to_string(),
        confidence: Some(0.8),
    }
}

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

fn request_generation(t: &Transition) -> &story_model::TurnRequest {
    t.effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::RequestGeneration { request, .. } => Some(request),
            _ => None,
        })
        .expect("a RequestGeneration was emitted")
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

// =====================================================================
// 1. SubmitFreeAction drives a turn + prompt carries verbatim text
// =====================================================================

#[test]
fn submit_free_action_drives_a_turn_and_records_player_action() {
    let pack = sample_pack();
    let (mut state, gid) = start_and_pending_id(&pack);
    // Accept the opening to reach turn 1 (choices available).
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

    // Submit a free-text action.
    let action = dialogue_action("한유나에게 오래간만이라고 말한다");
    let t = reduce(
        &state,
        EngineCommand::SubmitFreeAction {
            action: Box::new(action.clone()),
        },
        &pack,
    )
    .expect("submit free action");
    assert!(t.state.pending.is_some());
    assert_eq!(
        t.state.pending.as_ref().unwrap().player_action.as_ref(),
        Some(&action),
        "pending carries the parsed action verbatim"
    );
    assert_eq!(
        t.state.lineage.len(),
        1,
        "lineage seeded with the request's generation id"
    );

    let pending_gid = t.state.pending.as_ref().unwrap().generation_id;
    let checkpoint_bytes =
        serde_json::to_vec(&t.state.pending.as_ref().unwrap().checkpoint).unwrap();
    state = t.state;

    // Accept the free-action generation → turn 2, history grows by one.
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(pending_gid, 1)),
        },
        &pack,
    )
    .expect("accept free action");
    assert_eq!(t.state.game.turn, 2);
    assert_eq!(t.state.game.history.len(), 2);
    let last = t.state.game.history.last().unwrap();
    assert_eq!(
        last.player_action,
        Some(action),
        "accepted turn records the parsed free action"
    );
    // The committed state was pushed onto the checkpoint ring.
    assert_eq!(
        t.state.checkpoints.len(),
        2,
        "one checkpoint per committed turn"
    );
    assert!(t.state.pending.is_none());
    assert!(t.state.lineage.is_empty(), "lineage clears on commit");
    // last_choice not set for a free action (no prepared choice).
    assert!(t.state.game.last_choice.is_none());
    let _ = checkpoint_bytes; // checkpoint captured pre-choice; not asserted further here.
}

#[test]
fn submit_free_action_prompt_includes_sovereignty_clause_and_original_text() {
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

    let original = "한유나에게 오래간만이라고 말한다";
    let action = dialogue_action(original);
    let t = reduce(
        &state,
        EngineCommand::SubmitFreeAction {
            action: Box::new(action),
        },
        &pack,
    )
    .expect("submit");
    let request = request_generation(&t);

    let system = request
        .messages
        .iter()
        .find(|m| m.role == "system")
        .map(|m| m.content.as_str())
        .unwrap();
    let user = request
        .messages
        .iter()
        .find(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .unwrap();

    // Sovereignty clause (Korean) reinforced in the system prompt.
    assert!(
        system.contains("이 입력은 플레이어의 의도다"),
        "system prompt must carry the Korean sovereignty clause"
    );
    // The player's verbatim text is forwarded into the user prompt.
    assert!(
        user.contains(&format!("original_text: {original}")),
        "user prompt must include the verbatim original_text"
    );
    assert!(
        user.contains("한유나"),
        "user prompt must include the parsed content"
    );
}

#[test]
fn submit_free_action_rejected_while_generation_in_flight() {
    let pack = sample_pack();
    let (state, _gid) = start_and_pending_id(&pack);
    // Opening generation is pending; a free action must be rejected.
    let err = reduce(
        &state,
        EngineCommand::SubmitFreeAction {
            action: Box::new(dialogue_action("한유나에게 말한다")),
        },
        &pack,
    )
    .expect_err("free action during pending generation must Err");
    assert_eq!(err.reason_code(), Some("engine_generation_in_flight"));
}

// =====================================================================
// 2. RegenerateCurrentTurn
// =====================================================================

#[test]
fn regenerate_re_requests_from_same_checkpoint_with_new_gen_id() {
    let pack = sample_pack();
    let (mut state, gid) = start_and_pending_id(&pack);
    // Reach a committed turn 1.
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid, 1)),
        },
        &pack,
    )
    .expect("accept opening");
    state = t.state;
    let committed_history = state.game.history.len();
    let committed_bytes = serde_json::to_vec(&state.game).unwrap();

    // SubmitChoice → pending (gid1, checkpoint = turn-1 game).
    let t = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let gid1 = t.state.pending.as_ref().unwrap().generation_id;
    let checkpoint_bytes =
        serde_json::to_vec(&t.state.pending.as_ref().unwrap().checkpoint).unwrap();
    assert_eq!(t.state.lineage, vec![gid1]);
    state = t.state;

    // Regenerate → new pending (gid2 != gid1), SAME checkpoint, lineage grew.
    let t = reduce(&state, EngineCommand::RegenerateCurrentTurn, &pack).expect("regenerate");
    let pending = t.state.pending.as_ref().unwrap();
    let gid2 = pending.generation_id;
    assert_ne!(gid2, gid1, "regenerate mints a fresh generation id");
    assert_eq!(
        serde_json::to_vec(&pending.checkpoint).unwrap(),
        checkpoint_bytes,
        "checkpoint is byte-identical to the original request"
    );
    assert_eq!(
        t.state.lineage,
        vec![gid1, gid2],
        "lineage records every attempt for the current turn"
    );

    // Committed game/history untouched by the regenerate.
    assert_eq!(t.state.game.history.len(), committed_history);
    assert_eq!(
        serde_json::to_vec(&t.state.game).unwrap(),
        committed_bytes,
        "regenerate never mutates the committed stable state"
    );

    // A RequestGeneration is emitted for the new id.
    let req = request_generation(&t);
    assert_eq!(req.generation_id, gid2);
}

#[test]
fn regenerate_with_no_pending_is_identity() {
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
    let t = reduce(&state, EngineCommand::RegenerateCurrentTurn, &pack).expect("regen no-op");
    assert!(t.state.pending.is_none());
    assert!(t.effects.is_empty(), "no effects when nothing is pending");
    assert_eq!(serde_json::to_vec(&t.state.game).unwrap(), before);
}

// =====================================================================
// 3. THE KEY TEST: failed regenerate does NOT overwrite committed turn
// =====================================================================

#[test]
fn failed_regenerate_does_not_overwrite_committed_turn() {
    let pack = sample_pack();
    let (mut state, gid) = start_and_pending_id(&pack);
    // Commit the opening → turn 1, one history entry.
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid, 1)),
        },
        &pack,
    )
    .expect("accept opening");
    state = t.state;
    assert_eq!(state.game.history.len(), 1);
    let committed_bytes = serde_json::to_vec(&state.game).unwrap();

    // Player picks a choice, then re-rolls before the draft returns.
    let t = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    state = t.state;
    let t = reduce(&state, EngineCommand::RegenerateCurrentTurn, &pack).expect("regen");
    let pending_gid = t.state.pending.as_ref().unwrap().generation_id;
    state = t.state;

    // The provider returns a malformed draft → apply_patch fails. The
    // committed turn 1 must survive byte-identically.
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(malformed_draft(pending_gid)),
        },
        &pack,
    )
    .expect("failed accept is Ok with a recoverable error effect");

    assert_eq!(
        serde_json::to_vec(&t.state.game).unwrap(),
        committed_bytes,
        "failed regenerate must restore the pre-choice checkpoint, NOT overwrite turn 1"
    );
    assert_eq!(
        t.state.game.history.len(),
        1,
        "history still has only turn 1"
    );
    assert!(t.state.pending.is_none(), "pending cleared on failure");
    assert!(t.state.lineage.is_empty(), "lineage cleared on failure");
    assert!(
        t.effects
            .iter()
            .any(|e| matches!(e, EngineEffect::ShowRecoverableError(_))),
        "expected a recoverable error effect"
    );
}

// =====================================================================
// 4. History contains ONLY committed entries
// =====================================================================

#[test]
fn history_contains_only_committed_entries() {
    let pack = sample_pack();
    let (mut state, gid) = start_and_pending_id(&pack);
    // Accept opening → turn 1.
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid, 1)),
        },
        &pack,
    )
    .expect("accept opening");
    state = t.state;

    // choice → regenerate → regenerate (two re-rolls) → accept → turn 2.
    let t = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    state = t.state;
    let t = reduce(&state, EngineCommand::RegenerateCurrentTurn, &pack).expect("regen1");
    state = t.state;
    let t = reduce(&state, EngineCommand::RegenerateCurrentTurn, &pack).expect("regen2");
    assert_eq!(t.state.lineage.len(), 3, "two regenerates → three attempts");
    let pending_gid = t.state.pending.as_ref().unwrap().generation_id;
    state = t.state;

    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(pending_gid, 1)),
        },
        &pack,
    )
    .expect("accept turn 2");
    state = t.state;

    // Two committed turns → history length is exactly 2. Regenerates added 0.
    assert_eq!(state.game.history.len(), 2);
    assert_eq!(state.game.turn, 2);
    assert!(state.lineage.is_empty());
}

// =====================================================================
// 5. RestoreCheckpoint rewinds to a prior committed state
// =====================================================================

#[test]
fn restore_checkpoint_rewinds_to_prior_committed_state() {
    let pack = sample_pack();
    let (mut state, gid) = start_and_pending_id(&pack);
    // Commit opening → turn 1; checkpoint ring now has 1 entry (turn-1 state).
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid, 1)),
        },
        &pack,
    )
    .expect("accept opening");
    state = t.state;
    assert_eq!(state.checkpoints.len(), 1);
    let turn1_checkpoint_id = state.checkpoints[0].id;
    let turn1_checkpoint_game = state.checkpoints[0].game.clone();

    // Commit turn 2; ring now has 2 entries.
    let t = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let gid2 = t.state.pending.as_ref().unwrap().generation_id;
    state = t.state;
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid2, 1)),
        },
        &pack,
    )
    .expect("accept turn 2");
    state = t.state;
    assert_eq!(state.game.turn, 2);
    assert_eq!(state.checkpoints.len(), 2);

    // Start a generation for turn 3, then rewind to the turn-1 checkpoint.
    let t = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    state = t.state;
    assert!(state.pending.is_some());

    let t = reduce(
        &state,
        EngineCommand::RestoreCheckpoint {
            checkpoint_id: turn1_checkpoint_id,
        },
        &pack,
    )
    .expect("restore");
    assert!(t.state.pending.is_none(), "restore clears pending");
    assert!(t.state.lineage.is_empty(), "restore clears lineage");
    assert_eq!(
        serde_json::to_vec(&t.state.game).unwrap(),
        serde_json::to_vec(&turn1_checkpoint_game).unwrap(),
        "game rewound to the turn-1 checkpoint"
    );
    assert_eq!(t.state.game.turn, 1);
    assert_eq!(t.state.game.history.len(), 1);
    let _ = published_view(&t);
}

#[test]
fn restore_checkpoint_unknown_id_is_noop() {
    let pack = sample_pack();
    let (state, _gid) = start_and_pending_id(&pack);
    let before = serde_json::to_vec(&state.game).unwrap();
    let t = reduce(
        &state,
        EngineCommand::RestoreCheckpoint {
            checkpoint_id: uuid::Uuid::nil(),
        },
        &pack,
    )
    .expect("restore unknown");
    assert_eq!(serde_json::to_vec(&t.state.game).unwrap(), before);
    assert!(t.effects.is_empty(), "unknown checkpoint → no effects");
}
