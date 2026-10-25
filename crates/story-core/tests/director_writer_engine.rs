//! Integration tests for the optional Director/Writer 2-stage turn path
//! (W10.4, ST-033/034). Pins the load-bearing ADR-005 invariants:
//!
//! - `EngineState::use_director_writer` defaults to `false` (MVP single-call
//!   path unchanged).
//! - When on, `SubmitChoice` emits `RequestDirector` (NOT `RequestGeneration`).
//! - `AcceptDirectorPlan` with a bad plan restores the checkpoint and DOES NOT
//!   emit `RequestWriter` — the Writer is never called.
//! - `AcceptDirectorPlan` with a valid plan emits `RequestWriter`.
//! - `AcceptWriterFinish` commits the validated Director plan's patch +
//!   choices with the Writer's narration; the Writer text is narration only.
//! - Embedded JSON in the Writer text is stripped (not applied).
//! - Late/mismatched Director/Writer commands are silently discarded.

use std::collections::BTreeMap;
use std::path::PathBuf;

use story_core::engine::{EngineState, NarrationRepair, Transition, reduce};
use story_core::engine_cmd::{EngineCommand, EngineEffect};
use story_model::{
    CharacterDef, CharacterId, Choice, DirectorPlan, GeneratedTurnDraft, LocationDef, LocationId,
    PackContentPolicy, PackInitialState, PackMetadata, PackRating, PromptFragments,
    RelationshipStage, RiskLevel, StatePatch, StoryPack, StoryRules, TimeSlot, TruncationStatus,
    TurnSummary, WriterFinish,
};

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

fn choice(id: &str, label: &str, risk: RiskLevel) -> Choice {
    Choice {
        id: id.to_string(),
        label: label.to_string(),
        intent: label.to_string(),
        tone: None,
        target: None,
        risk,
        hidden_requirements: vec![],
        preview: Some(format!("{label} 선택의 즉각적인 대가와 반응을 확인한다")),
    }
}

/// A valid opening-turn draft (for priming the engine via the MVP path).
fn valid_draft(generation_id: uuid::Uuid) -> GeneratedTurnDraft {
    let mut patch = StatePatch::default();
    patch.affection_delta.insert(cid(), 1);
    GeneratedTurnDraft {
        generation_id,
        narration: "비가 그쳤다.".to_string(),
        choices: vec![
            choice("choice-0", "떠난 이유를 솔직하게 묻는다", RiskLevel::High),
            choice("choice-1", "탁자의 편지를 꺼내 읽어본다", RiskLevel::Low),
            choice("choice-2", "대화를 위해 복도로 이동한다", RiskLevel::Medium),
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

/// A valid Director plan (affection_delta within `-3..=3`, references `c-namu`).
fn valid_plan() -> DirectorPlan {
    let mut patch = StatePatch::default();
    patch.affection_delta.insert(cid(), 2);
    DirectorPlan {
        scene_goal: "scene".to_string(),
        required_beats: vec!["beat".to_string()],
        emotion_arcs: BTreeMap::new(),
        character_outfits: Default::default(),
        ambient_situation: None,
        reveal_fact_ids: vec!["f-rain".to_string()],
        must_not_reveal_fact_ids: vec![],
        proposed_patch: patch,
        choices: vec![
            choice("next-0", "전시회 약속을 먼저 사과한다", RiskLevel::High),
            choice("next-1", "구체적인 단서를 확인한다", RiskLevel::Low),
            choice("next-2", "다른 장소로 이동한다", RiskLevel::Medium),
        ],
        conflict_checks: vec![],
        end_condition: "scene continues".to_string(),
    }
}

/// A Director plan whose patch violates a STILL-hard stage-3 rule: a +5
/// affection delta exceeds the pack's ±3 single-turn delta limit. The engine
/// MUST reject it and NOT emit RequestWriter. (Unknown references no longer
/// reject — they are dropped with an audit entry; see
/// `unknown_patch_reference_is_dropped_and_writer_still_called`.)
fn bad_plan_out_of_range_delta() -> DirectorPlan {
    let mut patch = StatePatch::default();
    patch.affection_delta.insert(cid(), 5);
    let mut plan = valid_plan();
    plan.proposed_patch = patch;
    plan
}

/// Prime the engine with StartNewGame + AcceptGeneration so the player has a
/// real choice to submit. Returns the post-opening state (turn 1, one choice
/// offered). The returned state has `use_director_writer` flipped to `true`
/// to exercise the 2-stage path.
fn prime_then_enable_director() -> (EngineState, StoryPack) {
    let pack = sample_pack();
    let init = EngineState::initial(&pack);
    let t = reduce(&init, EngineCommand::StartNewGame, &pack).expect("start");
    let gid = t
        .effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::RequestGeneration { generation_id, .. } => Some(*generation_id),
            _ => None,
        })
        .expect("StartNewGame emits RequestGeneration");
    let t = reduce(
        &t.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid)),
        },
        &pack,
    )
    .expect("accept opening");
    let mut state = t.state;
    state.use_director_writer = true;
    (state, pack)
}

// ---------------------------------------------------------------------------

#[test]
fn use_director_writer_defaults_to_false() {
    let pack = sample_pack();
    let init = EngineState::initial(&pack);
    assert!(
        !init.use_director_writer,
        "ADR-005: 2-stage path MUST default off"
    );
}

#[test]
fn start_new_game_on_director_path_emits_request_director() {
    let pack = sample_pack();
    let mut init = EngineState::initial(&pack);
    init.use_director_writer = true;
    let t = reduce(&init, EngineCommand::StartNewGame, &pack).expect("start");
    let director_request = t
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestDirector { request, .. } => Some(request),
            _ => None,
        })
        .expect("director request");
    assert!(
        director_request.messages[0]
            .content
            .contains("# Director stage override")
    );
    assert!(director_request.messages[0].content.contains("\nd\n"));
    assert!(
        t.effects
            .iter()
            .any(|e| matches!(e, EngineEffect::RequestDirector { .. })),
        "StartNewGame on the 2-stage path MUST emit RequestDirector so the \
         configured Writer model narrates the opening scene too"
    );
    assert!(
        !t.effects
            .iter()
            .any(|e| matches!(e, EngineEffect::RequestGeneration { .. })),
        "StartNewGame on the 2-stage path MUST NOT emit RequestGeneration"
    );
}

#[test]
fn fallback_to_single_pass_reissues_turn_as_request_generation() {
    let pack = sample_pack();
    let mut init = EngineState::initial(&pack);
    init.use_director_writer = true;
    let t = reduce(&init, EngineCommand::StartNewGame, &pack).expect("start");
    let gid = t
        .effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::RequestDirector { generation_id, .. } => Some(*generation_id),
            _ => None,
        })
        .expect("StartNewGame on the director path emits RequestDirector");
    let state = t.state;

    // The Director model failed to produce a plan → host dispatches fallback.
    let t = reduce(
        &state,
        EngineCommand::FallbackToSinglePass { generation_id: gid },
        &pack,
    )
    .expect("fallback");
    assert!(
        t.effects.iter().any(|e| matches!(
            e,
            EngineEffect::RequestGeneration { generation_id, .. } if *generation_id == gid
        )),
        "FallbackToSinglePass MUST re-issue the turn as RequestGeneration with the same id"
    );
    assert!(
        t.state.pending.is_some(),
        "the pending generation is retained for the fallback result"
    );
}

#[test]
fn submit_choice_on_mvp_path_emits_request_generation_not_director() {
    let pack = sample_pack();
    let init = EngineState::initial(&pack);
    let t = reduce(&init, EngineCommand::StartNewGame, &pack).expect("start");
    let gid = t
        .effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::RequestGeneration { generation_id, .. } => Some(*generation_id),
            _ => None,
        })
        .expect("RequestGeneration");
    let t = reduce(
        &t.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(valid_draft(gid)),
        },
        &pack,
    )
    .expect("accept");

    // Now SubmitChoice on the MVP path — must produce RequestGeneration.
    let t = reduce(
        &t.state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    assert!(
        t.effects
            .iter()
            .any(|e| matches!(e, EngineEffect::RequestGeneration { .. })),
        "MVP SubmitChoice MUST emit RequestGeneration"
    );
    assert!(
        !t.effects
            .iter()
            .any(|e| matches!(e, EngineEffect::RequestDirector { .. })),
        "MVP SubmitChoice MUST NOT emit RequestDirector"
    );
}

#[test]
fn submit_choice_on_two_stage_path_emits_request_director() {
    let (state, pack) = prime_then_enable_director();
    let t = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");

    let director = t
        .effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::RequestDirector { generation_id, .. } => Some(*generation_id),
            _ => None,
        })
        .expect("2-stage SubmitChoice MUST emit RequestDirector");
    assert!(
        t.state.pending.is_some(),
        "pending set so the host can pair the Director result back"
    );
    assert_eq!(t.state.pending.as_ref().unwrap().generation_id, director);
    assert!(
        t.state.pending.as_ref().unwrap().validated_plan.is_none(),
        "no validated plan before AcceptDirectorPlan"
    );
}

#[test]
fn accept_director_plan_valid_emits_request_writer_and_stores_plan() {
    let (state, pack) = prime_then_enable_director();
    let t = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let gid = t
        .effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::RequestDirector { generation_id, .. } => Some(*generation_id),
            _ => None,
        })
        .expect("RequestDirector");

    let t = reduce(
        &t.state,
        EngineCommand::AcceptDirectorPlan {
            generation_id: gid,
            plan: Box::new(valid_plan()),
        },
        &pack,
    )
    .expect("accept director");

    // RequestWriter emitted; the validated plan is stored on the pending.
    let (writer, writer_request) = t
        .effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::RequestWriter {
                generation_id,
                request,
            } => Some((*generation_id, request)),
            _ => None,
        })
        .expect("valid plan MUST emit RequestWriter");
    assert_eq!(writer, gid);
    assert!(
        writer_request.messages[0]
            .content
            .contains("# Writer stage override")
    );
    assert!(writer_request.messages[0].content.contains("\nw\n"));
    assert!(
        writer_request.messages[1]
            .content
            .contains("# Validated Director plan")
    );
    assert!(
        writer_request.messages[1]
            .content
            .contains("\"scene_goal\":\"scene\"")
    );
    assert!(
        writer_request.messages[1]
            .content
            .contains(&writer_request.plan_digest)
    );
    assert!(
        t.state.pending.as_ref().unwrap().validated_plan.is_some(),
        "validated plan stored after AcceptDirectorPlan"
    );
}

#[test]
fn accept_director_plan_invalid_restores_checkpoint_and_skips_writer() {
    let (state, pack) = prime_then_enable_director();
    let before = state.clone();
    let t = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let gid = t
        .effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::RequestDirector { generation_id, .. } => Some(*generation_id),
            _ => None,
        })
        .expect("RequestDirector");

    let t = reduce(
        &t.state,
        EngineCommand::AcceptDirectorPlan {
            generation_id: gid,
            plan: Box::new(bad_plan_out_of_range_delta()),
        },
        &pack,
    )
    .expect("accept director (bad)");

    // No RequestWriter — the Writer is NEVER called on a rejected plan.
    assert!(
        !t.effects
            .iter()
            .any(|e| matches!(e, EngineEffect::RequestWriter { .. })),
        "Writer MUST NOT be requested for a rejected Director plan"
    );
    // Pending cleared and game restored to the pre-SubmitChoice checkpoint.
    assert!(t.state.pending.is_none(), "pending cleared on rejection");
    assert_eq!(
        t.state.game.turn, before.game.turn,
        "checkpoint restored on Director rejection"
    );
    assert_eq!(
        t.state.game.revision, before.game.revision,
        "revision unchanged on Director rejection"
    );
    assert_eq!(
        t.state.game.history.len(),
        before.game.history.len(),
        "history unchanged on Director rejection"
    );
    assert_eq!(
        t.state.game.characters.get(&cid()).unwrap().affection,
        before.game.characters.get(&cid()).unwrap().affection,
        "affection unchanged on Director rejection"
    );
    // A recoverable error surfaced.
    assert!(
        t.effects
            .iter()
            .any(|e| matches!(e, EngineEffect::ShowRecoverableError(_))),
        "Director rejection MUST surface a recoverable error"
    );
}

#[test]
fn unknown_patch_reference_is_dropped_and_writer_still_called() {
    let (state, pack) = prime_then_enable_director();
    let submitted = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let gid = submitted
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestDirector { generation_id, .. } => Some(*generation_id),
            _ => None,
        })
        .expect("RequestDirector");

    // The patch references an unknown character: the entry is dropped with
    // an audit record and the two-stage pipeline continues.
    let mut patch = StatePatch::default();
    patch
        .affection_delta
        .insert(CharacterId("c-ghost".to_string()), 1);
    let mut plan = valid_plan();
    plan.proposed_patch = patch;

    let t = reduce(
        &submitted.state,
        EngineCommand::AcceptDirectorPlan {
            generation_id: gid,
            plan: Box::new(plan),
        },
        &pack,
    )
    .expect("accept director (dropped reference)");

    assert!(
        t.effects
            .iter()
            .any(|e| matches!(e, EngineEffect::RequestWriter { .. })),
        "Writer IS requested when the only defect is a dropped reference"
    );
    assert!(
        t.state.pending.is_some(),
        "turn stays pending for the writer stage"
    );
}

#[test]
fn retry_after_director_rejection_restarts_at_the_director() {
    let (state, pack) = prime_then_enable_director();
    let submitted = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let gid = submitted
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestDirector { generation_id, .. } => Some(*generation_id),
            _ => None,
        })
        .expect("RequestDirector");
    let rejected = reduce(
        &submitted.state,
        EngineCommand::AcceptDirectorPlan {
            generation_id: gid,
            plan: Box::new(bad_plan_out_of_range_delta()),
        },
        &pack,
    )
    .expect("reject invalid plan");

    let retried =
        reduce(&rejected.state, EngineCommand::RegenerateCurrentTurn, &pack).expect("retry");
    assert!(retried.effects.iter().any(|effect| matches!(
        effect,
        EngineEffect::RequestDirector { generation_id, .. } if *generation_id != gid
    )));
    assert!(
        !retried
            .effects
            .iter()
            .any(|effect| matches!(effect, EngineEffect::RequestGeneration { .. })),
        "a Director rejection must not silently switch to the single-pass path"
    );
}

#[test]
fn accept_writer_finish_commits_plan_patch_and_writer_narration() {
    let (state, pack) = prime_then_enable_director();
    let t = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let gid = t
        .effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::RequestDirector { generation_id, .. } => Some(*generation_id),
            _ => None,
        })
        .expect("RequestDirector");

    let plan = valid_plan();
    let expected_affection = state
        .game
        .characters
        .get(&cid())
        .unwrap()
        .affection
        .saturating_add(
            plan.proposed_patch
                .affection_delta
                .get(&cid())
                .copied()
                .unwrap_or(0),
        );

    let t = reduce(
        &t.state,
        EngineCommand::AcceptDirectorPlan {
            generation_id: gid,
            plan: Box::new(plan.clone()),
        },
        &pack,
    )
    .expect("accept director");

    let t = reduce(
        &t.state,
        EngineCommand::AcceptWriterFinish {
            finish: Box::new(WriterFinish {
                generation_id: gid,
                full_text: "나무가 조용히 고개를 끄덕인다.".to_string(),
                usage: None,
                finish_reason: None,
            }),
        },
        &pack,
    )
    .expect("accept writer");

    assert!(t.state.pending.is_none(), "pending cleared");
    assert_eq!(
        t.state.game.turn,
        state.game.turn + 1,
        "turn bumped exactly once"
    );
    // The Director plan's patch applied (affection +2).
    assert_eq!(
        t.state.game.characters.get(&cid()).unwrap().affection,
        expected_affection,
        "Director plan patch applied"
    );
    // History narration is the WRITER's text; choices are the DIRECTOR's.
    let entry = t.state.game.history.last().expect("history entry");
    assert_eq!(entry.narration, "나무가 조용히 고개를 끄덕인다.");
    assert_eq!(entry.choices.len(), 3);
    assert_eq!(entry.choices[0].id, "next-0");
    assert_eq!(entry.applied_patch, plan.proposed_patch);
}

#[test]
fn accept_writer_finish_strips_embedded_patch_in_narration() {
    let (state, pack) = prime_then_enable_director();
    let t = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let gid = t
        .effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::RequestDirector { generation_id, .. } => Some(*generation_id),
            _ => None,
        })
        .expect("RequestDirector");
    let t = reduce(
        &t.state,
        EngineCommand::AcceptDirectorPlan {
            generation_id: gid,
            plan: Box::new(valid_plan()),
        },
        &pack,
    )
    .expect("accept director");

    // The Writer smuggles a patch through prose. The finalize pass MUST strip
    // it; the committed narration must NOT contain the smuggled content; the
    // Director's patch (affection +2) is the only patch applied.
    let smuggled = "나무가 웃는다.\n```json\n{\"state_patch\":{\"affection_delta\":{\"c-namu\":99}}}\n```\n끝.".to_string();
    let t = reduce(
        &t.state,
        EngineCommand::AcceptWriterFinish {
            finish: Box::new(WriterFinish {
                generation_id: gid,
                full_text: smuggled,
                usage: None,
                finish_reason: None,
            }),
        },
        &pack,
    )
    .expect("accept writer");

    let entry = t.state.game.history.last().expect("history");
    assert!(
        !entry.narration.contains("affection_delta"),
        "smuggled patch MUST be stripped from narration: {}",
        entry.narration
    );
    assert!(
        !entry.narration.contains("```"),
        "fence markers MUST be stripped: {}",
        entry.narration
    );
    // The applied affection is +2 (Director plan), NOT +99 (smuggled).
    let expected = state
        .game
        .characters
        .get(&cid())
        .unwrap()
        .affection
        .saturating_add(2);
    assert_eq!(
        t.state.game.characters.get(&cid()).unwrap().affection,
        expected,
        "Director's +2 applied, Writer's smuggled +99 ignored"
    );
}

#[test]
fn writer_repetition_gets_one_narration_only_rewrite_before_commit() {
    let (state, pack) = prime_then_enable_director();
    let submitted = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let director_id = submitted.state.pending.as_ref().unwrap().generation_id;
    let planned = reduce(
        &submitted.state,
        EngineCommand::AcceptDirectorPlan {
            generation_id: director_id,
            plan: Box::new(valid_plan()),
        },
        &pack,
    )
    .expect("plan");

    // The primed opening narration is exactly "비가 그쳤다.".
    let repairing = reduce(
        &planned.state,
        EngineCommand::AcceptWriterFinish {
            finish: Box::new(WriterFinish {
                generation_id: director_id,
                full_text: "비가 그쳤다.".to_string(),
                usage: None,
                finish_reason: None,
            }),
        },
        &pack,
    )
    .expect("writer repair");
    assert_eq!(repairing.state.game.turn, state.game.turn);
    assert!(matches!(
        repairing
            .state
            .pending
            .as_ref()
            .and_then(|pending| pending.narration_repair.as_ref()),
        Some(NarrationRepair::Writer { .. })
    ));
    let (repair_id, request) = repairing
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestWriter {
                generation_id,
                request,
            } => Some((*generation_id, request)),
            _ => None,
        })
        .expect("one writer repair request");
    assert_ne!(repair_id, director_id);
    assert!(
        request
            .messages
            .last()
            .unwrap()
            .content
            .contains("Writer narration quality repair")
    );

    let completed = reduce(
        &repairing.state,
        EngineCommand::AcceptWriterFinish {
            finish: Box::new(WriterFinish {
                generation_id: repair_id,
                full_text:
                    "나무는 접어 둔 초대장을 테이블 위에 펴고, 오늘 전시회에 동행할지 묻는다."
                        .to_string(),
                usage: None,
                finish_reason: None,
            }),
        },
        &pack,
    )
    .expect("accept writer repair");
    assert_eq!(completed.state.game.turn, state.game.turn + 1);
    assert_eq!(
        completed.state.game.history.last().unwrap().narration,
        "나무는 접어 둔 초대장을 테이블 위에 펴고, 오늘 전시회에 동행할지 묻는다."
    );
}

// ---------------------------------------------------------------------------
// T-4: empty Writer narration + finish_reason truncation state (AC-5/6/8)
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

fn published_view(t: &Transition) -> story_model::AppViewModel {
    t.effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::PublishView(view) => Some((**view).clone()),
            _ => None,
        })
        .expect("a PublishView was emitted")
}

/// A Director plan for the turn AFTER `valid_plan` was committed: none of its
/// choices repeat the action the player just picked (next-0), which would
/// trip the deterministic `RepeatsSelected` gate.
fn followup_plan() -> DirectorPlan {
    let mut plan = valid_plan();
    plan.choices = vec![
        choice("third-0", "고백의 뒷이야기를 묻는다", RiskLevel::High),
        choice("third-1", "멈춘 손목시계의 시각을 확인한다", RiskLevel::Low),
        choice("third-2", "함께 플랫폼으로 이동한다", RiskLevel::Medium),
    ];
    plan
}

/// Drive prime → SubmitChoice → AcceptDirectorPlan so a Writer request is
/// pending. Returns the waiting state, the pack, and the writer generation id.
fn writer_pending() -> (EngineState, StoryPack, uuid::Uuid) {
    let (state, pack) = prime_then_enable_director();
    let submitted = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let director_id = submitted.state.pending.as_ref().unwrap().generation_id;
    let planned = reduce(
        &submitted.state,
        EngineCommand::AcceptDirectorPlan {
            generation_id: director_id,
            plan: Box::new(valid_plan()),
        },
        &pack,
    )
    .expect("plan");
    let writer_id = planned
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestWriter { generation_id, .. } => Some(*generation_id),
            _ => None,
        })
        .expect("RequestWriter");
    (planned.state, pack, writer_id)
}

#[test]
fn writer_empty_narration_gets_one_repair_then_the_quality_overlay() {
    let (state, pack, writer_id) = writer_pending();

    let repairing = reduce(
        &state,
        EngineCommand::AcceptWriterFinish {
            finish: Box::new(WriterFinish {
                generation_id: writer_id,
                full_text: "   \n".to_string(),
                usage: None,
                finish_reason: Some("stop".to_string()),
            }),
        },
        &pack,
    )
    .expect("empty writer narration starts the bounded repair");
    assert_eq!(
        repairing.state.game.turn, state.game.turn,
        "nothing is committed"
    );
    assert!(matches!(
        repairing
            .state
            .pending
            .as_ref()
            .and_then(|pending| pending.narration_repair.as_ref()),
        Some(NarrationRepair::Writer { .. })
    ));
    let (repair_id, repair_request) = repairing
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestWriter {
                generation_id,
                request,
            } => Some((*generation_id, request)),
            _ => None,
        })
        .expect("one writer repair request");
    assert_ne!(repair_id, writer_id);
    assert!(
        repair_request
            .messages
            .last()
            .unwrap()
            .content
            .contains("본문이 비어 있다"),
        "the repair prompt must carry the Empty instruction"
    );

    // The repair also comes back empty → bounded failure: checkpoint
    // restored, retry offered, no second automatic repair.
    let failed = reduce(
        &repairing.state,
        EngineCommand::AcceptWriterFinish {
            finish: Box::new(WriterFinish {
                generation_id: repair_id,
                full_text: String::new(),
                usage: None,
                finish_reason: None,
            }),
        },
        &pack,
    )
    .expect("bounded repair failure");
    assert_eq!(failed.state.game.turn, state.game.turn);
    assert!(failed.state.pending.is_none());
    assert!(failed.state.retry_pending.is_some());
    assert!(failed.effects.iter().any(|effect| matches!(
        effect,
        EngineEffect::ShowRecoverableError(error)
            if error.code == "narration_quality_failed"
    )));
    assert!(
        !failed
            .effects
            .iter()
            .any(|effect| matches!(effect, EngineEffect::RequestWriter { .. })),
        "the repair is attempted exactly once"
    );
}

#[test]
fn writer_length_finish_marks_auto_and_the_next_turn_clears_it() {
    let (state, pack, writer_id) = writer_pending();
    let accepted = reduce(
        &state,
        EngineCommand::AcceptWriterFinish {
            finish: Box::new(WriterFinish {
                generation_id: writer_id,
                full_text: "나무가 창밖으로 시선을 옮기고, 접어 둔 초대장을 천천히 펼친다."
                    .to_string(),
                usage: None,
                finish_reason: Some("length".to_string()),
            }),
        },
        &pack,
    )
    .expect("accept truncated writer finish");
    assert_eq!(accepted.state.game.turn, state.game.turn + 1);
    assert_eq!(accepted.state.truncation, TruncationStatus::Auto);
    assert_eq!(published_view(&accepted).truncation, TruncationStatus::Auto);

    // The next regular turn retires the badge: the pending start clears it
    // and a "stop" finish keeps it cleared through the commit.
    let submitted = reduce(
        &accepted.state,
        EngineCommand::SubmitChoice {
            choice_id: "next-0".to_string(),
        },
        &pack,
    )
    .expect("submit next turn");
    assert_eq!(
        submitted.state.truncation,
        TruncationStatus::None,
        "a new regular turn pending clears the badge"
    );
    let director_id = submitted.state.pending.as_ref().unwrap().generation_id;
    let planned = reduce(
        &submitted.state,
        EngineCommand::AcceptDirectorPlan {
            generation_id: director_id,
            plan: Box::new(followup_plan()),
        },
        &pack,
    )
    .expect("plan next turn");
    let committed = reduce(
        &planned.state,
        EngineCommand::AcceptWriterFinish {
            finish: Box::new(WriterFinish {
                generation_id: director_id,
                full_text: "나무는 조용히 자리에서 일어난다.".to_string(),
                usage: None,
                finish_reason: Some("stop".to_string()),
            }),
        },
        &pack,
    )
    .expect("commit next turn");
    assert_eq!(committed.state.truncation, TruncationStatus::None);
    assert_eq!(
        published_view(&committed).truncation,
        TruncationStatus::None
    );
}

#[test]
fn writer_length_finish_after_a_spent_repair_is_manual_only() {
    let (state, pack, writer_id) = writer_pending();
    // The primed opening narration is exactly "비가 그쳤다." → the quality
    // gate fires and spends this turn's defensive re-call.
    let repairing = reduce(
        &state,
        EngineCommand::AcceptWriterFinish {
            finish: Box::new(WriterFinish {
                generation_id: writer_id,
                full_text: "비가 그쳤다.".to_string(),
                usage: None,
                finish_reason: Some("length".to_string()),
            }),
        },
        &pack,
    )
    .expect("repetition repair");
    assert!(matches!(
        repairing
            .state
            .pending
            .as_ref()
            .and_then(|pending| pending.narration_repair.as_ref()),
        Some(NarrationRepair::Writer { .. })
    ));
    let repair_id = repairing
        .effects
        .iter()
        .find_map(|effect| match effect {
            EngineEffect::RequestWriter { generation_id, .. } => Some(*generation_id),
            _ => None,
        })
        .expect("repair request");
    let committed = reduce(
        &repairing.state,
        EngineCommand::AcceptWriterFinish {
            finish: Box::new(WriterFinish {
                generation_id: repair_id,
                full_text: "나무는 접어 둔 초대장을 펼치다가 말을 잇지 못한다.".to_string(),
                usage: None,
                finish_reason: Some("length".to_string()),
            }),
        },
        &pack,
    )
    .expect("accept the repaired narration");
    assert_eq!(committed.state.game.turn, state.game.turn + 1);
    assert_eq!(
        committed.state.truncation,
        TruncationStatus::ManualOnly,
        "the repair already spent the per-turn defensive budget (AC-8)"
    );
    assert_eq!(
        published_view(&committed).truncation,
        TruncationStatus::ManualOnly
    );
}

#[test]
fn writer_stop_or_unreported_finish_leaves_no_truncation() {
    for finish_reason in [Some("stop".to_string()), None] {
        let (state, pack, writer_id) = writer_pending();
        let committed = reduce(
            &state,
            EngineCommand::AcceptWriterFinish {
                finish: Box::new(WriterFinish {
                    generation_id: writer_id,
                    full_text: "나무가 조용히 고개를 끄덕인다.".to_string(),
                    usage: None,
                    finish_reason,
                }),
            },
            &pack,
        )
        .expect("accept writer finish");
        assert_eq!(committed.state.game.turn, state.game.turn + 1);
        assert_eq!(committed.state.truncation, TruncationStatus::None);
        assert_eq!(
            published_view(&committed).truncation,
            TruncationStatus::None
        );
    }
}

#[test]
fn continue_accept_demotes_auto_truncation_to_manual_only() {
    let (state, pack, writer_id) = writer_pending();
    let accepted = reduce(
        &state,
        EngineCommand::AcceptWriterFinish {
            finish: Box::new(WriterFinish {
                generation_id: writer_id,
                full_text: "나무가 말을 고르다 절반에서 멈춘다.".to_string(),
                usage: None,
                finish_reason: Some("length".to_string()),
            }),
        },
        &pack,
    )
    .expect("accept truncated writer finish");
    assert_eq!(accepted.state.truncation, TruncationStatus::Auto);

    let requested =
        reduce(&accepted.state, EngineCommand::ContinueNarration, &pack).expect("continue");
    assert_eq!(
        requested.state.truncation,
        TruncationStatus::Auto,
        "the badge survives the continue cycle so the host can still act"
    );
    let continue_id = requested.state.pending.as_ref().unwrap().generation_id;
    let continued = reduce(
        &requested.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(side_draft(continue_id, "이어서 나무는 문을 나선다.")),
        },
        &pack,
    )
    .expect("accept continuation");
    assert_eq!(
        continued.state.truncation,
        TruncationStatus::ManualOnly,
        "the one auto-continue was spent; a second must not fire (AC-8)"
    );
    assert!(!continued.state.empty_side_result);
}

#[test]
fn mismatched_director_plan_generation_id_is_discarded() {
    let (state, pack) = prime_then_enable_director();
    let t = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");

    let other = uuid::Uuid::new_v4(); // not the pending id
    let t2 = reduce(
        &t.state,
        EngineCommand::AcceptDirectorPlan {
            generation_id: other,
            plan: Box::new(valid_plan()),
        },
        &pack,
    )
    .expect("ok (identity)");

    // Late-discard: state unchanged, pending untouched, no effects.
    assert_eq!(t2.state.game.turn, t.state.game.turn);
    assert_eq!(t2.state.game.revision, t.state.game.revision);
    assert_eq!(
        t2.state.pending.is_some(),
        t.state.pending.is_some(),
        "pending presence unchanged"
    );
    assert!(
        !t2.state
            .pending
            .as_ref()
            .and_then(|p| p.validated_plan.as_ref())
            .is_some(),
        "validated plan still absent after late discard"
    );
    assert!(t2.effects.is_empty(), "no effects on late discard");
}

#[test]
fn mismatched_writer_finish_generation_id_is_discarded() {
    let (state, pack) = prime_then_enable_director();
    let t = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let gid = t
        .effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::RequestDirector { generation_id, .. } => Some(*generation_id),
            _ => None,
        })
        .expect("RequestDirector");
    let t = reduce(
        &t.state,
        EngineCommand::AcceptDirectorPlan {
            generation_id: gid,
            plan: Box::new(valid_plan()),
        },
        &pack,
    )
    .expect("accept director");

    let other = uuid::Uuid::new_v4();
    let t2 = reduce(
        &t.state,
        EngineCommand::AcceptWriterFinish {
            finish: Box::new(WriterFinish {
                generation_id: other,
                full_text: "ignored".to_string(),
                usage: None,
                finish_reason: None,
            }),
        },
        &pack,
    )
    .expect("ok (identity)");

    assert_eq!(t2.state.game.turn, t.state.game.turn);
    assert_eq!(t2.state.game.revision, t.state.game.revision);
    assert!(t2.effects.is_empty(), "no effects on late discard");
}

#[test]
fn accept_director_plan_on_mvp_path_is_no_op() {
    let pack = sample_pack();
    let init = EngineState::initial(&pack);
    // MVP path (use_director_writer == false). No pending generation. Feeding
    // a stray AcceptDirectorPlan must be a silent identity.
    let t = reduce(
        &init,
        EngineCommand::AcceptDirectorPlan {
            generation_id: uuid::Uuid::new_v4(),
            plan: Box::new(valid_plan()),
        },
        &pack,
    )
    .expect("ok");
    assert_eq!(t.state.game.turn, init.game.turn);
    assert!(t.effects.is_empty());
}
