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
use story_model::{AppViewModel, DomainErrorView, GeneratedTurnDraft, TurnRequest};

/// A user / host intent handed to the engine. The engine validates each
/// command against its current state and returns a [`crate::engine::Transition`].
#[derive(Clone, Debug)]
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
    /// A generation completed. The draft's `generation_id` must match the
    /// pending one; a mismatch (or no pending generation) is a late/stale
    /// response and is silently discarded with no state change.
    AcceptGeneration {
        /// The draft returned by the provider (boxed to keep the enum cheap;
        /// `GeneratedTurnDraft` carries narration + choices + a patch).
        draft: Box<GeneratedTurnDraft>,
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
#[derive(Clone, Debug)]
pub enum EngineEffect {
    /// Call the provider with `request`. The host feeds the result back as
    /// [`EngineCommand::AcceptGeneration`].
    RequestGeneration {
        /// The pending generation id this request belongs to.
        generation_id: Uuid,
        /// The assembled prompt + sampling parameters.
        request: TurnRequest,
    },
    /// Render this view. The latest published view is what the player sees.
    /// (Boxed: `AppViewModel` carries narration + choices + status, large
    /// enough to dominate the enum discriminant.)
    PublishView(Box<AppViewModel>),
    /// Surface a recoverable error. Always co-emitted with a [`PublishView`]
    /// showing the error overlay so the view stays self-contained.
    ShowRecoverableError(DomainErrorView),
}
