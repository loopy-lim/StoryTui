//! Pack validation: referential integrity, reachability, teen metadata
//! (W4, ST-014).
//!
//! Full per-check implementations land in W4.5; this module currently
//! exposes a single aggregation hook so the loader pipeline can run
//! end-to-end before the validators are filled in.

use crate::error::PackError;
use story_model::StoryPack;

/// Run every validation pass against `pack`, appending any errors to
/// `errs`. W4.5 fills in the actual checks; for now this is a no-op.
pub fn validate_all(_pack: &StoryPack, _errs: &mut Vec<PackError>) {}
