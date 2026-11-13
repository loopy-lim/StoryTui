//! Contract tests for [`BuiltinStoryProvider`]: verifies that StoryTUI runs
//! complete multi-turn interactive stories, streaming, 2-stage Director/Writer,
//! side commands, choice refreshes, and cancellations with zero external LLM
//! servers running.

use story_core::action_parse::{ParseContext, parse_action};
use story_core::engine::{EngineState, reduce};
use story_core::engine_cmd::{EngineCommand, EngineEffect};
use story_core::tests_support::sample_pack;
use story_model::{ChoiceView, ProviderError, ProviderHealth, WriterEvent};
use story_provider::{
    BUILTIN_DEFAULT_MODEL, BUILTIN_PROVIDER_ID, BuiltinStoryProvider, LlmProvider,
    builtin_provider_config, is_builtin_config,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

async fn drive_single_pass_effect(
    state: &mut EngineState,
    pack: &story_model::StoryPack,
    provider: &BuiltinStoryProvider,
    command: EngineCommand,
    stream: bool,
) -> Vec<ChoiceView> {
    let mut transition = reduce(state, command, pack).expect("reduce command");
    *state = transition.state;
    let mut latest_choices = Vec::new();
    let mut safety = 0usize;

    loop {
        safety += 1;
        assert!(safety <= 4, "unexpectedly deep effect chain");

        let mut next_generation = None;
        for effect in transition.effects {
            match effect {
                EngineEffect::PublishView(view) => {
                    latest_choices = view.choices.clone();
                }
                EngineEffect::ShowRecoverableError(err) => {
                    panic!("unexpected recoverable error: {err:?}");
                }
                EngineEffect::RequestGeneration { request, .. } => {
                    next_generation = Some(request);
                }
                EngineEffect::RequestDirector { .. } | EngineEffect::RequestWriter { .. } => {
                    panic!("unexpected 2-stage effect in single-pass test");
                }
                EngineEffect::PersistStableState { .. } => {}
            }
        }

        let Some(request) = next_generation else {
            break;
        };

        let token = CancellationToken::new();
        let draft = if stream {
            let (tx, mut rx) = mpsc::channel(64);
            let draft = provider
                .generate_turn_stream(request, tx, token)
                .await
                .expect("stream turn");
            let mut saw_done = false;
            let mut streamed = String::new();
            while let Some(ev) = rx.recv().await {
                match ev {
                    WriterEvent::Chunk { text } => streamed.push_str(&text),
                    WriterEvent::Done { .. } => saw_done = true,
                    WriterEvent::Error { detail } => panic!("unexpected stream error: {detail}"),
                }
            }
            assert!(saw_done, "expected WriterEvent::Done");
            assert_eq!(streamed, draft.narration);
            draft
        } else {
            provider
                .generate_turn(request, token)
                .await
                .expect("generate turn")
        };

        transition = reduce(
            state,
            EngineCommand::AcceptGeneration {
                draft: Box::new(draft),
            },
            pack,
        )
        .expect("accept generation");
        *state = transition.state;
    }

    latest_choices
}

async fn drive_two_stage_effect(
    state: &mut EngineState,
    pack: &story_model::StoryPack,
    provider: &BuiltinStoryProvider,
    command: EngineCommand,
) -> Vec<ChoiceView> {
    let mut transition = reduce(state, command, pack).expect("reduce command");
    *state = transition.state;
    let mut latest_choices = Vec::new();
    let mut safety = 0usize;

    loop {
        safety += 1;
        assert!(safety <= 6, "unexpectedly deep 2-stage effect chain");

        let mut dir_req = None;
        let mut wri_req = None;
        let mut gen_req = None;
        for effect in transition.effects {
            match effect {
                EngineEffect::PublishView(view) => {
                    latest_choices = view.choices.clone();
                }
                EngineEffect::ShowRecoverableError(err) => {
                    panic!("unexpected recoverable error: {err:?}");
                }
                EngineEffect::RequestDirector { request, .. } => dir_req = Some(request),
                EngineEffect::RequestWriter { request, .. } => wri_req = Some(request),
                EngineEffect::RequestGeneration { request, .. } => gen_req = Some(request),
                EngineEffect::PersistStableState { .. } => {}
            }
        }

        let token = CancellationToken::new();
        if let Some(request) = dir_req {
            let gen_id = request.generation_id;
            let plan = provider.direct(request, token).await.expect("direct");
            transition = reduce(
                state,
                EngineCommand::AcceptDirectorPlan {
                    generation_id: gen_id,
                    plan: Box::new(plan),
                },
                pack,
            )
            .expect("accept director plan");
            *state = transition.state;
            continue;
        }
        if let Some(request) = wri_req {
            let (tx, mut rx) = mpsc::channel(64);
            let finish = provider
                .write_stream(request, tx, token)
                .await
                .expect("write_stream");
            let mut saw_done = false;
            let mut streamed = String::new();
            while let Some(ev) = rx.recv().await {
                match ev {
                    WriterEvent::Chunk { text } => streamed.push_str(&text),
                    WriterEvent::Done { .. } => saw_done = true,
                    WriterEvent::Error { detail } => {
                        panic!("unexpected writer stream error: {detail}")
                    }
                }
            }
            assert!(saw_done);
            assert_eq!(streamed, finish.full_text);
            transition = reduce(
                state,
                EngineCommand::AcceptWriterFinish {
                    finish: Box::new(finish),
                },
                pack,
            )
            .expect("accept writer finish");
            *state = transition.state;
            continue;
        }
        if let Some(request) = gen_req {
            let draft = provider
                .generate_turn(request, token)
                .await
                .expect("single-pass fallback/side");
            transition = reduce(
                state,
                EngineCommand::AcceptGeneration {
                    draft: Box::new(draft),
                },
                pack,
            )
            .expect("accept generation");
            *state = transition.state;
            continue;
        }
        break;
    }

    latest_choices
}

#[tokio::test]
async fn metadata_health_and_models_match_builtin_contract() {
    let cfg = builtin_provider_config();
    assert!(is_builtin_config(&cfg));
    let provider = BuiltinStoryProvider::new(cfg);

    assert_eq!(provider.id(), BUILTIN_PROVIDER_ID);
    assert_eq!(provider.model_name(), BUILTIN_DEFAULT_MODEL);
    assert_eq!(provider.health().await.unwrap(), ProviderHealth::Ready);
    let models = provider.list_models().await.unwrap();
    assert!(models.contains(&"builtin-story-v1".to_string()));
    assert!(models.contains(&"builtin-story-fast".to_string()));

    let caps = provider.capabilities();
    assert!(caps.supports_structured_output);
    assert!(caps.supports_director);
    assert!(caps.supports_streaming);
}

#[tokio::test]
async fn plays_eight_turns_and_free_action_without_rejections() {
    let pack = sample_pack();
    let provider = BuiltinStoryProvider::new(builtin_provider_config());
    let mut state = EngineState::initial(&pack);

    let mut choices = drive_single_pass_effect(
        &mut state,
        &pack,
        &provider,
        EngineCommand::StartNewGame,
        true,
    )
    .await;
    assert_eq!(state.game.turn, 1);
    assert!(!choices.is_empty());

    // 7 choice turns -> turn 8
    for idx in 0..7 {
        let choice_id = choices[idx % choices.len()].id.clone();
        choices = drive_single_pass_effect(
            &mut state,
            &pack,
            &provider,
            EngineCommand::SubmitChoice { choice_id },
            idx % 2 == 0,
        )
        .await;
        assert!(!choices.is_empty());
    }
    assert_eq!(state.game.turn, 8);

    // 1 free action turn -> turn 9
    let action = parse_action(
        "창문을 살짝 열어 비 냄새를 맡는다",
        &ParseContext {
            known_characters: &pack.characters,
            known_locations: &pack.rules.locations,
        },
    );
    let choices = drive_single_pass_effect(
        &mut state,
        &pack,
        &provider,
        EngineCommand::SubmitFreeAction {
            action: Box::new(action),
        },
        false,
    )
    .await;
    assert_eq!(state.game.turn, 9);
    assert!(!choices.is_empty());
    assert!(state.pending.is_none());
}

#[tokio::test]
async fn supports_two_stage_director_writer_and_side_commands() {
    let pack = sample_pack();
    let provider = BuiltinStoryProvider::new(builtin_provider_config());
    let mut state = EngineState::initial(&pack);
    state.use_director_writer = true;

    let choices =
        drive_two_stage_effect(&mut state, &pack, &provider, EngineCommand::StartNewGame).await;
    assert_eq!(state.game.turn, 1);
    assert!(!choices.is_empty());

    let choice_id = choices[0].id.clone();
    let _ = drive_two_stage_effect(
        &mut state,
        &pack,
        &provider,
        EngineCommand::SubmitChoice { choice_id },
    )
    .await;
    assert_eq!(state.game.turn, 2);

    // Side commands: ContinueNarration, GroupScene, SuggestAction, and RegenerateChoices
    let _ = drive_two_stage_effect(
        &mut state,
        &pack,
        &provider,
        EngineCommand::ContinueNarration,
    )
    .await;
    assert_eq!(state.game.turn, 2);

    let _ = drive_two_stage_effect(&mut state, &pack, &provider, EngineCommand::GroupScene).await;
    assert_eq!(state.game.turn, 2);

    let _ =
        drive_two_stage_effect(&mut state, &pack, &provider, EngineCommand::SuggestAction).await;
    assert!(
        state.take_suggested_action().is_some(),
        "expected suggested action text"
    );

    let refreshed = drive_two_stage_effect(
        &mut state,
        &pack,
        &provider,
        EngineCommand::RegenerateChoices,
    )
    .await;
    assert!(!refreshed.is_empty());
}

#[tokio::test]
async fn cancellation_returns_cancelled_error() {
    let pack = sample_pack();
    let provider = BuiltinStoryProvider::new(builtin_provider_config());
    let state = EngineState::initial(&pack);
    let transition = reduce(&state, EngineCommand::StartNewGame, &pack).expect("start");
    let request = transition
        .effects
        .into_iter()
        .find_map(|eff| match eff {
            EngineEffect::RequestGeneration { request, .. } => Some(request),
            _ => None,
        })
        .expect("generation request");

    let token = CancellationToken::new();
    token.cancel();
    let err = provider
        .generate_turn(request, token)
        .await
        .expect_err("cancelled");
    assert!(matches!(err, ProviderError::Cancelled));
}
