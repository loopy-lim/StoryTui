//! Engine command / effect boundary types (ST-020).
//!
//! These are the Elm-style messages that flow into [`crate::engine::reduce`]
//! ([`EngineCommand`]) and the side-effect descriptions that flow out
//! ([`EngineEffect`]). The engine is pure: it **describes** work (request a
//! generation, publish a view, surface an error) but performs no HTTP / FS /
//! UI itself (ADR-003). The host app (`apps/storytui`) interprets the effects.
//!
//! See `engine.rs` for [`crate::engine::EngineState`], [`crate::engine::Transition`],
//! and the [`crate::engine::reduce`] function.

use story_model::uuid::Uuid;
use story_model::{
    AppViewModel, DirectorPlan, DirectorRequest, DomainErrorView, GeneratedTurnDraft, ParsedAction,
    TurnRequest, WriterFinish, WriterRequest,
};

/// A user / host intent handed to the engine. The engine validates each
/// command against its current state and returns a [`crate::engine::Transition`].
#[derive(Clone, Debug, PartialEq)]
pub enum EngineCommand {
    /// Initialize a fresh game from the pack and request the opening turn's
    /// choices. Idempotent over the pack: rebuilds `game` from
    /// `pack.metadata.initial_state` + characters.
    StartNewGame,
    /// The player selected a choice. `choice_id` must match one of the current
    /// view's choices; rejected if a generation is already in flight.
    SubmitChoice {
        /// Id of the chosen [`story_model::Choice`].
        choice_id: String,
    },
    /// The player typed free text (parsed into a [`ParsedAction`] by the
    /// rule-based parser). Behaves like [`SubmitChoice`]: requires no pending
    /// generation, snapshots a checkpoint, and requests a generation whose
    /// prompt carries the parsed action + the player-sovereignty clause.
    /// `action` is boxed to keep this variant cheap relative to
    /// [`EngineCommand::AcceptGeneration`].
    SubmitFreeAction {
        /// The parsed free-text action. `original_text` is forwarded verbatim
        /// into the prompt; the engine never paraphrases it.
        action: Box<ParsedAction>,
    },
    /// Re-roll the in-flight generation for the current turn: cancels the
    /// pending generation and re-requests from the SAME checkpoint with a fresh
    /// `generation_id` (recorded in the turn's lineage). The previously
    /// committed turn is NOT touched. No-op when no generation is pending.
    RegenerateCurrentTurn,
    /// Internal automatic repair: regenerate only the currently offered
    /// choices. The accepted narration and state patch remain unchanged.
    /// This is not a player-facing control.
    RegenerateChoices,
    /// Restore a prior committed-state checkpoint (kept in the engine's
    /// in-memory checkpoint ring). Clears any pending generation. No-op when
    /// `checkpoint_id` does not match a stored checkpoint.
    RestoreCheckpoint {
        /// Id of the checkpoint to restore (one of `EngineState::checkpoints`).
        checkpoint_id: Uuid,
    },
    /// A generation completed. The draft's `generation_id` must match the
    /// pending one; a mismatch (or no pending generation) is a late/stale
    /// response and is silently discarded with no state change.
    AcceptGeneration {
        /// The draft returned by the provider (boxed to keep the enum cheap;
        /// `GeneratedTurnDraft` carries narration + choices + a patch).
        draft: Box<GeneratedTurnDraft>,
    },
    /// A provider request failed after the engine emitted a generation effect.
    /// The host supplies a sanitized, player-facing error; the engine restores
    /// the pre-generation checkpoint and publishes it without mutating history.
    ProviderFailed {
        generation_id: Uuid,
        error: DomainErrorView,
    },
    /// (W10 2-stage path, ST-033) Director generation completed. The engine
    /// validates + dry-runs `plan`; on rejection the Writer is NEVER called
    /// (the host sees [`crate::engine_cmd::EngineEffect::ShowRecoverableError`]).
    /// `generation_id` must match the pending one; a mismatch is silently
    /// discarded (late-discard semantics identical to [`Self::AcceptGeneration`]).
    /// No-op when the engine is on the MVP single-call path.
    AcceptDirectorPlan {
        /// The pending generation id this Director plan belongs to.
        generation_id: Uuid,
        /// The Director's structural plan (boxed to keep the enum cheap).
        plan: Box<DirectorPlan>,
    },
    /// (W10 2-stage path, ST-034) Writer stream completed. `finish.full_text`
    /// is the finalized narration (the engine runs the lint pass; partial /
    /// pre-`[DONE]` text never reaches this command). The engine commits the
    /// previously-validated Director plan's patch + choices with the Writer's
    /// narration. Late-discard on `generation_id` mismatch; no-op on the MVP
    /// single-call path.
    AcceptWriterFinish {
        /// The aggregated Writer finish returned by the provider.
        finish: Box<WriterFinish>,
    },
    /// Cancel an in-flight generation. Restores the checkpoint if the id
    /// matches; otherwise a no-op.
    CancelGeneration {
        /// Id of the generation to cancel.
        generation_id: Uuid,
    },
    /// Dismiss the current error overlay.
    DismissError,
    /// The host is quitting. The engine returns the current state unchanged;
    /// terminal teardown is the host's responsibility.
    Quit,
}

/// A side-effect the engine *describes* but does not execute. The host app
/// interprets this list (e.g. calls the provider for [`RequestGeneration`],
/// hands [`PublishView`] to the TUI renderer).
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEffect {
    /// Call the provider with `request`. The host feeds the result back as
    /// [`EngineCommand::AcceptGeneration`].
    RequestGeneration {
        /// The pending generation id this request belongs to.
        generation_id: Uuid,
        /// The assembled prompt + sampling parameters.
        request: TurnRequest,
    },
    /// (W10 2-stage path, ST-033) Call the provider's Director path. The host
    /// invokes `provider.direct(request, cancel)` and feeds the result back as
    /// [`EngineCommand::AcceptDirectorPlan`]. Only emitted when
    /// [`crate::engine::EngineState::use_director_writer`] is `true`.
    RequestDirector {
        /// The pending generation id this request belongs to.
        generation_id: Uuid,
        /// The assembled Director prompt + sampling parameters.
        request: DirectorRequest,
    },
    /// (W10 2-stage path, ST-034) Call the provider's Writer streaming path.
    /// The host invokes `provider.write_stream(request, sink, cancel)`, drives
    /// [`story_model::WriterEvent::Chunk`]s to the renderer, and feeds the
    /// final aggregated text back as [`EngineCommand::AcceptWriterFinish`].
    RequestWriter {
        /// The pending generation id this request belongs to.
        generation_id: Uuid,
        /// The assembled Writer prompt + sampling parameters.
        request: WriterRequest,
    },
    /// Render this view. The latest published view is what the player sees.
    /// (Boxed: `AppViewModel` carries narration + choices + status, large
    /// enough to dominate the enum discriminant.)
    PublishView(Box<AppViewModel>),
    /// Surface a recoverable error. Always co-emitted with a [`PublishView`]
    /// showing the error overlay so the view stays self-contained.
    ShowRecoverableError(DomainErrorView),
    /// Persist the engine's stable state to `slot_id`. Emitted after each
    /// accepted turn when `EngineState::autosave_on_accept` is enabled
    /// (default). The host executes this by calling
    /// `SaveStore::save(state.snapshot_payload().into_envelope(...))`. The
    /// engine NEVER touches the filesystem itself (ADR-003).
    PersistStableState {
        /// Slot id to write (default `"autosave"`).
        slot_id: String,
        /// Player-facing description (e.g. `"턴 3 — 카페 나무"`).
        description: Option<String>,
    },
}
