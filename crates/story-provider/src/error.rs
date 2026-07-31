//! Provider error handling.
//!
//! `story-model` owns the [`ProviderError`] type so it can cross the
//! `story-provider` <-> `story-core` boundary as pure data. This module simply
//! re-exports it so provider code can write `crate::error::ProviderError` and
//! downstream consumers have a single, stable path.

pub use story_model::ProviderError;
