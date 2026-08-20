//! Top-level engine orchestration.
//!
//! Two layers live here:
//!
//! - [`commit_turn`] (W3) — the high-level "validate + atomically apply one
//!   patch" primitive. Bumps `revision`; does **not** bump `turn`.
//! - [`reduce`] (W5) — the Elm-style command/effect boundary. Given an
//!   [`EngineState`] + [`EngineCommand`] + [`StoryPack`], it produces a
//!   [`Transition`] (new state + described [`EngineEffect`]s + audit log)
//!   without performing any HTTP/FS/UI work itself (ADR-003). The host app
//!   interprets the effects.
//!
//! # Checkpoints & late-discard invariants
//!
//! - On [`EngineCommand::SubmitChoice`] the current `game` is snapshotted as
//!   [`PendingGeneration::checkpoint`]. A later failed [`EngineCommand::AcceptGeneration`]
//!   or a [`EngineCommand::CancelGeneration`] restores it byte-identically —
//!   the committed stable state is never corrupted by a partial commit.
//! - A late/mismatched `AcceptGeneration` (no pending generation, or a
//!   `generation_id` that does not match) is silently discarded: the returned
//!   [`Transition`] is identity (state unchanged, no effects).

use sha2::Digest;
use story_model::chrono::Utc;
use story_model::uuid::Uuid;
use story_model::{
    ActionType, AppViewModel, CharacterId, CharacterState, ChoiceRef, DirectorPlan,
    DirectorRequest, DomainErrorView, Emotion, GameState, HistoryEntry, LoadingState, PackMetadata,
    ParsedAction, SavePayload, Screen, StateChangeLog, StatePatch, StoryPack, StoryRules,
    TurnRequest, WriterFinish, WriterRequest,
};

use crate::director::{ValidatedPlan, validate_plan};
use crate::engine_cmd::{EngineCommand, EngineEffect};
use crate::error::{
    CoreError, ENGINE_GENERATION_IN_FLIGHT, ENGINE_NO_CHOICES, ENGINE_UNKNOWN_CHOICE,
};
use crate::prompt_context::{build_prompt, build_prompt_with_choice};
use crate::reducer::{ApplyResult, apply_patch};
use crate::writer::{WriterResult, finalize_writer_output};

/// Sentinel `player_choice_id` for the opening generation (no player choice).
const OPENING_CHOICE_ID: &str = "__opening__";
/// Sentinel `player_choice_id` for a free-text action (no prepared choice).
const FREE_ACTION_CHOICE_ID: &str = "__free_action__";
/// Maximum number of committed-state checkpoints retained for
/// [`EngineCommand::RestoreCheckpoint`].
const CHECKPOINT_RING: usize = 8;
/// Slot id the engine writes to on autosave (after each accepted turn).
pub const AUTOSAVE_SLOT: &str = "autosave";
/// Cap on the in-memory audit tail kept on [`EngineState`] for the save
/// payload. Larger histories are truncated to the most recent entries so
/// saves stay compact.
const AUDIT_TAIL_CAP: usize = 1024;

/// Validate + atomically apply a generated patch for one turn (W3 primitive).
///
/// Delegates to [`apply_patch`]; bumped `revision` lives on the returned
/// [`ApplyResult::state`]. The `turn` field is **not** advanced here —
/// [`reduce`] owns turn-tick semantics.
pub fn commit_turn(
    state: &GameState,
    rules: &StoryRules,
    patch: &StatePatch,
    generation_id: Uuid,
) -> Result<ApplyResult, CoreError> {
    apply_patch(state, rules, patch, generation_id)
}

// ---------------------------------------------------------------------------
// Engine state
// ---------------------------------------------------------------------------

/// The engine's full state between commands. Plain data: no provider handle,
/// no terminal handle. Cloning it is cheap enough for the test-driven slice;
/// the host app keeps one `EngineState` and replaces it with each
/// [`Transition::state`].
#[derive(Clone, Debug)]
pub struct EngineState {
    /// The committed stable game state. When `pending` is `Some`, this is the
    /// pre-choice checkpoint (the in-flight turn has NOT been applied yet).
    pub game: GameState,
    /// Pack metadata carried for the view (title, etc.) so the engine does not
    /// need the full `StoryPack` on every render.
    pub pack_metadata: PackMetadata,
    /// The in-flight generation, if any. At most one is ever pending.
    pub pending: Option<PendingGeneration>,
    /// A failed generation that can be retried from its original checkpoint.
    /// Failed requests are no longer loading, but the player can retry them.
    pub retry_pending: Option<PendingGeneration>,
    /// Attempted `generation_id`s for the current pending turn (diagnostics /
    /// regenerate). Length 1 on a fresh request; grows by one per
    /// [`EngineCommand::RegenerateCurrentTurn`]. Cleared when the turn is
    /// accepted, cancelled, or failed.
    pub lineage: Vec<Uuid>,
    /// Committed-state ring for [`EngineCommand::RestoreCheckpoint`]. A new
    /// entry is pushed each time a turn is accepted; the ring keeps the most
    /// recent [`CHECKPOINT_RING`] entries.
    pub checkpoints: Vec<Checkpoint>,
    /// Tail of the field-level audit log accumulated across accepted turns.
    /// Bounded to the most recent [`AUDIT_TAIL_CAP`] entries so saves stay
    /// compact; included in [`EngineState::snapshot_payload`].
    pub audit_tail: Vec<StateChangeLog>,
    /// When `true` (default), the engine emits a [`EngineEffect::PersistStableState`]
    /// effect after each accepted turn, writing to the [`AUTOSAVE_SLOT`].
    /// Disable for headless benchmarks / tests that don't persist.
    pub autosave_on_accept: bool,
    /// When `true`, [`EngineCommand::SubmitChoice`] / [`EngineCommand::SubmitFreeAction`]
    /// route through the optional Director → Writer two-stage path (W10,
    /// ST-033/034): the engine emits [`EngineEffect::RequestDirector`] and
    /// expects [`EngineCommand::AcceptDirectorPlan`] →
    /// [`EngineEffect::RequestWriter`] → [`EngineCommand::AcceptWriterFinish`].
    ///
    /// **Default `false` (ADR-005, load-bearing):** the MVP single-call path
    /// ([`EngineEffect::RequestGeneration`] / [`EngineCommand::AcceptGeneration`])
    /// stays the default until a real 20-turn A/B gate proves the 2-stage path
    /// is at least as good. The flag is never flipped by the engine itself.
    pub use_director_writer: bool,
}

/// A committed-state snapshot the player can rewind to via
/// [`EngineCommand::RestoreCheckpoint`].
#[derive(Clone, Debug)]
pub struct Checkpoint {
    /// Stable unique id for this checkpoint (the host surfaces it as a
    /// restore target).
    pub id: Uuid,
    /// The committed `GameState` at the moment this checkpoint was taken.
    pub game: GameState,
}

/// A generation the engine has requested but not yet resolved.
#[derive(Clone, Debug)]
pub struct PendingGeneration {
    /// Unique id, echoed by the provider's `GeneratedTurnDraft.generation_id`.
    pub generation_id: Uuid,
    /// Pre-choice stable `GameState` to restore on cancel or apply failure.
    pub checkpoint: GameState,
    /// The choice id the player picked (`OPENING_CHOICE_ID` for the opening,
    /// `FREE_ACTION_CHOICE_ID` for a free-text action).
    pub player_choice_id: String,
    /// Turn counter at request time; the accepted turn becomes `requested_at_turn + 1`.
    pub requested_at_turn: u32,
    /// The parsed free-text action, when this generation originated from a
    /// [`EngineCommand::SubmitFreeAction`]. `None` for choices and the opening.
    /// Carried through to the accepted turn's `HistoryEntry.player_action` and
    /// into the prompt (with the player-sovereignty clause).
    pub player_action: Option<ParsedAction>,
    /// (2-stage path) The Director's validated plan, set by
    /// [`EngineCommand::AcceptDirectorPlan`] between the Director and Writer
    /// calls. Always `None` on the MVP single-call path. `None` between
    /// [`EngineCommand::SubmitChoice`] and `AcceptDirectorPlan` on the 2-stage
    /// path.
    pub validated_plan: Option<Box<ValidatedPlan>>,
    /// `true` only for an internal choice refresh after an already accepted
    /// turn. Its response replaces choices but never applies narration/patch.
    pub choice_refresh: bool,
}

/// The result of [`reduce`]: a new state, the effects the host should perform,
/// and a field-level audit trail.
#[derive(Clone, Debug)]
pub struct Transition {
    /// The post-command engine state.
    pub state: EngineState,
    /// Side-effects to interpret (request a generation, publish a view, ...).
    pub effects: Vec<EngineEffect>,
    /// Field-level change-log entries produced this command (empty unless the
    /// command applied a patch).
    pub audit: Vec<StateChangeLog>,
}

impl Transition {
    fn identity(state: EngineState) -> Self {
        Self {
            state,
            effects: Vec::new(),
            audit: Vec::new(),
        }
    }
}

impl EngineState {
    /// Build the initial engine state for `pack`: fresh `GameState` derived
    /// from `pack.metadata.initial_state` + characters, no pending generation.
    ///
    /// The host then issues [`EngineCommand::StartNewGame`] via [`reduce`] to
    /// request the opening turn's choices.
    pub fn initial(pack: &StoryPack) -> Self {
        Self {
            game: build_initial_game_state(pack),
            pack_metadata: pack.metadata.clone(),
            pending: None,
            retry_pending: None,
            lineage: Vec::new(),
            checkpoints: Vec::new(),
            audit_tail: Vec::new(),
            autosave_on_accept: true,
            use_director_writer: false,
        }
    }

    /// Assemble the pure-data [`SavePayload`] snapshot of this engine state.
    ///
    /// Mirrors `game.memories` / `game.history` into the payload's top-level
    /// mirrors (forward-compat with the W9 memory selector) and includes the
    /// bounded `audit_tail`. **Pure**: no FS, no UI, no HTTP — the host wraps
    /// this in a `SaveEnvelope` and hands it to `SaveStore::save`.
    pub fn snapshot_payload(&self) -> SavePayload {
        SavePayload {
            memories: self.game.memories.clone(),
            history: self.game.history.clone(),
            audit_tail: self.audit_tail.clone(),
            stable_state: self.game.clone(),
        }
    }

    /// Restore a stable, checksum-verified save payload for `pack`.
    ///
    /// Pending provider work is intentionally never persisted, so a resumed
    /// game always opens at a committed turn. The payload's top-level history
    /// and memories are the portable mirrors used on disk and become the
    /// authoritative display history after restore.
    pub fn from_snapshot_payload(pack: &StoryPack, payload: SavePayload) -> Self {
        let mut game = payload.stable_state;
        game.memories = payload.memories;
        game.history = payload.history;
        Self {
            game,
            pack_metadata: pack.metadata.clone(),
            pending: None,
            retry_pending: None,
            lineage: Vec::new(),
            checkpoints: Vec::new(),
            audit_tail: payload.audit_tail,
            autosave_on_accept: true,
            use_director_writer: false,
        }
    }
}

// ---------------------------------------------------------------------------
// reduce
// ---------------------------------------------------------------------------

/// Apply one command to the engine, returning the described [`Transition`].
///
/// Pure: performs no provider calls, no FS writes, no UI. The host interprets
/// [`EngineEffect::RequestGeneration`] by calling the provider and feeding the
/// result back as [`EngineCommand::AcceptGeneration`].
pub fn reduce(
    state: &EngineState,
    cmd: EngineCommand,
    pack: &StoryPack,
) -> Result<Transition, CoreError> {
    match cmd {
        EngineCommand::StartNewGame => reduce_start_new_game(state, pack),
        EngineCommand::SubmitChoice { choice_id } => {
            if state.use_director_writer {
                reduce_submit_choice_director(state, pack, choice_id)
            } else {
                reduce_submit_choice(state, pack, choice_id)
            }
        }
        EngineCommand::SubmitFreeAction { action } => {
            if state.use_director_writer {
                reduce_submit_free_action_director(state, pack, *action)
            } else {
                reduce_submit_free_action(state, pack, *action)
            }
        }
        EngineCommand::AcceptGeneration { draft } => reduce_accept(state, pack, *draft),
        EngineCommand::ProviderFailed {
            generation_id,
            error,
        } => Ok(reduce_provider_failed(state, pack, generation_id, error)),
        EngineCommand::AcceptDirectorPlan {
            generation_id,
            plan,
        } => reduce_accept_director_plan(state, pack, generation_id, *plan),
        EngineCommand::AcceptWriterFinish { finish } => {
            reduce_accept_writer_finish(state, pack, *finish)
        }
        EngineCommand::RegenerateCurrentTurn => Ok(reduce_regenerate(state, pack)),
        EngineCommand::RegenerateChoices => Ok(reduce_choice_refresh(state, pack)),
        EngineCommand::RestoreCheckpoint { checkpoint_id } => {
            Ok(reduce_restore_checkpoint(state, pack, checkpoint_id))
        }
        EngineCommand::CancelGeneration { generation_id } => {
            Ok(reduce_cancel(state, pack, generation_id))
        }
        EngineCommand::FallbackToSinglePass { generation_id } => {
            Ok(reduce_fallback_to_single_pass(state, pack, generation_id))
        }
        EngineCommand::DismissError => Ok(reduce_dismiss_error(state, pack)),
        EngineCommand::Quit => Ok(Transition::identity(clone_state(state))),
    }
}

fn reduce_start_new_game(state: &EngineState, pack: &StoryPack) -> Result<Transition, CoreError> {
    let mut game = build_initial_game_state(pack);
    game.stable_generation_id = None;
    let generation_id = Uuid::new_v4();
    let pending = PendingGeneration {
        generation_id,
        checkpoint: game.clone(),
        player_choice_id: OPENING_CHOICE_ID.to_string(),
        requested_at_turn: game.turn,
        player_action: None,
        validated_plan: None,
        choice_refresh: false,
    };
    let mut next = clone_state(state);
    next.game = game;
    next.pack_metadata = pack.metadata.clone();
    next.pending = Some(pending.clone());
    next.retry_pending = None;
    next.lineage = vec![generation_id];

    let loading_view = build_view(&next, pack, Some(&pending), None);
    let effects = if state.use_director_writer {
        // 2-stage path: plan the opening with the Director and narrate it with
        // the Writer, so the configured Writer model renders the first scene
        // too (consistent with every subsequent turn).
        let request = build_director_request(generation_id, pack, &next.game, None);
        vec![
            EngineEffect::RequestDirector {
                generation_id,
                request,
            },
            EngineEffect::PublishView(Box::new(loading_view)),
        ]
    } else {
        let request = build_turn_request(generation_id, pack, &next.game, None, None);
        vec![
            EngineEffect::RequestGeneration {
                generation_id,
                request,
            },
            EngineEffect::PublishView(Box::new(loading_view)),
        ]
    };
    Ok(Transition {
        state: next,
        effects,
        audit: Vec::new(),
    })
}

fn reduce_submit_choice(
    state: &EngineState,
    pack: &StoryPack,
    choice_id: String,
) -> Result<Transition, CoreError> {
    if state.pending.is_some() {
        return Err(CoreError::invalid_command(
            ENGINE_GENERATION_IN_FLIGHT,
            "a generation is already in flight; cancel it before submitting another choice",
        ));
    }
    // Validate the choice id against the most recent turn's offered choices.
    let last_entry =
        state.game.history.last().ok_or_else(|| {
            CoreError::invalid_command(ENGINE_NO_CHOICES, "no choices available yet")
        })?;
    let Some(selected_choice) = last_entry.choices.iter().find(|c| c.id == choice_id) else {
        return Err(CoreError::invalid_command(
            ENGINE_UNKNOWN_CHOICE,
            format!("unknown choice id: {choice_id}"),
        ));
    };

    let generation_id = Uuid::new_v4();
    let pending = PendingGeneration {
        generation_id,
        checkpoint: state.game.clone(),
        player_choice_id: choice_id,
        requested_at_turn: state.game.turn,
        player_action: None,
        validated_plan: None,
        choice_refresh: false,
    };
    let request = build_turn_request(
        generation_id,
        pack,
        &state.game,
        None,
        Some(selected_choice),
    );

    let mut next = clone_state(state);
    next.pending = Some(pending.clone());
    next.retry_pending = None;
    next.lineage = vec![generation_id];

    let loading_view = build_view(&next, pack, Some(&pending), None);
    let effects = vec![
        EngineEffect::RequestGeneration {
            generation_id,
            request,
        },
        EngineEffect::PublishView(Box::new(loading_view)),
    ];
    Ok(Transition {
        state: next,
        effects,
        audit: Vec::new(),
    })
}

/// Handle [`EngineCommand::SubmitFreeAction`]. Like [`reduce_submit_choice`]
/// but the request's prompt carries the parsed action + the player-sovereignty
/// clause, and the accepted turn's `HistoryEntry.player_action` is populated.
fn reduce_submit_free_action(
    state: &EngineState,
    pack: &StoryPack,
    action: ParsedAction,
) -> Result<Transition, CoreError> {
    if state.pending.is_some() {
        return Err(CoreError::invalid_command(
            ENGINE_GENERATION_IN_FLIGHT,
            "a generation is already in flight; cancel it before submitting a free action",
        ));
    }

    let generation_id = Uuid::new_v4();
    let pending = PendingGeneration {
        generation_id,
        checkpoint: state.game.clone(),
        player_choice_id: FREE_ACTION_CHOICE_ID.to_string(),
        requested_at_turn: state.game.turn,
        player_action: Some(action.clone()),
        validated_plan: None,
        choice_refresh: false,
    };
    let request = build_turn_request(generation_id, pack, &state.game, Some(&action), None);

    let mut next = clone_state(state);
    next.pending = Some(pending.clone());
    next.retry_pending = None;
    next.lineage = vec![generation_id];

    let loading_view = build_view(&next, pack, Some(&pending), None);
    let effects = vec![
        EngineEffect::RequestGeneration {
            generation_id,
            request,
        },
        EngineEffect::PublishView(Box::new(loading_view)),
    ];
    Ok(Transition {
        state: next,
        effects,
        audit: Vec::new(),
    })
}

/// Handle [`EngineCommand::RegenerateCurrentTurn`]. Cancels the in-flight
/// generation and re-requests from the SAME checkpoint with a fresh
/// `generation_id`; the new id is appended to `lineage`. The committed stable
/// state and history are NOT touched. No-op when nothing is pending.
fn reduce_regenerate(state: &EngineState, pack: &StoryPack) -> Transition {
    let Some(pending) = state.pending.as_ref().or(state.retry_pending.as_ref()) else {
        return Transition::identity(clone_state(state));
    };
    let generation_id = Uuid::new_v4();
    let new_pending = PendingGeneration {
        generation_id,
        checkpoint: pending.checkpoint.clone(),
        player_choice_id: pending.player_choice_id.clone(),
        requested_at_turn: pending.requested_at_turn,
        player_action: pending.player_action.clone(),
        // Regenerate always tears down any in-flight 2-stage plan: the fresh
        // generation id is for the MVP single-call path, and even on the
        // 2-stage path the host will re-issue a new Director request.
        validated_plan: None,
        choice_refresh: pending.choice_refresh,
    };
    let selected_choice = pending.checkpoint.history.last().and_then(|entry| {
        entry
            .choices
            .iter()
            .find(|choice| choice.id == pending.player_choice_id)
    });
    let request = if pending.choice_refresh {
        build_choice_refresh_request(generation_id, pack, &pending.checkpoint)
    } else {
        build_turn_request(
            generation_id,
            pack,
            &pending.checkpoint,
            pending.player_action.as_ref(),
            selected_choice,
        )
    };

    let mut next = clone_state(state);
    next.pending = Some(new_pending.clone());
    next.retry_pending = None;
    next.lineage.push(generation_id);

    let loading_view = build_view(&next, pack, Some(&new_pending), None);
    let effects = vec![
        EngineEffect::RequestGeneration {
            generation_id,
            request,
        },
        EngineEffect::PublishView(Box::new(loading_view)),
    ];
    Transition {
        state: next,
        effects,
        audit: Vec::new(),
    }
}

/// Handle [`EngineCommand::RestoreCheckpoint`]. Rewinds `game` to the stored
/// checkpoint, clears any pending generation, and clears `lineage`. No-op when
/// `checkpoint_id` does not match a stored checkpoint.
fn reduce_restore_checkpoint(
    state: &EngineState,
    pack: &StoryPack,
    checkpoint_id: Uuid,
) -> Transition {
    let Some(cp) = state
        .checkpoints
        .iter()
        .find(|c| c.id == checkpoint_id)
        .cloned()
    else {
        return Transition::identity(clone_state(state));
    };
    let mut next = clone_state(state);
    next.game = cp.game;
    next.pending = None;
    next.lineage = Vec::new();
    let view = build_view(&next, pack, None, None);
    Transition {
        state: next,
        effects: vec![EngineEffect::PublishView(Box::new(view))],
        audit: Vec::new(),
    }
}

fn reduce_accept(
    state: &EngineState,
    pack: &StoryPack,
    draft: story_model::GeneratedTurnDraft,
) -> Result<Transition, CoreError> {
    let Some(pending) = state.pending.as_ref() else {
        // Late discard: nothing pending. Identity transition, no effects.
        return Ok(Transition::identity(clone_state(state)));
    };
    if draft.generation_id != pending.generation_id {
        // Mismatched discard. Identity transition, no effects.
        return Ok(Transition::identity(clone_state(state)));
    }

    if pending.choice_refresh {
        return Ok(reduce_accept_choice_refresh(state, pack, pending, draft));
    }

    // Matching generation: apply the proposed patch against the checkpoint.
    match apply_patch(
        &pending.checkpoint,
        &pack.rules,
        &draft.proposed_patch,
        draft.generation_id,
    ) {
        Ok(result) => {
            let ending_resolved = result.ending.is_some();
            let (next, audit) = assemble_accepted_turn(state, pack, pending, draft.clone(), result);
            if !ending_resolved
                && pending.player_choice_id != OPENING_CHOICE_ID
                && pending.player_choice_id != FREE_ACTION_CHOICE_ID
                && choice_set_repeats(pending.checkpoint.history.last(), &draft.choices)
            {
                // Do not discard the accepted scene or ask the player to
                // intervene. One fresh request replaces choices only.
                return Ok(reduce_choice_refresh(&next, pack));
            }
            let view = build_view(&next, pack, None, None);
            let mut effects = vec![EngineEffect::PublishView(Box::new(view))];
            // Autosave-on-accept (configurable; default on for MVP). The host
            // executes the effect by calling SaveStore::save.
            if next.autosave_on_accept {
                effects.push(EngineEffect::PersistStableState {
                    slot_id: AUTOSAVE_SLOT.to_string(),
                    description: Some(format!("턴 {}", next.game.turn)),
                });
            }
            Ok(Transition {
                state: next,
                effects,
                audit,
            })
        }
        Err(err) => {
            // Restore the checkpoint byte-identically; surface a recoverable
            // error. The committed stable state is never corrupted.
            let error_view = domain_error_view(&err);
            let mut restored = clone_state(state);
            restored.game = pending.checkpoint.clone();
            restored.pending = None;
            restored.retry_pending = Some(pending.clone());
            restored.lineage = Vec::new();
            let view = build_view(&restored, pack, None, Some(error_view.clone()));
            Ok(Transition {
                state: restored,
                effects: vec![
                    EngineEffect::ShowRecoverableError(error_view),
                    EngineEffect::PublishView(Box::new(view)),
                ],
                audit: Vec::new(),
            })
        }
    }
}

/// Begin a choice-only refresh from the already committed current turn. The
/// checkpoint is the stable state itself, so cancellation/failure cannot undo
/// the narration or the validated state patch that led here.
fn reduce_choice_refresh(state: &EngineState, pack: &StoryPack) -> Transition {
    if state.pending.is_some()
        || state
            .game
            .history
            .last()
            .is_none_or(|entry| entry.choices.is_empty())
    {
        return Transition::identity(clone_state(state));
    }
    let generation_id = Uuid::new_v4();
    let pending = PendingGeneration {
        generation_id,
        checkpoint: state.game.clone(),
        player_choice_id: "__choice_refresh__".to_string(),
        requested_at_turn: state.game.turn,
        player_action: None,
        validated_plan: None,
        choice_refresh: true,
    };
    let request = build_choice_refresh_request(generation_id, pack, &state.game);
    let mut next = clone_state(state);
    next.pending = Some(pending.clone());
    next.retry_pending = None;
    next.lineage = vec![generation_id];
    let view = build_view(&next, pack, Some(&pending), None);
    let mut effects = Vec::new();
    // A choice-only repair starts after the narration and state patch have
    // already been accepted. Persist that stable turn before the next network
    // request so a refresh failure or process exit cannot lose it.
    if next.autosave_on_accept {
        effects.push(EngineEffect::PersistStableState {
            slot_id: AUTOSAVE_SLOT.to_string(),
            description: Some(format!("턴 {} (선택지 갱신 전)", next.game.turn)),
        });
    }
    effects.extend([
        EngineEffect::RequestGeneration {
            generation_id,
            request,
        },
        EngineEffect::PublishView(Box::new(view)),
    ]);
    Transition {
        state: next,
        effects,
        audit: Vec::new(),
    }
}

fn reduce_accept_choice_refresh(
    state: &EngineState,
    pack: &StoryPack,
    pending: &PendingGeneration,
    draft: story_model::GeneratedTurnDraft,
) -> Transition {
    // Provider recovery already verifies the basic output contract. The
    // engine additionally refuses an empty replacement so the playable turn
    // never loses its choices on a malformed late response.
    if draft.choices.is_empty() {
        return reduce_provider_failed(
            state,
            pack,
            pending.generation_id,
            DomainErrorView {
                code: "choice_refresh_empty".to_string(),
                title_ko: "선택지 갱신 실패".to_string(),
                message_ko: "새 선택지를 만들지 못했습니다. 기존 선택지를 유지합니다.".to_string(),
                recoverable: true,
            },
        );
    }
    let mut next = clone_state(state);
    if let Some(entry) = next.game.history.last_mut() {
        entry.choices = draft.choices;
    }
    next.game.revision = next.game.revision.saturating_add(1);
    next.pending = None;
    next.retry_pending = None;
    next.lineage.clear();
    let view = build_view(&next, pack, None, None);
    let mut effects = vec![EngineEffect::PublishView(Box::new(view))];
    if next.autosave_on_accept {
        effects.push(EngineEffect::PersistStableState {
            slot_id: AUTOSAVE_SLOT.to_string(),
            description: Some(format!("턴 {} · 선택지 갱신", next.game.turn)),
        });
    }
    Transition {
        state: next,
        effects,
        audit: Vec::new(),
    }
}

fn reduce_provider_failed(
    state: &EngineState,
    pack: &StoryPack,
    generation_id: Uuid,
    error: DomainErrorView,
) -> Transition {
    let Some(pending) = state.pending.as_ref() else {
        return Transition::identity(clone_state(state));
    };
    if pending.generation_id != generation_id {
        return Transition::identity(clone_state(state));
    }

    let mut restored = clone_state(state);
    restored.game = pending.checkpoint.clone();
    restored.pending = None;
    restored.retry_pending = Some(pending.clone());
    restored.lineage.clear();
    let view = build_view(&restored, pack, None, Some(error.clone()));
    Transition {
        state: restored,
        effects: vec![
            EngineEffect::ShowRecoverableError(error),
            EngineEffect::PublishView(Box::new(view)),
        ],
        audit: Vec::new(),
    }
}

fn reduce_cancel(state: &EngineState, pack: &StoryPack, generation_id: Uuid) -> Transition {
    let Some(pending) = state.pending.as_ref() else {
        return Transition::identity(clone_state(state));
    };
    if pending.generation_id != generation_id {
        return Transition::identity(clone_state(state));
    }
    let mut restored = clone_state(state);
    restored.game = pending.checkpoint.clone();
    restored.pending = None;
    restored.retry_pending = None;
    restored.lineage = Vec::new();
    let view = build_view(&restored, pack, None, None);
    Transition {
        state: restored,
        effects: vec![EngineEffect::PublishView(Box::new(view))],
        audit: Vec::new(),
    }
}

/// Handle [`EngineCommand::FallbackToSinglePass`]: the Director stage could not
/// produce a plan, so re-issue the SAME turn (same `generation_id`, same
/// checkpoint) as a single-pass [`EngineEffect::RequestGeneration`]. This is the
/// graceful-degradation path that keeps a weak Director model from blocking the
/// player. The pending generation is left in place; the single-pass result is
/// fed back as [`EngineCommand::AcceptGeneration`]. If that also fails the host
/// surfaces a normal error (it only falls back on a Director failure, so there
/// is no fallback loop).
fn reduce_fallback_to_single_pass(
    state: &EngineState,
    pack: &StoryPack,
    generation_id: Uuid,
) -> Transition {
    let Some(pending) = state.pending.as_ref() else {
        return Transition::identity(clone_state(state));
    };
    if pending.generation_id != generation_id {
        return Transition::identity(clone_state(state));
    }
    // Reuse the same generation_id so AcceptGeneration completes this turn.
    let request = build_turn_request(
        generation_id,
        pack,
        &pending.checkpoint,
        pending.player_action.as_ref(),
        None,
    );
    let next = clone_state(state);
    // Keep pending (still in flight); the lineage already holds this id.
    let loading = next.pending.clone();
    let loading_view = build_view(&next, pack, loading.as_ref(), None);
    Transition {
        state: next,
        effects: vec![
            EngineEffect::RequestGeneration {
                generation_id,
                request,
            },
            EngineEffect::PublishView(Box::new(loading_view)),
        ],
        audit: Vec::new(),
    }
}

fn reduce_dismiss_error(state: &EngineState, pack: &StoryPack) -> Transition {
    let mut next = clone_state(state);
    next.retry_pending = None;
    let view = build_view(&next, pack, None, None);
    Transition {
        state: next,
        effects: vec![EngineEffect::PublishView(Box::new(view))],
        audit: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// optional 2-stage Director/Writer path (W10, ADR-005: off by default)
// ---------------------------------------------------------------------------

/// Like [`reduce_submit_choice`] but emits [`EngineEffect::RequestDirector`]
/// instead of [`EngineEffect::RequestGeneration`]. Used when
/// [`EngineState::use_director_writer`] is `true`.
fn reduce_submit_choice_director(
    state: &EngineState,
    pack: &StoryPack,
    choice_id: String,
) -> Result<Transition, CoreError> {
    if state.pending.is_some() {
        return Err(CoreError::invalid_command(
            ENGINE_GENERATION_IN_FLIGHT,
            "a generation is already in flight; cancel it before submitting another choice",
        ));
    }
    let last_entry =
        state.game.history.last().ok_or_else(|| {
            CoreError::invalid_command(ENGINE_NO_CHOICES, "no choices available yet")
        })?;
    if !last_entry.choices.iter().any(|c| c.id == choice_id) {
        return Err(CoreError::invalid_command(
            ENGINE_UNKNOWN_CHOICE,
            format!("unknown choice id: {choice_id}"),
        ));
    }

    let generation_id = Uuid::new_v4();
    let pending = PendingGeneration {
        generation_id,
        checkpoint: state.game.clone(),
        player_choice_id: choice_id,
        requested_at_turn: state.game.turn,
        player_action: None,
        validated_plan: None,
        choice_refresh: false,
    };
    let request = build_director_request(generation_id, pack, &state.game, None);

    let mut next = clone_state(state);
    next.pending = Some(pending.clone());
    next.retry_pending = None;
    next.lineage = vec![generation_id];

    let loading_view = build_view(&next, pack, Some(&pending), None);
    let effects = vec![
        EngineEffect::RequestDirector {
            generation_id,
            request,
        },
        EngineEffect::PublishView(Box::new(loading_view)),
    ];
    Ok(Transition {
        state: next,
        effects,
        audit: Vec::new(),
    })
}

/// 2-stage variant of [`reduce_submit_free_action`].
fn reduce_submit_free_action_director(
    state: &EngineState,
    pack: &StoryPack,
    action: ParsedAction,
) -> Result<Transition, CoreError> {
    if state.pending.is_some() {
        return Err(CoreError::invalid_command(
            ENGINE_GENERATION_IN_FLIGHT,
            "a generation is already in flight; cancel it before submitting a free action",
        ));
    }

    let generation_id = Uuid::new_v4();
    let pending = PendingGeneration {
        generation_id,
        checkpoint: state.game.clone(),
        player_choice_id: FREE_ACTION_CHOICE_ID.to_string(),
        requested_at_turn: state.game.turn,
        player_action: Some(action.clone()),
        validated_plan: None,
        choice_refresh: false,
    };
    let request = build_director_request(generation_id, pack, &state.game, Some(&action));

    let mut next = clone_state(state);
    next.pending = Some(pending.clone());
    next.lineage = vec![generation_id];

    let loading_view = build_view(&next, pack, Some(&pending), None);
    let effects = vec![
        EngineEffect::RequestDirector {
            generation_id,
            request,
        },
        EngineEffect::PublishView(Box::new(loading_view)),
    ];
    Ok(Transition {
        state: next,
        effects,
        audit: Vec::new(),
    })
}

/// Handle [`EngineCommand::AcceptDirectorPlan`] (2-stage path): validate +
/// dry-run; on success store the [`ValidatedPlan`] on the pending generation
/// and emit [`EngineEffect::RequestWriter`]; on rejection restore the
/// checkpoint and surface a recoverable error (the Writer is NEVER called).
fn reduce_accept_director_plan(
    state: &EngineState,
    pack: &StoryPack,
    generation_id: Uuid,
    plan: DirectorPlan,
) -> Result<Transition, CoreError> {
    let Some(pending) = state.pending.as_ref() else {
        // Late discard — nothing pending. Identity transition, no effects.
        return Ok(Transition::identity(clone_state(state)));
    };
    if pending.generation_id != generation_id {
        // Mismatched discard. Identity transition, no effects.
        return Ok(Transition::identity(clone_state(state)));
    }

    match validate_plan(&plan, &pending.checkpoint, pack) {
        Ok(validated) => {
            let digest = plan_digest(&plan);
            let request = build_writer_request(
                pending.generation_id,
                pack,
                &pending.checkpoint,
                pending.player_action.as_ref(),
                &validated.plan,
                digest,
            );
            let mut next = clone_state(state);
            if let Some(ref mut p) = next.pending {
                p.validated_plan = Some(Box::new(validated));
            }
            let loading = next.pending.clone();
            let loading_view = build_view(&next, pack, loading.as_ref(), None);
            let effects = vec![
                EngineEffect::RequestWriter {
                    generation_id: pending.generation_id,
                    request,
                },
                EngineEffect::PublishView(Box::new(loading_view)),
            ];
            Ok(Transition {
                state: next,
                effects,
                audit: Vec::new(),
            })
        }
        Err(err) => {
            // Restore the checkpoint byte-identically; surface a recoverable
            // error. The Writer is NEVER called for a rejected plan.
            let error_view = domain_error_view(&err);
            let mut restored = clone_state(state);
            restored.game = pending.checkpoint.clone();
            restored.pending = None;
            restored.retry_pending = Some(pending.clone());
            restored.lineage = Vec::new();
            let view = build_view(&restored, pack, None, Some(error_view.clone()));
            Ok(Transition {
                state: restored,
                effects: vec![
                    EngineEffect::ShowRecoverableError(error_view),
                    EngineEffect::PublishView(Box::new(view)),
                ],
                audit: Vec::new(),
            })
        }
    }
}

/// Handle [`EngineCommand::AcceptWriterFinish`] (2-stage path): finalize the
/// Writer text (strip + lint embedded patches), then commit the validated
/// Director plan's patch + choices with the Writer's narration.
fn reduce_accept_writer_finish(
    state: &EngineState,
    pack: &StoryPack,
    finish: WriterFinish,
) -> Result<Transition, CoreError> {
    let Some(pending) = state.pending.as_ref() else {
        return Ok(Transition::identity(clone_state(state)));
    };
    if pending.generation_id != finish.generation_id {
        return Ok(Transition::identity(clone_state(state)));
    };
    let Some(validated) = pending.validated_plan.as_ref() else {
        // No validated plan — either the host fed back a Writer finish out of
        // order, or we are on the MVP path. Identity in either case.
        return Ok(Transition::identity(clone_state(state)));
    };

    let writer: WriterResult =
        finalize_writer_output(finish.full_text, validated, pending.generation_id)?;
    let (next, audit) = assemble_accepted_2stage_turn(state, pack, pending, validated, writer);
    let view = build_view(&next, pack, None, None);
    let mut effects = vec![EngineEffect::PublishView(Box::new(view))];
    if next.autosave_on_accept {
        effects.push(EngineEffect::PersistStableState {
            slot_id: AUTOSAVE_SLOT.to_string(),
            description: Some(format!("턴 {}", next.game.turn)),
        });
    }
    Ok(Transition {
        state: next,
        effects,
        audit,
    })
}

/// Recent-arc storage cap (FIFO). Keeps the prompt digest bounded; older
/// consequences persist via the engine state snapshot.
const RECENT_SUMMARY_CAP: usize = 12;

/// One-line semantic digest of an accepted turn, stored for the prompt's
/// `# Recent arc` block. Prefers the model's own `interpretation`; falls back
/// to the narration's first clause when the model omitted one.
pub fn turn_digest(narration: &str, interpretation: Option<&str>) -> String {
    if let Some(interp) = interpretation.map(str::trim).filter(|s| !s.is_empty()) {
        return interp.to_string();
    }
    crate::prompt_context::first_clause(narration, 120)
}

/// Push `digest` onto `game.recent_summaries`, keeping the most recent
/// [`RECENT_SUMMARY_CAP`] entries in 1:1 alignment with `game.history`.
fn push_recent_summary(game: &mut story_model::GameState, digest: String) {
    let queue = &mut game.recent_summaries;
    queue.push_back(digest);
    while queue.len() > RECENT_SUMMARY_CAP {
        queue.pop_front();
    }
}

/// Build the post-accept `EngineState` for the 2-stage path. Mirrors
/// [`assemble_accepted_turn`] but the narration comes from the Writer while
/// the patch + choices come from the Director's validated plan.
fn assemble_accepted_2stage_turn(
    state: &EngineState,
    pack: &StoryPack,
    pending: &PendingGeneration,
    validated: &ValidatedPlan,
    writer: WriterResult,
) -> (EngineState, Vec<StateChangeLog>) {
    let history_id = Uuid::new_v4();
    let fired_events = validated.dry_run.fired_events.clone();
    let new_memories = crate::memory::memories_from_turn(
        &validated.dry_run,
        pack,
        pending.requested_at_turn + 1,
        history_id,
    );

    let mut game = validated.dry_run.state.clone();
    game.turn = pending.requested_at_turn + 1;
    let selected_choice = selected_choice_from_pending(pending);

    if pending.player_choice_id != OPENING_CHOICE_ID
        && pending.player_choice_id != FREE_ACTION_CHOICE_ID
    {
        let label = pending
            .checkpoint
            .history
            .last()
            .and_then(|h| h.choices.iter().find(|c| c.id == pending.player_choice_id))
            .map(|c| c.label.clone())
            .unwrap_or_default();
        game.last_choice = Some(ChoiceRef {
            choice_id: pending.player_choice_id.clone(),
            label,
            turn: pending.requested_at_turn,
        });
    }

    crate::memory::resolve_memories(&mut game.memories, &fired_events);
    game.memories.extend(new_memories);

    let mut narration = writer.narration.clone();
    let mut choices = validated.plan.choices.clone();
    apply_resolved_ending(
        &mut narration,
        &mut choices,
        validated.dry_run.ending.as_deref(),
        pack,
    );
    let entry = HistoryEntry {
        id: history_id,
        turn: game.turn,
        scene_id: game.current_scene_id.clone(),
        created_at: Utc::now(),
        narration,
        player_action: recorded_player_action(pending),
        choices,
        applied_patch: validated.plan.proposed_patch.clone(),
        generation_id: pending.generation_id,
    };
    game.history.push(entry);
    push_recent_summary(&mut game, turn_digest(&writer.narration, None));
    crate::narrative::advance_after_accept(&mut game, pack, selected_choice.as_ref());
    crate::narrative::mark_recalled(&mut game, pack);
    game.stable_generation_id = Some(pending.generation_id);

    let mut checkpoints = state.checkpoints.clone();
    checkpoints.push(Checkpoint {
        id: Uuid::new_v4(),
        game: game.clone(),
    });
    if checkpoints.len() > CHECKPOINT_RING {
        let drop_n = checkpoints.len() - CHECKPOINT_RING;
        checkpoints.drain(0..drop_n);
    }

    let audit = validated.dry_run.change_log.clone();
    let next = EngineState {
        game,
        pack_metadata: state.pack_metadata.clone(),
        pending: None,
        retry_pending: None,
        lineage: Vec::new(),
        checkpoints,
        audit_tail: extend_audit_tail(&state.audit_tail, &audit),
        autosave_on_accept: state.autosave_on_accept,
        use_director_writer: state.use_director_writer,
    };
    // The writer lint pass is non-fatal; the offending text has already been
    // stripped. Surfacing it through the engine is future work (a tracing
    // log or a HistoryEntry warning) — for MVP it is intentionally dropped.
    let _ = writer.lint;
    (next, audit)
}

// ---------------------------------------------------------------------------
// turn acceptance helpers
// ---------------------------------------------------------------------------

/// Build the post-accept `EngineState` from the reducer result + draft.
///
/// - `turn` bumps by exactly 1 from `requested_at_turn`.
/// - A new `HistoryEntry` is appended (choices offered, applied patch, draft
///   narration). `player_action` records both direct text and numbered choices
///   so the transcript can show what led to the scene. The chosen id
///   is recorded on `game.last_choice` (skipped for the opening and free-action
///   sentinels).
/// - The pending generation is cleared and the committed state is pushed onto
///   the checkpoint ring for later [`EngineCommand::RestoreCheckpoint`].
fn assemble_accepted_turn(
    state: &EngineState,
    pack: &StoryPack,
    pending: &PendingGeneration,
    draft: story_model::GeneratedTurnDraft,
    result: ApplyResult,
) -> (EngineState, Vec<StateChangeLog>) {
    // Derive long-term memories from this turn's fired events BEFORE we move
    // `result.state` (ST-029). The history-entry id is generated once and used
    // both as the memory's `source_history_id` and the actual `HistoryEntry`
    // id, so every memory traces to a real, present source entry.
    let history_id = Uuid::new_v4();
    let fired_events = result.fired_events.clone();
    let new_memories =
        crate::memory::memories_from_turn(&result, pack, pending.requested_at_turn + 1, history_id);

    let ending = result.ending.clone();
    let mut game = result.state;
    game.turn = pending.requested_at_turn + 1;
    let selected_choice = selected_choice_from_pending(pending);

    // Record the player's choice (skip for the opening and free-action
    // sentinels — neither corresponds to a prepared choice).
    if pending.player_choice_id != OPENING_CHOICE_ID
        && pending.player_choice_id != FREE_ACTION_CHOICE_ID
    {
        let label = pending
            .checkpoint
            .history
            .last()
            .and_then(|h| h.choices.iter().find(|c| c.id == pending.player_choice_id))
            .map(|c| c.label.clone())
            .unwrap_or_default();
        game.last_choice = Some(ChoiceRef {
            choice_id: pending.player_choice_id.clone(),
            label,
            turn: pending.requested_at_turn,
        });
    }

    // Resolve prior memories against this turn's fired events, then append the
    // new ones. Resolution is deterministic and never deletes a memory — it
    // only clears the `unresolved` flag of matching priors.
    crate::memory::resolve_memories(&mut game.memories, &fired_events);
    game.memories.extend(new_memories);

    let mut narration = draft.narration.clone();
    let mut choices = draft.choices.clone();
    apply_resolved_ending(&mut narration, &mut choices, ending.as_deref(), pack);
    let entry = HistoryEntry {
        id: history_id,
        turn: game.turn,
        scene_id: game.current_scene_id.clone(),
        created_at: Utc::now(),
        narration,
        player_action: recorded_player_action(pending),
        choices,
        applied_patch: draft.proposed_patch.clone(),
        generation_id: draft.generation_id,
    };
    game.history.push(entry);
    push_recent_summary(
        &mut game,
        turn_digest(&draft.narration, draft.summary.interpretation.as_deref()),
    );
    crate::narrative::advance_after_accept(&mut game, pack, selected_choice.as_ref());
    crate::narrative::mark_recalled(&mut game, pack);
    game.stable_generation_id = Some(draft.generation_id);

    // Push the just-committed state onto the checkpoint ring (cap keeps the
    // most recent CHECKPOINT_RING entries).
    let mut checkpoints = state.checkpoints.clone();
    checkpoints.push(Checkpoint {
        id: Uuid::new_v4(),
        game: game.clone(),
    });
    if checkpoints.len() > CHECKPOINT_RING {
        let drop_n = checkpoints.len() - CHECKPOINT_RING;
        checkpoints.drain(0..drop_n);
    }

    let next = EngineState {
        game,
        pack_metadata: state.pack_metadata.clone(),
        pending: None,
        retry_pending: None,
        lineage: Vec::new(),
        checkpoints,
        audit_tail: extend_audit_tail(&state.audit_tail, &result.change_log),
        autosave_on_accept: state.autosave_on_accept,
        use_director_writer: state.use_director_writer,
    };
    (next, result.change_log)
}

fn apply_resolved_ending(
    narration: &mut String,
    choices: &mut Vec<story_model::Choice>,
    ending_id: Option<&str>,
    pack: &StoryPack,
) {
    let Some(ending_id) = ending_id else {
        return;
    };
    let label = pack
        .rules
        .endings
        .get(ending_id)
        .map(|ending| ending.label.as_str())
        .unwrap_or(ending_id);
    narration.push_str("\n\n[엔딩] ");
    narration.push_str(label);
    choices.clear();
}

/// Persist the action that led into a generated scene. Numbered choices are
/// represented like free input so history and the stacked TUI transcript can
/// show the player's actual direction.
fn selected_choice_from_pending(pending: &PendingGeneration) -> Option<story_model::Choice> {
    if pending.player_choice_id == OPENING_CHOICE_ID
        || pending.player_choice_id == FREE_ACTION_CHOICE_ID
    {
        return None;
    }
    pending
        .checkpoint
        .history
        .last()
        .and_then(|entry| {
            entry
                .choices
                .iter()
                .find(|choice| choice.id == pending.player_choice_id)
        })
        .cloned()
}

fn recorded_player_action(pending: &PendingGeneration) -> Option<ParsedAction> {
    if let Some(action) = &pending.player_action {
        return Some(action.clone());
    }
    if matches!(
        pending.player_choice_id.as_str(),
        OPENING_CHOICE_ID | FREE_ACTION_CHOICE_ID
    ) {
        return None;
    }
    let choice = pending.checkpoint.history.last().and_then(|entry| {
        entry
            .choices
            .iter()
            .find(|choice| choice.id == pending.player_choice_id)
    })?;
    Some(ParsedAction {
        action_type: ActionType::GeneralAction,
        target: choice.target.clone(),
        intent: Some(choice.intent.clone()),
        tone: choice.tone.clone(),
        content: choice.label.clone(),
        original_text: choice.label.clone(),
        confidence: Some(1.0),
    })
}

/// Append `new_entries` to the audit tail, trimming to the most recent
/// [`AUDIT_TAIL_CAP`] entries so the save payload stays compact.
fn extend_audit_tail(
    existing: &[StateChangeLog],
    new_entries: &[StateChangeLog],
) -> Vec<StateChangeLog> {
    let mut out = Vec::with_capacity(existing.len() + new_entries.len());
    out.extend_from_slice(existing);
    out.extend_from_slice(new_entries);
    if out.len() > AUDIT_TAIL_CAP {
        let drop_n = out.len() - AUDIT_TAIL_CAP;
        out.drain(0..drop_n);
    }
    out
}

// ---------------------------------------------------------------------------
// initial state construction
// ---------------------------------------------------------------------------

/// Build a fresh `GameState` for `pack`: location/time/chapter from
/// `metadata.initial_state`, one `CharacterState` per character def with
/// pack-declared numeric defaults, known facts, initial relationship stage,
/// and a `Neutral` emotion.
fn build_initial_game_state(pack: &StoryPack) -> GameState {
    let init = &pack.metadata.initial_state;
    let characters = pack
        .characters
        .iter()
        .map(|(id, def)| {
            (
                id.clone(),
                CharacterState {
                    affection: initial_character_stat(pack, &id.0, "affection"),
                    trust: initial_character_stat(pack, &id.0, "trust"),
                    tension: initial_character_stat(pack, &id.0, "tension"),
                    relationship_stage: def.initial_player_relationship,
                    current_emotion: Emotion::Neutral,
                    known_facts: def.known_facts.iter().cloned().collect(),
                    address_form_overrides: Default::default(),
                },
            )
        })
        .collect();
    let flags = pack
        .rules
        .allowed_flags
        .iter()
        .map(|id| (id.clone(), false))
        .collect();
    let numbers = pack
        .rules
        .numbers
        .iter()
        .map(|(id, def)| (id.clone(), def.default))
        .collect();
    GameState {
        save_schema_version: story_model::SAVE_SCHEMA_VERSION,
        story_id: pack.metadata.story_id.clone(),
        story_version: pack.metadata.story_version.clone(),
        pack_schema_version: pack.metadata.schema_version.clone(),
        session_id: Uuid::new_v4(),
        current_chapter: init.chapter.clone(),
        current_scene_id: init.scene.clone(),
        turn: 0,
        location: init.location.clone(),
        time_slot: init.time_slot,
        active_route: None,
        characters,
        flags,
        numbers,
        strings: Default::default(),
        inventory: Default::default(),
        occurred_events: Default::default(),
        recent_summaries: Default::default(),
        memories: Default::default(),
        history: Vec::new(),
        last_choice: None,
        revision: 0,
        stable_generation_id: None,
    }
}

fn initial_character_stat(pack: &StoryPack, character_id: &str, axis: &str) -> i16 {
    pack.rules
        .numbers
        .get(&format!("{character_id}.{axis}"))
        .and_then(|def| i16::try_from(def.default).ok())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// view construction
// ---------------------------------------------------------------------------

/// Build the [`AppViewModel`] for the given state. Narration + choices come
/// from the latest history entry (or the pack opening on turn 0). The primary
/// character (first in key order) supplies the status stats.
pub fn build_view(
    state: &EngineState,
    pack: &StoryPack,
    loading: Option<&PendingGeneration>,
    error: Option<DomainErrorView>,
) -> AppViewModel {
    let game = &state.game;
    let primary: Option<&CharacterId> = game.characters.keys().next();
    let (affection, trust, tension) = primary
        .and_then(|id| game.characters.get(id))
        .map(|c| (Some(c.affection), Some(c.trust), Some(c.tension)))
        .unwrap_or((None, None, None));

    let location_display = pack
        .rules
        .locations
        .get(&game.location)
        .map(|l| l.display_name.clone())
        .unwrap_or_else(|| game.location.0.clone());

    let (narration, choices) = match game.history.last() {
        Some(entry) => (
            entry.narration.clone(),
            entry
                .choices
                .iter()
                .map(|c| story_model::ChoiceView {
                    id: c.id.clone(),
                    label: c.label.clone(),
                    risk: c.risk,
                    preview: c.preview.clone(),
                })
                .collect::<Vec<_>>(),
        ),
        None => (pack.opening_md.clone(), Vec::new()),
    };

    let loading_state = loading.map(|p| LoadingState {
        generation_id: p.generation_id,
        message_ko: "생성 중...".to_string(),
    });

    AppViewModel {
        screen: Screen::Story,
        story_title: pack.metadata.title.clone(),
        chapter: game.current_chapter.clone(),
        location_display,
        time_slot: game.time_slot,
        narration,
        choices,
        status: story_model::StatusLine {
            route: game.active_route.clone(),
            affection,
            trust,
            tension,
            turn: game.turn,
        },
        loading: loading_state,
        error,
    }
}

// ---------------------------------------------------------------------------
// request construction
// ---------------------------------------------------------------------------

/// Build a compatibility-safe choice-only repair request. Local providers
/// currently share the full turn JSON contract, so the wire response still
/// contains narration and a patch; `reduce_accept_choice_refresh` deliberately
/// ignores them and commits only `choices`.
fn build_choice_refresh_request(
    generation_id: Uuid,
    pack: &StoryPack,
    game: &GameState,
) -> TurnRequest {
    let mut request = build_turn_request(generation_id, pack, game, None, None);
    let previous = game
        .history
        .last()
        .map(|entry| {
            entry
                .choices
                .iter()
                .map(|choice| format!("- {} | {}", choice.label, choice.intent))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    request.messages.push(story_model::ChatMessage {
        role: "user".to_string(),
        content: format!(
            "# Choice-only refresh\nThe narration and state are already committed. Return a full valid JSON envelope for compatibility, but only the `choices` array will be used. Give exactly three materially different next actions. Do not repeat these previous offers or paraphrase them:\n{previous}\nUse relationship, evidence, and situation directions where the current story allows; preserve player agency."
        ),
    });
    // A refresh needs only the envelope plus three concise choices, not a
    // second long scene. This reduces latency on local models.
    request.max_tokens = request.max_tokens.min(900);
    request
}

/// True only when at least two newly offered choices repeat a prior offer's
/// direction. It compares a compact semantic signature (role, target, and
/// intent-word overlap), never a regex over rendered prose.
fn choice_set_repeats(previous: Option<&HistoryEntry>, candidate: &[story_model::Choice]) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    let matches = candidate
        .iter()
        .filter(|choice| {
            previous
                .choices
                .iter()
                .any(|old| choices_match(old, choice))
        })
        .count();
    matches >= 2
}

fn choices_match(left: &story_model::Choice, right: &story_model::Choice) -> bool {
    let left_label = normalized_choice_text(&left.label);
    let right_label = normalized_choice_text(&right.label);
    if !left.intent.trim().is_empty()
        && !right.intent.trim().is_empty()
        && !left_label.is_empty()
        && (left_label == right_label
            || left_label.contains(&right_label)
            || right_label.contains(&left_label))
    {
        return true;
    }
    if crate::narrative::classify_choice(left) != crate::narrative::classify_choice(right)
        || left.target != right.target
    {
        return false;
    }
    let left_words = choice_words(&left.intent);
    let right_words = choice_words(&right.intent);
    if left_words.is_empty() || right_words.is_empty() {
        return false;
    }
    let shared = left_words.intersection(&right_words).count();
    shared * 2 >= left_words.len().min(right_words.len())
}

fn normalized_choice_text(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn choice_words(text: &str) -> std::collections::BTreeSet<String> {
    text.split_whitespace()
        .map(normalized_choice_text)
        .filter(|word| word.chars().count() >= 2)
        .collect()
}

/// Build a [`TurnRequest`] for one generation: deterministic system + user
/// prompt via [`build_prompt`] and MVP sampling defaults. The provider decides
/// whether to send the schema based on its structured-output mode. When
/// `player_action` is `Some`, the prompt carries the parsed free-text action
/// plus the player-sovereignty clause.
fn build_turn_request(
    generation_id: Uuid,
    pack: &StoryPack,
    game: &GameState,
    player_action: Option<&ParsedAction>,
    player_choice: Option<&story_model::Choice>,
) -> TurnRequest {
    let bundle = build_prompt_with_choice(pack, game, &game.history, player_action, player_choice);
    let messages = vec![
        story_model::ChatMessage {
            role: "system".to_string(),
            content: bundle.system,
        },
        story_model::ChatMessage {
            role: "user".to_string(),
            content: bundle.user,
        },
    ];
    TurnRequest {
        generation_id,
        messages,
        model: "mock".to_string(),
        temperature: 0.8,
        top_p: 0.95,
        top_k: 40,
        max_tokens: 2048,
        json_schema: Some(generated_turn_json_schema()),
    }
}

/// JSON Schema sent to OpenAI-compatible providers that support structured
/// output. Rust still performs strict parsing and reducer validation after the
/// response; this schema prevents common malformed-Qwen/MLX output shapes.
fn generated_turn_json_schema() -> story_model::serde_json::Value {
    story_model::serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["narration", "choices", "state_patch", "summary"],
        "properties": {
            "narration": {"type": "string"},
            "choices": {
                "type": "array",
                "minItems": 1,
                "maxItems": 6,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "label", "intent", "tone", "target", "risk", "hidden_requirements", "preview"],
                    "properties": {
                        "id": {"type": "string"},
                        "label": {"type": "string"},
                        "intent": {"type": "string"},
                        "tone": {"type": ["string", "null"]},
                        "target": {"type": ["string", "null"]},
                        "risk": {"type": "string", "enum": ["safe", "low", "medium", "high", "severe"]},
                        "hidden_requirements": {"type": "array", "items": {"type": "object"}},
                        "preview": {"type": ["string", "null"]}
                    }
                }
            },
            "state_patch": {
                "type": "object",
                "additionalProperties": false,
                "required": ["affection_delta", "trust_delta", "tension_delta", "relationship_transitions", "set_flags", "number_delta", "set_strings", "inventory_delta", "add_events", "next_location", "next_time_slot", "proposed_route"],
                "properties": {
                    "affection_delta": {"type": "object", "additionalProperties": {"type": "integer"}},
                    "trust_delta": {"type": "object", "additionalProperties": {"type": "integer"}},
                    "tension_delta": {"type": "object", "additionalProperties": {"type": "integer"}},
                    "relationship_transitions": {"type": "object", "additionalProperties": {"type": "string"}},
                    "set_flags": {"type": "object", "additionalProperties": {"type": "boolean"}},
                    "number_delta": {"type": "object", "additionalProperties": {"type": "integer"}},
                    "set_strings": {"type": "object", "additionalProperties": {"type": "string"}},
                    "inventory_delta": {"type": "object", "additionalProperties": {"type": "integer"}},
                    "add_events": {"type": "array", "items": {"type": "string"}},
                    "next_location": {"type": ["string", "null"]},
                    "next_time_slot": {"type": ["string", "null"]},
                    "proposed_route": {"type": ["string", "null"]}
                }
            },
            "summary": {
                "type": "object",
                "additionalProperties": false,
                "required": ["facts", "unresolved", "relationship_changes", "knowledge_changes", "interpretation", "address_form_changes", "important_player_actions", "source_history_ids"],
                "properties": {
                    "facts": {"type": "array", "items": {"type": "string"}},
                    "unresolved": {"type": "array", "items": {"type": "string"}},
                    "relationship_changes": {"type": "array", "items": {"type": "string"}},
                    "knowledge_changes": {"type": "array", "items": {"type": "string"}},
                    "interpretation": {"type": ["string", "null"]},
                    "address_form_changes": {"type": "array", "items": {"type": "string"}},
                    "important_player_actions": {"type": "array", "items": {"type": "string"}},
                    // Provenance is advisory metadata. The recovery layer
                    // discards non-UUID values instead of rejecting a usable
                    // scene, so do not force every local server to implement
                    // UUID-pattern constrained decoding.
                    "source_history_ids": {"type": "array", "items": {"type": "string"}}
                }
            }
        }
    })
}

/// Build a [`DirectorRequest`] for the 2-stage path. Mirrors
/// [`build_turn_request`] but targets the non-streaming Director call (lower
/// temperature, smaller budget — a Director plan is short).
fn build_director_request(
    generation_id: Uuid,
    pack: &StoryPack,
    game: &GameState,
    player_action: Option<&ParsedAction>,
) -> DirectorRequest {
    let bundle = build_prompt(pack, game, &game.history, player_action);
    let mut system = bundle.system;
    system.push_str("\n\n# Director stage override\n");
    system.push_str(&pack.prompts.director);
    system.push_str(
        "\nThis request is the Director stage. Return only one DirectorPlan JSON object matching the supplied schema; do not return GeneratedTurn narration.",
    );
    let messages = vec![
        story_model::ChatMessage {
            role: "system".to_string(),
            content: system,
        },
        story_model::ChatMessage {
            role: "user".to_string(),
            content: bundle.user,
        },
    ];
    DirectorRequest {
        generation_id,
        messages,
        model: "mock".to_string(),
        temperature: 0.3,
        top_p: 0.9,
        top_k: 40,
        // The Director plan is a large Korean JSON object (scene_goal, beats,
        // emotion_arcs, outfits, ambient_situation, reveal lists, a patch and
        // 3–4 choices × 7 fields). 1024 tokens truncated it mid-object. The
        // live provider overrides this with the operator's `max_tokens`; this
        // default only binds callers that bypass that override (mock, tests).
        max_tokens: 6144,
        // Guide constrained decoding toward the DirectorPlan shape so the model
        // emits required fields (scene_goal, choices, ...) under the canonical
        // keys. Rust still strict-parses + validates afterward; the provider
        // only attaches this when structured output is enabled.
        json_schema: Some(director_plan_json_schema()),
    }
}

/// JSON Schema for [`story_model::DirectorPlan`], sent to OpenAI-compatible
/// providers that support structured output. Unlike [`generated_turn_json_schema`]
/// this is not declared `strict`: the plan's nested maps (emotion_arcs,
/// character_outfits keyed by dynamic character ids) and many optional fields
/// make full strict mode brittle on local guided-decoding servers. The required
/// set is exactly what makes a plan pass [`crate::director::validate_plan`] —
/// everything else is guidance the model may include.
fn director_plan_json_schema() -> story_model::serde_json::Value {
    story_model::serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["scene_goal", "proposed_patch", "choices", "end_condition"],
        "properties": {
            "scene_goal": {"type": "string"},
            "required_beats": {"type": "array", "items": {"type": "string"}},
            "emotion_arcs": {
                "type": "object",
                "additionalProperties": {
                    "type": "object",
                    "properties": {
                        "character": {"type": "string"},
                        "from": {"type": "string"},
                        "to": {"type": "string"},
                        "reason": {"type": ["string", "null"]}
                    }
                }
            },
            "character_outfits": {
                "type": "object",
                "additionalProperties": {"type": "string"}
            },
            "ambient_situation": {"type": ["string", "null"]},
            "reveal_fact_ids": {"type": "array", "items": {"type": "string"}},
            "must_not_reveal_fact_ids": {"type": "array", "items": {"type": "string"}},
            "proposed_patch": {
                "type": "object",
                "properties": {
                    "affection_delta": {"type": "object", "additionalProperties": {"type": "integer"}},
                    "trust_delta": {"type": "object", "additionalProperties": {"type": "integer"}},
                    "tension_delta": {"type": "object", "additionalProperties": {"type": "integer"}},
                    "relationship_transitions": {"type": "object", "additionalProperties": {"type": "string"}},
                    "set_flags": {"type": "object", "additionalProperties": {"type": "boolean"}},
                    "number_delta": {"type": "object", "additionalProperties": {"type": "integer"}},
                    "set_strings": {"type": "object", "additionalProperties": {"type": "string"}},
                    "inventory_delta": {"type": "object", "additionalProperties": {"type": "integer"}},
                    "add_events": {"type": "array", "items": {"type": "string"}},
                    "next_location": {"type": ["string", "null"]},
                    "next_time_slot": {"type": ["string", "null"]},
                    "proposed_route": {"type": ["string", "null"]}
                }
            },
            "choices": {
                "type": "array",
                "minItems": 1,
                "maxItems": 6,
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "label": {"type": "string"},
                        "intent": {"type": "string"},
                        "tone": {"type": ["string", "null"]},
                        "target": {"type": ["string", "null"]},
                        "risk": {"type": "string"},
                        "hidden_requirements": {"type": "array", "items": {"type": "string"}},
                        "preview": {"type": ["string", "null"]}
                    }
                }
            },
            "conflict_checks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "description": {"type": "string"},
                        "resolved": {"type": "boolean"}
                    }
                }
            },
            "end_condition": {"type": "string"}
        }
    })
}

/// Build a [`WriterRequest`] for the 2-stage path. The Writer is given the
/// same prompt context plus a digest of the validated plan for attestation.
fn build_writer_request(
    generation_id: Uuid,
    pack: &StoryPack,
    game: &GameState,
    player_action: Option<&ParsedAction>,
    plan: &DirectorPlan,
    plan_digest: String,
) -> WriterRequest {
    let bundle = build_prompt(pack, game, &game.history, player_action);
    let mut system = bundle.system;
    system.push_str("\n\n# Writer stage override\n");
    system.push_str(&pack.prompts.writer);
    system.push_str(
        "\nThis is the Writer stage. Follow the validated plan below exactly. Stream only player-visible Korean narration; do not emit a new patch, choices, JSON, or planning text.",
    );
    let mut user = bundle.user;
    user.push_str("\n\n# Validated Director plan (engine-authorized)\n");
    user.push_str(&story_model::serde_json::to_string(plan).unwrap_or_else(|_| "{}".to_string()));
    user.push_str("\nplan_digest: ");
    user.push_str(&plan_digest);
    let messages = vec![
        story_model::ChatMessage {
            role: "system".to_string(),
            content: system,
        },
        story_model::ChatMessage {
            role: "user".to_string(),
            content: user,
        },
    ];
    WriterRequest {
        generation_id,
        messages,
        model: "mock".to_string(),
        temperature: 0.85,
        top_p: 0.92,
        top_k: 40,
        // Rich Korean narration (min_narration_chars is in the thousands) needs
        // well over the old 1800-token cap. The live provider overrides this
        // with the operator's `max_tokens`; this default only binds callers
        // that bypass that override (mock, tests).
        max_tokens: 8192,
        plan_digest,
    }
}

/// Stable lowercase-hex SHA-256 digest of the canonical JSON serialization of
/// `plan`. Stamped onto the [`WriterRequest`] so the host can attest the
/// streamed narration belongs to a confirmed plan. `sha2` is already a
/// `story-core` dependency (used by [`build_prompt`]).
fn plan_digest(plan: &DirectorPlan) -> String {
    let bytes = story_model::serde_json::to_vec(plan).unwrap_or_default();
    let mut hasher = sha2::Sha256::new();
    sha2::Digest::update(&mut hasher, &bytes);
    hex_lower(sha2::Digest::finalize(hasher).as_slice())
}

/// Lowercase-hex encode a byte slice without pulling in a `hex` crate. Kept
/// in sync with the private helper in [`prompt_context`].
fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

// ---------------------------------------------------------------------------
// error mapping
// ---------------------------------------------------------------------------

fn domain_error_view(err: &CoreError) -> DomainErrorView {
    DomainErrorView {
        code: err.reason_code().unwrap_or("core_error").to_string(),
        title_ko: "생성 실패".to_string(),
        message_ko: err.to_string(),
        recoverable: true,
    }
}

fn clone_state(state: &EngineState) -> EngineState {
    EngineState {
        game: state.game.clone(),
        pack_metadata: state.pack_metadata.clone(),
        pending: state.pending.clone(),
        retry_pending: state.retry_pending.clone(),
        lineage: state.lineage.clone(),
        checkpoints: state.checkpoints.clone(),
        audit_tail: state.audit_tail.clone(),
        autosave_on_accept: state.autosave_on_accept,
        use_director_writer: state.use_director_writer,
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_resolved_ending, turn_digest};
    use story_model::{ConditionExpr, EndingDef};

    #[test]
    fn turn_digest_prefers_interpretation_then_falls_back_to_first_clause() {
        assert_eq!(
            turn_digest(
                "비가 그쳤다. 유나가 찻잔을 내려놓는다.",
                Some("유나가 거리를 둔 채 창 밖을 본다")
            ),
            "유나가 거리를 둔 채 창 밖을 본다"
        );
        // No interpretation → narration's first clause.
        assert_eq!(
            turn_digest("비가 그쳤다. 유나가 찻잔을 내려놓는다.", None),
            "비가 그쳤다"
        );
        // Blank interpretation is treated as absent.
        assert_eq!(turn_digest("비가 그쳤다.", Some("   ")), "비가 그쳤다");
    }

    #[test]
    fn resolved_ending_is_appended_to_narration_and_closes_choices() {
        let mut pack = crate::tests_support::sample_pack();
        pack.rules.endings.insert(
            "ending-test".to_string(),
            EndingDef {
                id: "ending-test".to_string(),
                condition: ConditionExpr::All { children: vec![] },
                label: "다시 시작되는 관계".to_string(),
            },
        );
        let mut draft = crate::tests_support::valid_draft(story_model::uuid::Uuid::nil(), 0);
        apply_resolved_ending(
            &mut draft.narration,
            &mut draft.choices,
            Some("ending-test"),
            &pack,
        );
        assert!(draft.narration.contains("[엔딩] 다시 시작되는 관계"));
        assert!(draft.choices.is_empty());
    }
}
