//! Pack schema version compat (W4, ST-014).
//!
//! Full in-memory migration is implemented in W4.4; this module currently
//! exposes a no-op compat hook so the loader can compile end-to-end before
//! the migration logic lands.

use crate::error::PackError;
use crate::pack::dto::StoryToml;

/// Schema-compatibility hook called by the loader after parsing
/// `story.toml`. W4.4 fills in actual major-version comparison; for now it
/// is a no-op so the loader pipeline can be wired up.
pub fn check_schema_compat(_dto: &StoryToml, _errs: &mut Vec<PackError>) {}
