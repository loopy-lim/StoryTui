#![warn(clippy::all)]
//! Deterministic story core: reducer, validators, rules engine, condition DSL,
//! memory selector, and prompt context. Depends only on [`story-model`] and must
//! never depend on terminal, HTTP, or filesystem crates (ADR-003).

pub mod engine;
pub mod error;
pub mod eval;
pub mod reducer;
pub mod state_machine;
pub mod validator;
