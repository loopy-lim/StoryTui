#![warn(clippy::all)]
//! Deterministic story core: reducer, validators, rules engine, condition DSL,
//! memory selector, and prompt context. Depends only on [`story-model`] and must
//! never depend on terminal, HTTP, or filesystem crates (ADR-003).

pub mod action_parse;
pub mod engine;
pub mod engine_cmd;
pub mod error;
pub mod eval;
pub mod prompt_context;
pub mod reducer;
pub mod state_machine;
pub mod validator;
