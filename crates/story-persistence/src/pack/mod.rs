//! Story Pack loading, validation, and migration (W4).
//!
//! This module orchestrates reading a Pack directory, parsing TOML/Markdown
//! into the DTOs in [`dto`], running safety checks (path / size / schema
//! version), validating referential integrity and reachability, and producing
//! a canonical [`story_model::StoryPack`].
//!
//! The loader executes **no** arbitrary code (ADR-007) — only TOML +
//! Markdown parsing.

pub mod dto;
pub mod loader;
pub mod migrate;
pub mod validate;

pub use loader::{PackLoader, REQUIRED_FILES};
pub use migrate::{check_schema_compat, migrate_in_memory};
pub use validate::validate_all;
