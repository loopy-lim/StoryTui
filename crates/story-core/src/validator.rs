//! Eight-stage [`StatePatch`] validation pipeline (ST-007).
//!
//! [`validate`] is the *only* entry point that converts a raw model-emitted
//! patch into a sanitized patch the reducer may apply. It is total and
//! deterministic: on any hard failure it returns [`CoreError`] and the caller's
//! `GameState` is never mutated. Cosmetic-only issues are surfaced as
//! [`GenerationWarning`]s and do not reject the patch.
//!
//! # Stages
//!
//! 1. **Structural** — handled at parse time (`deny_unknown_fields` on
//!    [`StatePatch`]); nothing to do here at runtime.
//! 2. **Reference** — every id in the patch must exist in either the live
//!    state (characters) or the rules (flags/numbers/strings/items/events/
//!    locations).
//! 3. **Range** — single-turn deltas within `delta_limits`; projected final
//!    stats within `stat_limits` (no automatic clamping); number deltas within
//!    the var's `[min, max]`; strings ≤ `max_len` and in `allowed`; projected
//!    inventory ≥ 0.
//! 4. **Permission** — relationship transitions limited to single-step moves
//!    unless an `allow_relationship_skip` event is present in `add_events`.
//! 5. **Transition** — `next_location` must be adjacent to the current
//!    location (an event in `add_events` with matching `force_location`
//!    exempts the check, since the reducer will apply the override).
//! 6. **Cross** — light internal-consistency checks; issues become warnings,
//!    not errors (MVP).
//!
//! Stages 7 (dry-run) and 8 (commit) live in the reducer, which calls
//! [`validate`] first.

use story_model::uuid::Uuid;

use story_model::{
    CharacterId, CharacterState, GameState, GenerationWarning, Int16Range, StateChangeLog,
    StatePatch, StoryRules,
};

use crate::error::{
    CoreError, PATCH_DELTA_OUT_OF_RANGE, PATCH_NEGATIVE_INVENTORY, PATCH_NONADJACENT_LOCATION,
    PATCH_NUMBER_OUT_OF_RANGE, PATCH_STAT_OUT_OF_RANGE, PATCH_STRING_VIOLATION,
    PATCH_UNKNOWN_CHARACTER, PATCH_UNKNOWN_EVENT, PATCH_UNKNOWN_FLAG, PATCH_UNKNOWN_ITEM,
    PATCH_UNKNOWN_LOCATION, PATCH_UNKNOWN_NUMBER, PATCH_UNKNOWN_STRING,
};
use crate::state_machine::would_permit_relationship_transition;

/// Output of a successful validation.
#[derive(Clone, Debug)]
pub struct ValidationOutcome {
    /// Sanitized patch safe for the reducer to apply. With the strict
    /// reject-the-whole-patch policy, this equals the input patch on success.
    pub sanitized: StatePatch,
    /// Non-fatal issues discovered during validation.
    pub warnings: Vec<GenerationWarning>,
    /// Field-level audit entries (mostly populated by the reducer on apply;
    /// the validator only adds Rejected entries it could not surface as
    /// warnings — empty under the strict policy).
    pub change_log: Vec<StateChangeLog>,
}

/// Validate a patch against the current state and rules.
///
/// Returns `Err([CoreError::InvalidPatch])` on any hard failure, leaving the
/// caller's state untouched (atomicity is enforced by the reducer, which only
/// ever mutates a clone).
pub fn validate(
    state: &GameState,
    rules: &StoryRules,
    patch: &StatePatch,
    _generation_id: Uuid,
) -> Result<ValidationOutcome, CoreError> {
    // Stage 2: Reference ----------------------------------------------
    check_references(state, rules, patch)?;

    // Stage 3: Range ---------------------------------------------------
    check_ranges(state, rules, patch)?;

    // Stage 4: Permission ---------------------------------------------
    check_relationship_transitions(state, rules, patch)?;

    // Stage 5: Transition ---------------------------------------------
    check_location_transition(state, rules, patch)?;

    // Stage 6: Cross ---------------------------------------------------
    // Light MVP checks reserved for future expansion; issues here become
    // warnings rather than errors.

    Ok(ValidationOutcome {
        sanitized: patch.clone(),
        warnings: Vec::new(),
        change_log: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Stage 2: Reference
// ---------------------------------------------------------------------------

fn check_references(
    state: &GameState,
    rules: &StoryRules,
    patch: &StatePatch,
) -> Result<(), CoreError> {
    for ch in patch.affection_delta.keys() {
        require_character(state, ch, "affection_delta")?;
    }
    for ch in patch.trust_delta.keys() {
        require_character(state, ch, "trust_delta")?;
    }
    for ch in patch.tension_delta.keys() {
        require_character(state, ch, "tension_delta")?;
    }
    for ch in patch.relationship_transitions.keys() {
        require_character(state, ch, "relationship_transitions")?;
    }
    for flag in patch.set_flags.keys() {
        if !rules.allowed_flags.contains(flag) {
            return Err(CoreError::invalid_patch(
                PATCH_UNKNOWN_FLAG,
                format!("set_flags references unknown flag {flag}"),
            ));
        }
    }
    for num in patch.number_delta.keys() {
        if !rules.numbers.contains_key(num) {
            return Err(CoreError::invalid_patch(
                PATCH_UNKNOWN_NUMBER,
                format!("number_delta references unknown number variable {num}"),
            ));
        }
    }
    for s in patch.set_strings.keys() {
        if !rules.strings.contains_key(s) {
            return Err(CoreError::invalid_patch(
                PATCH_UNKNOWN_STRING,
                format!("set_strings references unknown string variable {s}"),
            ));
        }
    }
    for item in patch.inventory_delta.keys() {
        if !rules.allowed_items.contains(item) {
            return Err(CoreError::invalid_patch(
                PATCH_UNKNOWN_ITEM,
                format!("inventory_delta references unknown item {item}"),
            ));
        }
    }
    for ev in &patch.add_events {
        if !rules.events.contains_key(&story_model::EventId(ev.clone())) {
            return Err(CoreError::invalid_patch(
                PATCH_UNKNOWN_EVENT,
                format!("add_events references unknown event {ev}"),
            ));
        }
    }
    if let Some(loc) = &patch.next_location
        && !rules.locations.contains_key(loc)
    {
        return Err(CoreError::invalid_patch(
            PATCH_UNKNOWN_LOCATION,
            format!("next_location references unknown location {loc}"),
        ));
    }
    Ok(())
}

fn require_character(
    state: &GameState,
    ch: &CharacterId,
    field: &'static str,
) -> Result<(), CoreError> {
    if state.characters.contains_key(ch) {
        Ok(())
    } else {
        Err(CoreError::invalid_patch(
            PATCH_UNKNOWN_CHARACTER,
            format!("{field} references unknown character {ch}"),
        ))
    }
}

// ---------------------------------------------------------------------------
// Stage 3: Range
// ---------------------------------------------------------------------------

fn check_ranges(
    state: &GameState,
    rules: &StoryRules,
    patch: &StatePatch,
) -> Result<(), CoreError> {
    check_axis_delta(
        &patch.affection_delta,
        state,
        |c| c.affection,
        &rules.delta_limits.affection,
        rules.affection_stat_limits(),
        "affection",
    )?;
    check_axis_delta(
        &patch.trust_delta,
        state,
        |c| c.trust,
        &rules.delta_limits.trust,
        rules.trust_stat_limits(),
        "trust",
    )?;
    check_axis_delta(
        &patch.tension_delta,
        state,
        |c| c.tension,
        &rules.delta_limits.tension,
        rules.tension_stat_limits(),
        "tension",
    )?;

    // Number variable deltas: must keep projected value within var [min, max].
    for (id, delta) in &patch.number_delta {
        let def = rules.numbers.get(id).expect("checked at reference stage");
        let current = state.numbers.get(id).copied().unwrap_or(def.default);
        let projected = current.saturating_add(*delta);
        if projected < def.min || projected > def.max {
            return Err(CoreError::invalid_patch(
                PATCH_NUMBER_OUT_OF_RANGE,
                format!(
                    "number {id} projected to {projected} outside [{}, {}]",
                    def.min, def.max
                ),
            ));
        }
    }

    // Strings: length ≤ max_len; in allowed if set.
    for (id, value) in &patch.set_strings {
        let def = rules.strings.get(id).expect("checked at reference stage");
        if value.len() as u32 > def.max_len {
            return Err(CoreError::invalid_patch(
                PATCH_STRING_VIOLATION,
                format!(
                    "string {id} length {} exceeds max_len {}",
                    value.len(),
                    def.max_len
                ),
            ));
        }
        if let Some(allowed) = &def.allowed
            && !allowed.contains(value)
        {
            return Err(CoreError::invalid_patch(
                PATCH_STRING_VIOLATION,
                format!("string {id} value {value:?} not in allowed set"),
            ));
        }
    }

    // Inventory: projected quantity ≥ 0.
    for (id, delta) in &patch.inventory_delta {
        let current = state.inventory.get(id).copied().unwrap_or(0) as i64;
        let projected = current + *delta as i64;
        if projected < 0 {
            return Err(CoreError::invalid_patch(
                PATCH_NEGATIVE_INVENTORY,
                format!("inventory {id} projected to {projected} (< 0)"),
            ));
        }
    }

    Ok(())
}

/// Check delta-magnitude + projected-final-stat bounds for a single axis
/// (affection/trust/tension).
fn check_axis_delta(
    deltas: &std::collections::BTreeMap<CharacterId, i16>,
    state: &GameState,
    read_current: impl Fn(&CharacterState) -> i16,
    delta_range: &Int16Range,
    stat_range: &Int16Range,
    axis_name: &str,
) -> Result<(), CoreError> {
    for (ch, delta) in deltas {
        if !delta_range.contains(*delta) {
            return Err(CoreError::invalid_patch(
                PATCH_DELTA_OUT_OF_RANGE,
                format!(
                    "{axis_name}_delta for {ch} is {delta}, outside [{}, {}]",
                    delta_range.min, delta_range.max
                ),
            ));
        }
        let current = state
            .characters
            .get(ch)
            .map(&read_current)
            .expect("checked at reference stage");
        let projected = current.saturating_add(*delta);
        if !stat_range.contains(projected) {
            return Err(CoreError::invalid_patch(
                PATCH_STAT_OUT_OF_RANGE,
                format!(
                    "{axis_name} for {ch} projected to {projected}, outside [{}, {}]",
                    stat_range.min, stat_range.max
                ),
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Stage 4: Permission (relationship transitions)
// ---------------------------------------------------------------------------

fn check_relationship_transitions(
    state: &GameState,
    rules: &StoryRules,
    patch: &StatePatch,
) -> Result<(), CoreError> {
    // Does any event in add_events grant allow_relationship_skip?
    let any_skip_candidate = patch.add_events.iter().any(|ev_id| {
        rules
            .events
            .get(&story_model::EventId(ev_id.clone()))
            .is_some_and(|d| d.allow_relationship_skip)
    });

    for (ch, target) in &patch.relationship_transitions {
        let current = state
            .characters
            .get(ch)
            .map(|c| c.relationship_stage)
            .expect("checked at reference stage");
        would_permit_relationship_transition(current, *target, any_skip_candidate)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Stage 5: Transition (location adjacency)
// ---------------------------------------------------------------------------

fn check_location_transition(
    state: &GameState,
    rules: &StoryRules,
    patch: &StatePatch,
) -> Result<(), CoreError> {
    let Some(next) = &patch.next_location else {
        return Ok(());
    };

    // Staying in the current location is a valid no-op. Location adjacency
    // describes movement between distinct locations and usually does not list
    // a location as adjacent to itself.
    if next == &state.location {
        return Ok(());
    }

    // Exempt: an event in add_events whose force_location equals `next`.
    let event_forces_next = patch.add_events.iter().any(|ev_id| {
        rules
            .events
            .get(&story_model::EventId(ev_id.clone()))
            .and_then(|d| d.effects.force_location.as_ref())
            .is_some_and(|forced| forced == next)
    });
    if event_forces_next {
        return Ok(());
    }

    let adjacent = rules
        .locations
        .get(&state.location)
        .map(|def| def.adjacent.contains(next))
        .unwrap_or(false);
    if adjacent {
        Ok(())
    } else {
        Err(CoreError::invalid_patch(
            PATCH_NONADJACENT_LOCATION,
            format!(
                "next_location {next} is not adjacent to current location {}",
                state.location
            ),
        ))
    }
}
