//! Atomic patch application (ST-008).
//!
//! [`apply_patch`] is the *only* path by which a validated [`StatePatch`] (plus
//! any events that fire as a consequence) mutates the engine's
//! [`GameState`]. The algorithm is:
//!
//! 1. Run the 8-stage validator. On any hard failure, return [`CoreError`];
//!    the caller's original state is never touched.
//! 2. Clone the state and apply the sanitized deltas/maps to the clone.
//! 3. Compute the set of `add_events` events whose `trigger` evaluates `true`
//!    against the post-patch clone (and whose `route_lock`, if any, matches).
//! 4. Apply `relationship_transitions` via [`resolve_relationship_transition`],
//!    using the *fired* event set to authorize any skip.
//! 5. Fire the matching events in `(priority desc, id asc)` order, applying
//!    each [`EventEffects`].
//! 6. Resolve `ending` ([`resolve_ending`]) and `active_route`
//!    ([`confirm_route`]) from the post-events state. Neither is ever read
//!    from the patch.
//! 7. Bump `revision` by exactly one. `turn` is **not** bumped here — that is
//!    the engine layer's responsibility.
//! 8. Build [`StateChangeLog`] entries for every changed field. Narration
//!    text is never placed in a change-log entry.
//!
//! Atomicity: on any error after cloning, return `Err`. The original state is
//! never mutated because we only ever mutate the clone.

use std::collections::{BTreeMap, BTreeSet};

use story_model::serde_json;
use story_model::uuid::Uuid;
use story_model::{
    ChangeDecision, ChangeSource, CharacterId, EventEffects, EventId, GameState, RelationshipStage,
    StateChangeLog, StatePatch, StoryRules,
};

use crate::error::{CoreError, PATCH_DELTA_OUT_OF_RANGE, PATCH_STAT_OUT_OF_RANGE};
use crate::eval::{EvalContext, evaluate};
use crate::state_machine::{confirm_route, resolve_ending, resolve_relationship_transition};
use crate::validator::{ValidationOutcome, validate};

/// Successful result of [`apply_patch`].
#[derive(Clone, Debug)]
pub struct ApplyResult {
    /// The new state, with revision bumped exactly once.
    pub state: GameState,
    /// Field-level audit entries (`decision: Applied`).
    pub change_log: Vec<StateChangeLog>,
    /// Non-fatal warnings emitted during validation/apply.
    pub warnings: Vec<story_model::GenerationWarning>,
    /// Events that fired during the turn, in fire order
    /// (`priority desc, id asc`).
    pub fired_events: Vec<EventId>,
    /// Computed ending id, if any condition matched.
    pub ending: Option<String>,
}

/// Apply `patch` to `state` atomically.
///
/// On `Err`, `state` is byte-identical to its pre-call self (atomicity). On
/// `Ok`, the returned [`ApplyResult::state`] has its `revision` bumped exactly
/// once.
pub fn apply_patch(
    state: &GameState,
    rules: &StoryRules,
    patch: &StatePatch,
    generation_id: Uuid,
) -> Result<ApplyResult, CoreError> {
    // Stage 1–6: validate. Hard failure → return Err; original untouched.
    let ValidationOutcome {
        sanitized,
        warnings,
        ..
    } = validate(state, rules, patch, generation_id)?;

    // Clone so we never mutate the caller's state.
    let mut next = state.clone();
    let mut log = ChangeLog::new(generation_id, state.turn);

    // --- Stage 7a: apply direct patch mutations --------------------------
    apply_direct_mutations(state, &mut next, rules, &sanitized, &mut log);

    // --- Stage 7b: compute fired events ---------------------------------
    let fired = compute_fired_events(&next, rules, &sanitized.add_events);

    // Did any *fired* event grant allow_relationship_skip?
    let fired_skip_allowed = fired.iter().any(|ev| {
        rules
            .events
            .get(ev)
            .is_some_and(|d| d.allow_relationship_skip)
    });

    // --- Stage 7c: apply relationship_transitions ----------------------
    apply_relationship_transitions(
        state,
        &mut next,
        &sanitized.relationship_transitions,
        fired_skip_allowed,
        &mut log,
    )?;

    // --- Stage 7d: fire events in (priority desc, id asc) --------------
    for ev_id in &fired {
        let def = rules
            .events
            .get(ev_id)
            .expect("fired events come from add_events validated against rules");
        apply_event_effects(state, &mut next, rules, &def.effects, ev_id, &mut log)?;
    }

    // --- Stage 7e: resolve route + ending ------------------------------
    let ending = resolve_ending(&next, rules);
    if let Some(ref ending_id) = ending {
        record_field(
            &mut log,
            ChangeSource::Reducer,
            "ending",
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::json!(ending_id),
            None,
        );
    }
    let new_route = confirm_route(&next, rules, &sanitized.proposed_route);
    if new_route != state.active_route {
        let before = state
            .active_route
            .clone()
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null);
        let proposed = sanitized
            .proposed_route
            .clone()
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null);
        let applied = new_route
            .clone()
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null);
        record_field(
            &mut log,
            ChangeSource::Reducer,
            "active_route",
            before,
            proposed,
            applied,
            None,
        );
    }
    next.active_route = new_route;

    // --- Stage 8: bump revision exactly once --------------------------
    next.revision = state.revision + 1;
    // `turn` is intentionally NOT bumped here — see module docs.

    Ok(ApplyResult {
        state: next,
        change_log: log.into_entries(),
        warnings,
        fired_events: fired,
        ending,
    })
}

// ---------------------------------------------------------------------------
// Direct mutations from the sanitized patch
// ---------------------------------------------------------------------------

fn apply_direct_mutations(
    before: &GameState,
    next: &mut GameState,
    rules: &StoryRules,
    patch: &StatePatch,
    log: &mut ChangeLog,
) {
    let llm = ChangeSource::LlmPatch;

    for (ch, delta) in &patch.affection_delta {
        let c = next.characters.get_mut(ch).expect("validated");
        let was = c.affection;
        c.affection = was.saturating_add(*delta);
        log.record(ChangeEntry::applied(
            llm,
            format!("characters.{ch}.affection"),
            serde_json::json!(was),
            serde_json::json!(delta),
            serde_json::json!(c.affection),
        ));
    }
    for (ch, delta) in &patch.trust_delta {
        let c = next.characters.get_mut(ch).expect("validated");
        let was = c.trust;
        c.trust = was.saturating_add(*delta);
        log.record(ChangeEntry::applied(
            llm,
            format!("characters.{ch}.trust"),
            serde_json::json!(was),
            serde_json::json!(delta),
            serde_json::json!(c.trust),
        ));
    }
    for (ch, delta) in &patch.tension_delta {
        let c = next.characters.get_mut(ch).expect("validated");
        let was = c.tension;
        c.tension = was.saturating_add(*delta);
        log.record(ChangeEntry::applied(
            llm,
            format!("characters.{ch}.tension"),
            serde_json::json!(was),
            serde_json::json!(delta),
            serde_json::json!(c.tension),
        ));
    }

    for (id, value) in &patch.set_flags {
        let was = before.flags.get(id).copied();
        next.flags.insert(id.clone(), *value);
        record_field(
            log,
            llm,
            format!("flags.{id}"),
            map_bool_to_json(was),
            serde_json::json!(value),
            serde_json::json!(value),
            None,
        );
    }

    for (id, delta) in &patch.number_delta {
        let def = rules.numbers.get(id).expect("validated");
        let was = before.numbers.get(id).copied().unwrap_or(def.default);
        let applied = was.saturating_add(*delta);
        next.numbers.insert(id.clone(), applied);
        record_field(
            log,
            llm,
            format!("numbers.{id}"),
            serde_json::json!(was),
            serde_json::json!(delta),
            serde_json::json!(applied),
            None,
        );
    }

    for (id, value) in &patch.set_strings {
        let was = before.strings.get(id).cloned();
        next.strings.insert(id.clone(), value.clone());
        record_field(
            log,
            llm,
            format!("strings.{id}"),
            was.clone()
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
            serde_json::json!(value),
            serde_json::json!(value),
            None,
        );
    }

    for (id, delta) in &patch.inventory_delta {
        let was = before.inventory.get(id).copied().unwrap_or(0);
        let applied = (was as i64 + *delta as i64) as u32;
        next.inventory.insert(id.clone(), applied);
        record_field(
            log,
            llm,
            format!("inventory.{id}"),
            serde_json::json!(was),
            serde_json::json!(delta),
            serde_json::json!(applied),
            None,
        );
    }

    for ev in &patch.add_events {
        if next.occurred_events.insert(ev.clone()) {
            record_field(
                log,
                llm,
                format!("occurred_events.{ev}"),
                serde_json::Value::Bool(false),
                serde_json::Value::Bool(true),
                serde_json::Value::Bool(true),
                None,
            );
        }
    }

    if let Some(loc) = &patch.next_location {
        let was = before.location.clone();
        next.location = loc.clone();
        record_field(
            log,
            llm,
            "location",
            serde_json::json!(was.0),
            serde_json::json!(loc.0),
            serde_json::json!(loc.0),
            None,
        );
    }

    if let Some(slot) = &patch.next_time_slot {
        let was = before.time_slot;
        next.time_slot = *slot;
        record_field(
            log,
            llm,
            "time_slot",
            serde_json::json!(was),
            serde_json::json!(slot),
            serde_json::json!(slot),
            None,
        );
    }
}

// ---------------------------------------------------------------------------
// Relationship transitions (from the patch)
// ---------------------------------------------------------------------------

fn apply_relationship_transitions(
    before: &GameState,
    next: &mut GameState,
    transitions: &std::collections::BTreeMap<CharacterId, RelationshipStage>,
    fired_skip_allowed: bool,
    log: &mut ChangeLog,
) -> Result<(), CoreError> {
    for (ch, target) in transitions {
        let current = before
            .characters
            .get(ch)
            .map(|c| c.relationship_stage)
            .expect("validated");
        let resolved = resolve_relationship_transition(current, *target, fired_skip_allowed)?;
        let c = next.characters.get_mut(ch).expect("validated");
        c.relationship_stage = resolved;
        let entry = ChangeEntry::applied(
            ChangeSource::LlmPatch,
            format!("characters.{ch}.relationship_stage"),
            serde_json::json!(format!("{current:?}")),
            serde_json::json!(format!("{target:?}")),
            serde_json::json!(format!("{resolved:?}")),
        );
        log.record(entry);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Event firing
// ---------------------------------------------------------------------------

/// Determine which events fire, in deterministic `(priority desc, id asc)`
/// order. Returns the ordered list.
fn compute_fired_events(
    state: &GameState,
    rules: &StoryRules,
    add_events: &BTreeSet<String>,
) -> Vec<EventId> {
    let ctx = EvalContext { state, rules };
    let mut candidates: Vec<&story_model::EventDef> = rules
        .events
        .values()
        .filter(|d| add_events.contains(&d.id.0))
        .filter(|d| match &d.route_lock {
            Some(lock) => state.active_route.as_deref() == Some(lock.as_str()),
            None => true,
        })
        .filter(|d| evaluate(&d.trigger, &ctx))
        .collect();
    // Sort by id (asc) then by priority (desc); stable sort keeps id-asc order
    // within a priority bucket.
    candidates.sort_by_key(|d| d.id.clone());
    candidates.sort_by_key(|d| std::cmp::Reverse(d.priority));
    candidates.into_iter().map(|d| d.id.clone()).collect()
}

fn apply_event_effects(
    before: &GameState,
    next: &mut GameState,
    rules: &StoryRules,
    effects: &EventEffects,
    ev_id: &EventId,
    log: &mut ChangeLog,
) -> Result<(), CoreError> {
    let source = ChangeSource::EventRule;
    let rule_id = ev_id.0.clone();

    for (ch, delta) in &effects.affection_delta {
        if let Some(c) = next.characters.get_mut(ch) {
            let was = c.affection;
            c.affection = was.saturating_add(*delta);
            let entry = ChangeEntry::applied(
                source,
                format!("characters.{ch}.affection"),
                serde_json::json!(was),
                serde_json::json!(delta),
                serde_json::json!(c.affection),
            )
            .with_rule(rule_id.clone());
            log.record(entry);
        }
    }
    // Trust/tension: same apply mechanics as affection (saturating_add, skip
    // unknown characters defensively) plus the delta-magnitude and projected-
    // stat range guards the patch-path validator enforces. Out-of-range deltas
    // reject the whole turn with an error (no silent clamp); atomicity holds
    // because the reducer only ever mutates a clone of the caller's state.
    apply_event_trust_delta(
        next,
        rules,
        &effects.trust_delta,
        ev_id,
        source,
        &rule_id,
        log,
    )?;
    apply_event_tension_delta(
        next,
        rules,
        &effects.tension_delta,
        ev_id,
        source,
        &rule_id,
        log,
    )?;
    for (id, value) in &effects.set_flags {
        let was = before.flags.get(id).copied();
        next.flags.insert(id.clone(), *value);
        record_field(
            log,
            source,
            format!("flags.{id}"),
            map_bool_to_json(was),
            serde_json::json!(value),
            serde_json::json!(value),
            Some(rule_id.clone()),
        );
    }
    for (id, delta) in &effects.number_delta {
        let def_default = rules.numbers.get(id).map(|d| d.default).unwrap_or(0);
        let was = before.numbers.get(id).copied().unwrap_or(def_default);
        let applied = was.saturating_add(*delta);
        next.numbers.insert(id.clone(), applied);
        record_field(
            log,
            source,
            format!("numbers.{id}"),
            serde_json::json!(was),
            serde_json::json!(delta),
            serde_json::json!(applied),
            Some(rule_id.clone()),
        );
    }
    for ev in &effects.add_events {
        if next.occurred_events.insert(ev.clone()) {
            record_field(
                log,
                source,
                format!("occurred_events.{ev}"),
                serde_json::Value::Bool(false),
                serde_json::Value::Bool(true),
                serde_json::Value::Bool(true),
                Some(rule_id.clone()),
            );
        }
    }
    if let Some(loc) = &effects.force_location {
        let was = before.location.clone();
        next.location = loc.clone();
        record_field(
            log,
            source,
            "location",
            serde_json::json!(was.0),
            serde_json::json!(loc.0),
            serde_json::json!(loc.0),
            Some(rule_id.clone()),
        );
    }
    for (ch, stage) in &effects.force_relationship {
        if let Some(c) = next.characters.get_mut(ch) {
            let was = c.relationship_stage;
            c.relationship_stage = *stage;
            record_field(
                log,
                source,
                format!("characters.{ch}.relationship_stage"),
                serde_json::json!(format!("{was:?}")),
                serde_json::json!(format!("{stage:?}")),
                serde_json::json!(format!("{stage:?}")),
                Some(rule_id.clone()),
            );
        }
    }
    // effects.ending is intentionally NOT applied here — endings are computed
    // solely by resolve_ending() from EndingDef conditions, never from events
    // or the patch. (Documented for future maintainers.)
    let _ = effects.ending;
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-axis event-effect application (trust/tension)
// ---------------------------------------------------------------------------

/// Apply `effects.trust_delta` to `state.characters[c].trust`.
///
/// Mirrors the affection apply mechanics — [`i16::saturating_add`] under a
/// `characters.{c}.trust` change-log entry attributed to
/// [`ChangeSource::EventRule`] — and adds the same delta-magnitude +
/// projected-stat range guards the patch-path validator applies via
/// `check_axis_delta`. Unknown character ids are skipped defensively (the
/// Pack-load-time referential-integrity check is expected to catch them
/// earlier); an out-of-range delta for a *known* character rejects the turn
/// with [`CoreError::invalid_patch`] so the reducer's atomic clone is
/// discarded and the caller's state is left byte-identical.
fn apply_event_trust_delta(
    next: &mut GameState,
    rules: &StoryRules,
    deltas: &BTreeMap<CharacterId, i16>,
    ev_id: &EventId,
    source: ChangeSource,
    rule_id: &str,
    log: &mut ChangeLog,
) -> Result<(), CoreError> {
    let range = &rules.delta_limits.trust;
    for (ch, delta) in deltas {
        let Some(c) = next.characters.get_mut(ch) else {
            continue;
        };
        if !range.contains(*delta) {
            return Err(CoreError::invalid_patch(
                PATCH_DELTA_OUT_OF_RANGE,
                format!(
                    "event {ev_id} trust_delta for {ch} is {delta}, outside [{}, {}]",
                    range.min, range.max
                ),
            ));
        }
        let was = c.trust;
        let projected = was.saturating_add(*delta);
        if !rules.stat_limits.contains(projected) {
            return Err(CoreError::invalid_patch(
                PATCH_STAT_OUT_OF_RANGE,
                format!(
                    "event {ev_id} trust for {ch} projected to {projected}, outside [{}, {}]",
                    rules.stat_limits.min, rules.stat_limits.max
                ),
            ));
        }
        c.trust = projected;
        let entry = ChangeEntry::applied(
            source,
            format!("characters.{ch}.trust"),
            serde_json::json!(was),
            serde_json::json!(delta),
            serde_json::json!(projected),
        )
        .with_rule(rule_id.to_string());
        log.record(entry);
    }
    Ok(())
}

/// Apply `effects.tension_delta` to `state.characters[c].tension`.
///
/// Structurally identical to [`apply_event_trust_delta`]; see its docs.
fn apply_event_tension_delta(
    next: &mut GameState,
    rules: &StoryRules,
    deltas: &BTreeMap<CharacterId, i16>,
    ev_id: &EventId,
    source: ChangeSource,
    rule_id: &str,
    log: &mut ChangeLog,
) -> Result<(), CoreError> {
    let range = &rules.delta_limits.tension;
    for (ch, delta) in deltas {
        let Some(c) = next.characters.get_mut(ch) else {
            continue;
        };
        if !range.contains(*delta) {
            return Err(CoreError::invalid_patch(
                PATCH_DELTA_OUT_OF_RANGE,
                format!(
                    "event {ev_id} tension_delta for {ch} is {delta}, outside [{}, {}]",
                    range.min, range.max
                ),
            ));
        }
        let was = c.tension;
        let projected = was.saturating_add(*delta);
        if !rules.stat_limits.contains(projected) {
            return Err(CoreError::invalid_patch(
                PATCH_STAT_OUT_OF_RANGE,
                format!(
                    "event {ev_id} tension for {ch} projected to {projected}, outside [{}, {}]",
                    rules.stat_limits.min, rules.stat_limits.max
                ),
            ));
        }
        c.tension = projected;
        let entry = ChangeEntry::applied(
            source,
            format!("characters.{ch}.tension"),
            serde_json::json!(was),
            serde_json::json!(delta),
            serde_json::json!(projected),
        )
        .with_rule(rule_id.to_string());
        log.record(entry);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Change-log builder
// ---------------------------------------------------------------------------

/// Mutable audit-log accumulator bound to a single turn. Carries the
/// `generation_id` + `turn` so individual record calls need not repeat them.
struct ChangeLog {
    generation_id: Uuid,
    turn: u32,
    entries: Vec<StateChangeLog>,
}

impl ChangeLog {
    fn new(generation_id: Uuid, turn: u32) -> Self {
        Self {
            generation_id,
            turn,
            entries: Vec::new(),
        }
    }

    fn into_entries(self) -> Vec<StateChangeLog> {
        self.entries
    }

    /// Append a fully-described entry. The decision is always `Applied` at
    /// this MVP scope — partial-rejection is handled by the validator before
    /// the reducer ever mutates a clone.
    fn record(&mut self, entry: ChangeEntry) {
        self.entries.push(StateChangeLog {
            id: Uuid::new_v4(),
            generation_id: self.generation_id,
            turn: self.turn,
            source: entry.source,
            field_path: entry.field_path,
            before: entry.before,
            proposed: entry.proposed,
            applied: entry.applied,
            decision: entry.decision,
            rule_id: entry.rule_id,
            reason_code: entry.reason_code,
        });
    }
}

/// Per-field data for one change-log entry.
struct ChangeEntry {
    source: ChangeSource,
    field_path: String,
    before: serde_json::Value,
    proposed: serde_json::Value,
    applied: Option<serde_json::Value>,
    decision: ChangeDecision,
    rule_id: Option<String>,
    reason_code: Option<String>,
}

impl ChangeEntry {
    /// Build an `Applied` entry with no rule/reason attribution.
    fn applied(
        source: ChangeSource,
        field_path: impl Into<String>,
        before: serde_json::Value,
        proposed: serde_json::Value,
        applied: serde_json::Value,
    ) -> Self {
        Self {
            source,
            field_path: field_path.into(),
            before,
            proposed,
            applied: Some(applied),
            decision: ChangeDecision::Applied,
            rule_id: None,
            reason_code: None,
        }
    }

    /// Attach a rule id (used by event-driven entries).
    fn with_rule(mut self, rule_id: String) -> Self {
        self.rule_id = Some(rule_id);
        self
    }
}

/// Record an arbitrary field change with optional rule attribution.
fn record_field(
    log: &mut ChangeLog,
    source: ChangeSource,
    field_path: impl Into<String>,
    before: serde_json::Value,
    proposed: serde_json::Value,
    applied: serde_json::Value,
    rule_id: Option<String>,
) {
    let mut entry = ChangeEntry::applied(source, field_path, before, proposed, applied);
    entry.rule_id = rule_id;
    log.record(entry);
}

fn map_bool_to_json(v: Option<bool>) -> serde_json::Value {
    match v {
        Some(b) => serde_json::json!(b),
        None => serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_patch_increments_revision_exactly_once_and_leaves_turn() {
        let (state, rules) = super::tests_support::hand_built();
        let patch = StatePatch::default();
        let before_revision = state.revision;
        let before_turn = state.turn;
        let result = apply_patch(&state, &rules, &patch, Uuid::nil()).expect("empty patch applies");
        assert_eq!(result.state.revision, before_revision + 1);
        assert_eq!(
            result.state.turn, before_turn,
            "turn is not bumped by reducer"
        );
    }
}

/// Shared test scaffolding (hand-built [`GameState`] + [`StoryRules`]) used by
/// unit tests across `story-core` modules.
#[cfg(test)]
pub(crate) mod tests_support {
    use std::collections::{BTreeMap, BTreeSet, VecDeque};

    use story_model::{
        CharacterId, CharacterState, ConditionExpr, Emotion, EndingDef, EventDef, EventEffects,
        EventId, GameState, LocationDef, LocationId, RelationshipStage, RouteDef,
        SAVE_SCHEMA_VERSION, StoryRules,
    };

    pub(crate) fn hand_built() -> (GameState, StoryRules) {
        let state = GameState {
            save_schema_version: SAVE_SCHEMA_VERSION,
            story_id: "demo".to_string(),
            story_version: semver::Version::new(0, 1, 0),
            pack_schema_version: semver::Version::new(1, 0, 0),
            session_id: uuid::Uuid::nil(),
            current_chapter: "ch-1".to_string(),
            current_scene_id: "sc-1".to_string(),
            turn: 6,
            location: LocationId("loc-classroom".to_string()),
            time_slot: story_model::TimeSlot::Morning,
            active_route: None,
            characters: chars(),
            flags: BTreeMap::from([("met_namu".to_string(), true)]),
            numbers: BTreeMap::from([("score".to_string(), 7)]),
            strings: BTreeMap::new(),
            inventory: BTreeMap::from([("i-letter".to_string(), 2)]),
            occurred_events: BTreeSet::new(),
            recent_summaries: VecDeque::new(),
            memories: Vec::new(),
            history: Vec::new(),
            last_choice: None,
            revision: 5,
            stable_generation_id: None,
        };
        let rules = rules();
        (state, rules)
    }

    fn chars() -> BTreeMap<CharacterId, CharacterState> {
        let mut m = BTreeMap::new();
        m.insert(
            CharacterId("c-namu".to_string()),
            CharacterState {
                affection: 25,
                trust: 10,
                tension: 3,
                relationship_stage: RelationshipStage::Friend,
                current_emotion: Emotion::Neutral,
                known_facts: BTreeSet::new(),
                address_form_overrides: BTreeMap::new(),
            },
        );
        m
    }

    fn rules() -> StoryRules {
        let mut r = StoryRules::minimal();
        let mut adj = BTreeSet::new();
        adj.insert(LocationId("loc-hall".to_string()));
        r.locations.insert(
            LocationId("loc-classroom".to_string()),
            LocationDef {
                id: LocationId("loc-classroom".to_string()),
                display_name: "Classroom".to_string(),
                adjacent: adj,
            },
        );
        r.locations.insert(
            LocationId("loc-hall".to_string()),
            LocationDef {
                id: LocationId("loc-hall".to_string()),
                display_name: "Hall".to_string(),
                adjacent: BTreeSet::from([LocationId("loc-classroom".to_string())]),
            },
        );
        r.allowed_flags
            .extend(["met_namu", "f-brave"].map(String::from));
        r.allowed_items.insert("i-letter".to_string());
        r.endings.insert(
            "end-good".to_string(),
            EndingDef {
                id: "end-good".to_string(),
                condition: ConditionExpr::AffectionAtLeast {
                    character: CharacterId("c-namu".to_string()),
                    value: 100,
                },
                label: "Good".to_string(),
            },
        );
        r.routes.insert(
            "route-roof".to_string(),
            RouteDef {
                id: "route-roof".to_string(),
                enter_condition: ConditionExpr::AffectionAtLeast {
                    character: CharacterId("c-namu".to_string()),
                    value: 20,
                },
                premise: "The rooftop calls.".to_string(),
            },
        );
        r.events.insert(
            EventId("e-namu".to_string()),
            EventDef {
                id: EventId("e-namu".to_string()),
                trigger: ConditionExpr::FlagEquals {
                    id: "met_namu".to_string(),
                    value: true,
                },
                priority: 1,
                effects: EventEffects {
                    affection_delta: BTreeMap::from([(CharacterId("c-namu".to_string()), 2)]),
                    ..EventEffects::default()
                },
                allow_relationship_skip: false,
                route_lock: None,
            },
        );
        r
    }
}
