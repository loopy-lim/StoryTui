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
    AppViewModel, CharacterId, CharacterState, ChoiceRef, DirectorPlan, DirectorRequest,
    DomainErrorView, Emotion, GameState, HistoryEntry, LoadingState, PackMetadata, ParsedAction,
    SavePayload, Screen, StateChangeLog, StatePatch, StoryPack, StoryRules, TurnRequest,
    WriterFinish, WriterRequest,
};

use crate::director::{ValidatedPlan, validate_plan};
use crate::engine_cmd::{EngineCommand, EngineEffect};
use crate::error::{
    CoreError, ENGINE_GENERATION_IN_FLIGHT, ENGINE_NO_CHOICES, ENGINE_UNKNOWN_CHOICE,
};
use crate::prompt_context::build_prompt;
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
        EngineCommand::RestoreCheckpoint { checkpoint_id } => {
            Ok(reduce_restore_checkpoint(state, pack, checkpoint_id))
        }
        EngineCommand::CancelGeneration { generation_id } => {
            Ok(reduce_cancel(state, pack, generation_id))
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
    };
    let request = build_turn_request(generation_id, pack, &game, None);
    let mut next = clone_state(state);
    next.game = game;
    next.pack_metadata = pack.metadata.clone();
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
    };
    let request = build_turn_request(generation_id, pack, &state.game, None);

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
    };
    let request = build_turn_request(generation_id, pack, &state.game, Some(&action));

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
    };
    let request = build_turn_request(
        generation_id,
        pack,
        &pending.checkpoint,
        pending.player_action.as_ref(),
    );

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

    // Matching generation: apply the proposed patch against the checkpoint.
    match apply_patch(
        &pending.checkpoint,
        &pack.rules,
        &draft.proposed_patch,
        draft.generation_id,
    ) {
        Ok(result) => {
            let (next, audit) = assemble_accepted_turn(state, pack, pending, draft.clone(), result);
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

    let entry = HistoryEntry {
        id: history_id,
        turn: game.turn,
        scene_id: game.current_scene_id.clone(),
        created_at: Utc::now(),
        narration: writer.narration.clone(),
        player_action: pending.player_action.clone(),
        choices: validated.plan.choices.clone(),
        applied_patch: validated.plan.proposed_patch.clone(),
        generation_id: pending.generation_id,
    };
    game.history.push(entry);
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
///   narration). `player_action` is populated from the pending generation when
///   the turn originated from a free action; otherwise `None`. The chosen id
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

    let mut game = result.state;
    game.turn = pending.requested_at_turn + 1;

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

    let entry = HistoryEntry {
        id: history_id,
        turn: game.turn,
        scene_id: game.current_scene_id.clone(),
        created_at: Utc::now(),
        narration: draft.narration.clone(),
        player_action: pending.player_action.clone(),
        choices: draft.choices.clone(),
        applied_patch: draft.proposed_patch.clone(),
        generation_id: draft.generation_id,
    };
    game.history.push(entry);
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
/// zeroed affection/trust/tension, the pack-declared initial relationship
/// stage, and a `Neutral` emotion.
fn build_initial_game_state(pack: &StoryPack) -> GameState {
    let init = &pack.metadata.initial_state;
    let characters = pack
        .characters
        .iter()
        .map(|(id, def)| {
            (
                id.clone(),
                CharacterState {
                    affection: 0,
                    trust: 0,
                    tension: 0,
                    relationship_stage: def.initial_player_relationship,
                    current_emotion: Emotion::Neutral,
                    known_facts: Default::default(),
                    address_form_overrides: Default::default(),
                },
            )
        })
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
        flags: Default::default(),
        numbers: Default::default(),
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
) -> TurnRequest {
    let bundle = build_prompt(pack, game, &game.history, player_action);
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
                "items": {"type": "object", "additionalProperties": true}
            },
            "state_patch": {"type": "object", "additionalProperties": true},
            "summary": {"type": "object", "additionalProperties": true}
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
    DirectorRequest {
        generation_id,
        messages,
        model: "mock".to_string(),
        temperature: 0.7,
        top_p: 0.9,
        top_k: 40,
        max_tokens: 1024,
    }
}

/// Build a [`WriterRequest`] for the 2-stage path. The Writer is given the
/// same prompt context plus a digest of the validated plan for attestation.
fn build_writer_request(
    generation_id: Uuid,
    pack: &StoryPack,
    game: &GameState,
    player_action: Option<&ParsedAction>,
    plan_digest: String,
) -> WriterRequest {
    let bundle = build_prompt(pack, game, &game.history, player_action);
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
    WriterRequest {
        generation_id,
        messages,
        model: "mock".to_string(),
        temperature: 0.85,
        top_p: 0.92,
        top_k: 40,
        max_tokens: 1800,
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
