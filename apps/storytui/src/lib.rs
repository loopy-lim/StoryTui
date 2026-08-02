//! `storytui` — the StoryTUI binary entrypoint crate.
//!
//! Library surface (this file) exposes the testable, host-loop-independent
//! modules: [`diagnostic`] (ST-035) and [`bench`] (ST-036). The interactive
//! host loop is intentionally not wired here; see
//! `docs/KNOWN-LIMITATIONS.md`.

#![warn(clippy::all)]

pub mod bench;
pub mod config;
pub mod diagnostic;
