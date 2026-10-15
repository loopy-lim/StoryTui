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

use story_core::engine::{EngineState, NarrationRepair, reduce};
use story_core::engine_cmd::{EngineCommand, EngineEffect};
use story_model::{
    CharacterDef, CharacterId, Choice, DirectorPlan, GeneratedTurnDraft, LocationDef, LocationId,
    PackContentPolicy, PackInitialState, PackMetadata, PackRating, PromptFragments,
    RelationshipStage, RiskLevel, StatePatch, StoryPack, StoryRules, TimeSlot, TurnSummary,
    WriterFinish,
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

/// A Director plan whose patch references an unknown character (validation
/// failure). The engine MUST reject it and NOT emit RequestWriter.
fn bad_plan_unknown_character() -> DirectorPlan {
    let mut patch = StatePatch::default();
    patch
        .affection_delta
        .insert(CharacterId("c-ghost".to_string()), 1);
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
            plan: Box::new(bad_plan_unknown_character()),
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
            plan: Box::new(bad_plan_unknown_character()),
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
