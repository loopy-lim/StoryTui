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
//!    locations). Unknown references are *dropped*, not rejected: the JSON
//!    schema cannot constrain object keys to a pack's id lists, so a stray
//!    key is an expected model slip and must not discard a whole generation.
//!    Each drop is audited as a `Rejected` change-log entry plus a
//!    [`GenerationWarning`].
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
    /// Sanitized patch safe for the reducer to apply: identical to the input
    /// patch minus every unknown-reference entry dropped at stage 2.
    pub sanitized: StatePatch,
    /// Non-fatal issues discovered during validation (one per dropped
    /// reference, plus any future soft checks).
    pub warnings: Vec<GenerationWarning>,
    /// Field-level audit entries (one `Rejected` entry per dropped
    /// reference; the reducer appends its `Applied` entries on apply).
    pub change_log: Vec<StateChangeLog>,
}

/// Validate a patch against the current state and rules.
///
/// Stage 2 drops unknown references (audited — see the module docs); stages
/// 3–5 return `Err([CoreError::InvalidPatch])` on any hard failure, leaving
/// the caller's state untouched (atomicity is enforced by the reducer, which
/// only ever mutates a clone).
pub fn validate(
    state: &GameState,
    rules: &StoryRules,
    patch: &StatePatch,
    generation_id: Uuid,
) -> Result<ValidationOutcome, CoreError> {
    // Stage 2: Reference ----------------------------------------------
    // Unknown references are dropped (with audit), not rejected: one stray
    // key must not discard a whole generation.
    let (sanitized, warnings, change_log) =
        drop_unknown_references(state, rules, patch, generation_id);

    // Stage 3: Range ---------------------------------------------------
    // Registry lookups below rely on the reference stage having removed
    // every unknown id from `sanitized`.
    check_ranges(state, rules, &sanitized)?;

    // Stage 4: Permission ---------------------------------------------
    check_relationship_transitions(state, rules, &sanitized)?;

    // Stage 5: Transition ---------------------------------------------
    check_location_transition(state, rules, &sanitized)?;

    // Stage 6: Cross ---------------------------------------------------
    // Light MVP checks reserved for future expansion; issues here become
    // warnings rather than errors.

    Ok(ValidationOutcome {
        sanitized,
        warnings,
        change_log,
    })
}

// ---------------------------------------------------------------------------
// Stage 2: Reference
// ---------------------------------------------------------------------------

/// Remove every unknown-reference entry from `patch`, returning the
/// sanitized patch plus one warning and one `Rejected` change-log entry per
/// dropped key.
///
/// The response JSON schema cannot constrain object keys to a pack's id
/// lists (any server, schema-enforced or not, accepts arbitrary keys), so a
/// stray key — often empty or a free-text label — is an expected model slip.
/// Rejecting the whole turn threw away a full generation over one key;
/// dropping the entry keeps the turn alive and the audit trail explains it.
fn drop_unknown_references(
    state: &GameState,
    rules: &StoryRules,
    patch: &StatePatch,
    generation_id: Uuid,
) -> (StatePatch, Vec<GenerationWarning>, Vec<StateChangeLog>) {
    let mut sanitized = patch.clone();
    let mut audit = DropAudit::new(generation_id, state.turn);

    drop_unknown_character_keys(
        &mut sanitized.affection_delta,
        state,
        &mut audit,
        "affection_delta",
    );
    drop_unknown_character_keys(&mut sanitized.trust_delta, state, &mut audit, "trust_delta");
    drop_unknown_character_keys(
        &mut sanitized.tension_delta,
        state,
        &mut audit,
        "tension_delta",
    );
    drop_unknown_character_keys(
        &mut sanitized.relationship_transitions,
        state,
        &mut audit,
        "relationship_transitions",
    );
    drop_unknown_registry_keys(
        &mut sanitized.set_flags,
        |flag| rules.allowed_flags.contains(flag),
        &mut audit,
        "set_flags",
        PATCH_UNKNOWN_FLAG,
    );
    drop_unknown_registry_keys(
        &mut sanitized.number_delta,
        |num| rules.numbers.contains_key(num),
        &mut audit,
        "number_delta",
        PATCH_UNKNOWN_NUMBER,
    );
    drop_unknown_registry_keys(
        &mut sanitized.set_strings,
        |s| rules.strings.contains_key(s),
        &mut audit,
        "set_strings",
        PATCH_UNKNOWN_STRING,
    );
    drop_unknown_registry_keys(
        &mut sanitized.inventory_delta,
        |item| rules.allowed_items.contains(item),
        &mut audit,
        "inventory_delta",
        PATCH_UNKNOWN_ITEM,
    );

    let unknown_events: Vec<String> = sanitized
        .add_events
        .iter()
        .filter(|ev| {
            !rules
                .events
                .contains_key(&story_model::EventId((*ev).clone()))
        })
        .cloned()
        .collect();
    for ev in unknown_events {
        audit.record(
            format!("state_patch.add_events.{ev}"),
            story_model::serde_json::Value::String(ev.clone()),
            PATCH_UNKNOWN_EVENT,
            format!("add_events dropped unknown event {ev:?}"),
        );
        sanitized.add_events.remove(&ev);
    }

    if let Some(loc) = sanitized.next_location.clone()
        && !rules.locations.contains_key(&loc)
    {
        audit.record(
            format!("state_patch.next_location.{}", loc),
            story_model::serde_json::Value::String(loc.0.clone()),
            PATCH_UNKNOWN_LOCATION,
            format!("next_location dropped unknown location {loc:?}"),
        );
        sanitized.next_location = None;
    }

    (sanitized, audit.warnings, audit.entries)
}

/// Per-call audit sink for stage-2 drops: one [`GenerationWarning`] and one
/// `Rejected` [`StateChangeLog`] entry per dropped key.
struct DropAudit {
    generation_id: Uuid,
    turn: u32,
    warnings: Vec<GenerationWarning>,
    entries: Vec<StateChangeLog>,
}

impl DropAudit {
    fn new(generation_id: Uuid, turn: u32) -> Self {
        Self {
            generation_id,
            turn,
            warnings: Vec::new(),
            entries: Vec::new(),
        }
    }

    fn record(
        &mut self,
        field_path: String,
        proposed: story_model::serde_json::Value,
        code: &'static str,
        detail: String,
    ) {
        self.warnings.push(GenerationWarning {
            code: code.to_string(),
            detail: detail.clone(),
        });
        self.entries.push(StateChangeLog {
            id: Uuid::new_v4(),
            generation_id: self.generation_id,
            turn: self.turn,
            source: story_model::ChangeSource::LlmPatch,
            field_path,
            before: story_model::serde_json::Value::Null,
            proposed,
            applied: None,
            decision: story_model::ChangeDecision::Rejected,
            rule_id: None,
            reason_code: Some(code.to_string()),
        });
    }
}

fn drop_unknown_character_keys<V: serde::Serialize>(
    map: &mut std::collections::BTreeMap<CharacterId, V>,
    state: &GameState,
    audit: &mut DropAudit,
    field: &'static str,
) {
    let unknown: Vec<CharacterId> = map
        .keys()
        .filter(|ch| !state.characters.contains_key(*ch))
        .cloned()
        .collect();
    for ch in unknown {
        let proposed = story_model::serde_json::to_value(map.get(&ch))
            .unwrap_or(story_model::serde_json::Value::Null);
        audit.record(
            format!("state_patch.{field}.{ch}"),
            proposed,
            PATCH_UNKNOWN_CHARACTER,
            format!("{field} dropped unknown character {ch:?}"),
        );
        map.remove(&ch);
    }
}

fn drop_unknown_registry_keys<V: serde::Serialize>(
    map: &mut std::collections::BTreeMap<String, V>,
    known: impl Fn(&str) -> bool,
    audit: &mut DropAudit,
    field: &'static str,
    code: &'static str,
) {
    let unknown: Vec<String> = map.keys().filter(|key| !known(key)).cloned().collect();
    for key in unknown {
        let proposed = story_model::serde_json::to_value(map.get(&key))
            .unwrap_or(story_model::serde_json::Value::Null);
        audit.record(
            format!("state_patch.{field}.{key}"),
            proposed,
            code,
            format!("{field} dropped unknown variable {key:?}"),
        );
        map.remove(&key);
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
