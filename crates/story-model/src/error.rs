//! Error types for the core engine.
//!
//! Minimal skeleton fleshed out in W3 (validator/reducer) and W5 (providers).

/// Top-level error enum for story-core operations. Re-exported by `story-model`
/// for downstream crates that consume model data and need a shared error type.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// A proposed [`crate::StatePatch`] failed validation.
    #[error("invalid patch: {0}")]
    InvalidPatch(String),
    /// A story rule (constraint) was violated by a patch or action.
    #[error("rule violation: {0}")]
    RuleViolation(String),
    /// A serde_json (de)serialization failure.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
