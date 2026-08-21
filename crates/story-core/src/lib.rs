#![warn(clippy::all)]
//! Deterministic story core: reducer, validators, rules engine, condition DSL,
//! memory selector, and prompt context. Depends only on [`story-model`] and must
//! never depend on terminal, HTTP, or filesystem crates (ADR-003).

pub mod action_parse;
pub mod choice_quality;
pub mod director;
pub mod engine;
pub mod engine_cmd;
pub mod error;
pub mod eval;
pub mod memory;
pub mod memory_selector;
pub mod narration_quality;
pub mod narrative;
pub mod prompt_context;
pub mod redact;
pub mod reducer;
pub mod state_machine;
pub mod summary;
pub mod tests_support;
pub mod validator;
pub mod writer;
