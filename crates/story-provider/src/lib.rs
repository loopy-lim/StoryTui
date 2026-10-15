#![warn(clippy::all)]
//! LLM provider abstraction for StoryTUI: the trait surface and a local MLX
//! HTTP client implementation. Owns all network interaction (`reqwest`) so that
//! [`story-core`] and [`story-model`] remain IO-free. Streaming, retry, and
//! prompt scaffolding live here.
//!
//! # Tranche 1 scope (W5.1–W5.4)
//! - [`LlmProvider`] trait + [`MockProvider`] (no network).
//! - recovery parser (W5.3): strict `GeneratedTurn` JSON parse + limited repair.
//! - [`envelope`] is a stub — the real OpenAI HTTP client lands in W6.

pub mod config;
pub mod envelope;
pub mod error;
pub mod mock;
pub mod openai_compatible;
pub mod provider;
pub mod recovery;
pub mod sse;

pub use config::{MaxTokensField, ProviderConfig, StructuredOutputMode};
pub use mock::{MockProvider, MockResponse};
pub use openai_compatible::{OpenAiCompatibleProvider, is_loopback_base_url};
pub use provider::LlmProvider;
pub use recovery::{ParseOutcome, parse_generated_choices, parse_generated_turn};
pub use sse::{SseEvent, SseParser};

// Convenient re-export of the boundary DTOs / error for downstream crates.
pub use story_model::{
    ChatMessage, GeneratedTurnDraft, ProviderCapabilities, ProviderError, ProviderHealth,
    TurnRequest,
};
