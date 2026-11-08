//! `storytui` — the StoryTUI binary entrypoint crate.
//!
//! Library surface (this file) exposes the testable modules: [`diagnostic`]
//! (ST-035), [`bench`] (ST-036), and [`host`] — the host automation surface
//! (`TurnAutomation`, message/frame handlers, resume body) that the
//! `apps/storytui/tests` integration tests drive. The terminal lifecycle
//! itself stays behind `host::run`.

#![warn(clippy::all)]

// The host modules predate the lib target and refer to sibling modules by
// crate name (`storytui::config`); the self-alias keeps those paths valid in
// both the lib and (via `use storytui::host`) the binary.
extern crate self as storytui;

pub mod bench;
pub mod config;
pub mod diagnostic;
pub mod host;
