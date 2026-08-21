//! Deterministic event-based memory creation (ST-029).
//!
//! [`memories_from_turn`] derives long-term [`MemoryEntry`]s from the
//! [`ApplyResult::fired_events`] of a single accepted turn. The mapping is a
//! pure function of (`ApplyResult`, [`StoryPack`], turn, source history id):
//! same inputs always yield byte-identical output, including stable
//! [`Uuid`] ids derived from a SHA-256 seed (no randomness, no HashMap
//! iteration).
//!
//! # Event -> MemoryKind mapping (convention)
//!
//! `EventDef` does not yet carry an explicit `memory_kind` hint, so the kind is
//! derived deterministically from the event id (case-insensitive substring):
//!
//! | id contains             | MemoryKind        | starts `unresolved` |
//! |-------------------------|-------------------|---------------------|
//! | `promise` / `약속`        | `Promise`         | yes                 |
//! | `secret` / `비밀`         | `SecretReveal`    | yes                 |
//! | `confession` / `고백`     | `Confession`      | no                  |
//! | `betrayal` / `배신`       | `Betrayal`        | no                  |
//! | `first_meeting`         | `FirstMeeting`    | no                  |
//! | `key_item`              | `KeyItem`         | no                  |
//! | `relationship` / `관계`   | `RelationshipChange` | no               |
//! | `resolve_promise[...]`  | resolver — clears prior `Promise` unresolved | no |
//! | `resolve_secret[...]`   | resolver — clears prior `SecretReveal` unresolved | no |
//!
//! Events whose id carries none of these hints still produce an `Other`
//! memory. Fired events are canonical, validator-approved story beats; dropping
//! them because a pack author chose a semantic id such as
//! `event.truth_revealed` made long-running relationships forget their most
//! important turns. The resolver convention is the MVP "resolves" relation
//! permitted by the W9 plan: a fired resolver event clears the `unresolved`
//! flag on prior memories of the matching kind via [`resolve_memories`].
//!
//! # Knowledge-owner (`known_by`) policy
//!
//! The player sentinel (`"player"`) plus every participant of the event is
//! recorded as a knower. Participants are the union of the keys of the event's
//! `affection_delta`, `trust_delta`, `tension_delta`, and `force_relationship`
//! effect maps. No other character is ever invented as a knower.
//!
//! # Determinism / ADR-003
//!
//! `sha2` is the only non-`story-model` dependency used here, already cleared
//! by ADR-003. No UI/HTTP/FS.

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};
use story_model::uuid::Uuid;
use story_model::{
    CharacterId, EventDef, EventEffects, EventId, LocationId, MemoryEntry, MemoryFact, MemoryKind,
    StoryPack,
};

use crate::reducer::ApplyResult;

/// Sentinel character id representing the player in `known_by` sets.
pub const PLAYER: &str = "player";

/// Derive long-term memories from a single turn's fired events.
///
/// Pure and deterministic: identical (`result`, `pack`, `turn`,
/// `source_history_id`) inputs yield identical output. Returns one
/// [`MemoryEntry`] per fired event whose id maps to a memory kind, sorted by
/// stable id.
pub fn memories_from_turn(
    result: &ApplyResult,
    pack: &StoryPack,
    turn: u32,
    source_history_id: Uuid,
) -> Vec<MemoryEntry> {
    let location = result.state.location.clone();
    let mut out: Vec<MemoryEntry> = Vec::new();
    for ev_id in &result.fired_events {
        let Some(def) = pack.rules.events.get(ev_id) else {
            continue;
        };
        let kind = memory_kind_for_event(&def.id.0).unwrap_or(MemoryKind::Other);
        let participants = participants_of(def);
        let (facts, importance) = build_facts(&def.id.0, kind, &participants);
        let unresolved = starts_unresolved(&def.id.0, kind);
        let entry = MemoryEntry {
            // Deterministic id: hash of (turn, event id, source history id).
            id: deterministic_uuid(format!("{turn}|{}|{source_history_id}", def.id.0)),
            kind,
            facts,
            interpretation: interpretation_for(&def.id.0, kind),
            participants,
            location: Some(location.clone()),
            importance,
            unresolved,
            source_history_ids: vec![source_history_id],
            created_turn: turn,
            last_recalled_turn: None,
        };
        out.push(entry);
    }
    // Deterministic output order: sort by stable id (ids are unique per event).
    out.sort_by_key(|m| m.id);
    out
}

/// Clear the `unresolved` flag on prior memories when a resolving event fires.
///
/// Convention: a fired event whose id contains `resolve_promise` clears prior
/// `Promise` memories; `resolve_secret` clears prior `SecretReveal` memories.
/// Used by the engine in `reduce_accept` after new memories are appended.
/// Deterministic: operates on every matching memory regardless of map order.
pub fn resolve_memories(memories: &mut [MemoryEntry], fired_event_ids: &[EventId]) {
    let resolver_kinds: Vec<MemoryKind> = fired_event_ids
        .iter()
        .filter_map(|e| resolver_kind_for_event(&e.0))
        .collect();
    if resolver_kinds.is_empty() {
        return;
    }
    for m in memories.iter_mut() {
        if m.unresolved && resolver_kinds.contains(&m.kind) {
            m.unresolved = false;
        }
    }
}

// ---------------------------------------------------------------------------
// Event id -> MemoryKind classification
// ---------------------------------------------------------------------------

/// Map an event id to its memory kind, or `None` if the event is not
/// memory-worthy. Case-insensitive substring match on the id.
pub fn memory_kind_for_event(id: &str) -> Option<MemoryKind> {
    let l = id.to_lowercase();
    if contains_any(&l, &["resolve_promise", "resolve.promise"]) {
        return Some(MemoryKind::Promise);
    }
    if contains_any(&l, &["resolve_secret", "resolve.secret"]) {
        return Some(MemoryKind::SecretReveal);
    }
    if contains_any(&l, &["promise", "약속"]) {
        return Some(MemoryKind::Promise);
    }
    if contains_any(&l, &["secret", "비밀"]) {
        return Some(MemoryKind::SecretReveal);
    }
    if contains_any(&l, &["confession", "고백"]) {
        return Some(MemoryKind::Confession);
    }
    if contains_any(&l, &["betrayal", "배신"]) {
        return Some(MemoryKind::Betrayal);
    }
    if contains_any(&l, &["first_meeting", "first.meeting", "firstmeet"]) {
        return Some(MemoryKind::FirstMeeting);
    }
    if contains_any(&l, &["key_item", "key.item", "keyitem"]) {
        return Some(MemoryKind::KeyItem);
    }
    if contains_any(&l, &["relationship", "rel_change", "relchange", "관계"]) {
        return Some(MemoryKind::RelationshipChange);
    }
    None
}

/// If this event id is a resolver, the kind of prior memory it clears.
fn resolver_kind_for_event(id: &str) -> Option<MemoryKind> {
    let l = id.to_lowercase();
    if contains_any(&l, &["resolve_promise", "resolve.promise"]) {
        return Some(MemoryKind::Promise);
    }
    if contains_any(&l, &["resolve_secret", "resolve.secret"]) {
        return Some(MemoryKind::SecretReveal);
    }
    None
}

/// Whether a freshly-created memory of `kind` (from event `id`) starts as an
/// open thread. Resolvers never start unresolved; promises and secrets do.
fn starts_unresolved(id: &str, kind: MemoryKind) -> bool {
    if resolver_kind_for_event(id).is_some() {
        return false;
    }
    matches!(kind, MemoryKind::Promise | MemoryKind::SecretReveal)
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

// ---------------------------------------------------------------------------
// Participants / facts / importance / interpretation
// ---------------------------------------------------------------------------

/// Characters participating in an event: the union of the keys of its typed
/// delta maps and its forced-relationship map.
fn participants_of(def: &EventDef) -> BTreeSet<CharacterId> {
    let e: &EventEffects = &def.effects;
    let mut s = BTreeSet::new();
    for k in e.affection_delta.keys() {
        s.insert(k.clone());
    }
    for k in e.trust_delta.keys() {
        s.insert(k.clone());
    }
    for k in e.tension_delta.keys() {
        s.insert(k.clone());
    }
    for k in e.force_relationship.keys() {
        s.insert(k.clone());
    }
    s
}

/// Importance weight per kind (0..=10). Promise/secret are the most salient.
fn importance_for(kind: MemoryKind) -> u8 {
    match kind {
        MemoryKind::Promise | MemoryKind::SecretReveal => 9,
        MemoryKind::Confession | MemoryKind::Betrayal => 8,
        MemoryKind::RelationshipChange => 7,
        MemoryKind::KeyItem => 6,
        MemoryKind::FirstMeeting => 5,
        MemoryKind::Other => 3,
    }
}

/// Deterministically derive the structured fact(s), importance, and unresolved
/// flag for a memory of `kind` sourced from event `id`.
fn build_facts(
    id: &str,
    kind: MemoryKind,
    participants: &BTreeSet<CharacterId>,
) -> (Vec<MemoryFact>, u8) {
    let player = CharacterId(PLAYER.to_string());
    let mut known_by: BTreeSet<CharacterId> = std::iter::once(player.clone()).collect();
    for p in participants {
        known_by.insert(p.clone());
    }
    // The first non-player participant (deterministic BTreeSet order) is the
    // NPC counterpart when one is needed (secret-reveal subject, etc.).
    let other = participants
        .iter()
        .find(|c| c.0 != player.0)
        .cloned()
        .unwrap_or(player.clone());

    let (subject, predicate) = match kind {
        MemoryKind::Promise => (player.0.as_str(), "약속"),
        MemoryKind::SecretReveal => (other.0.as_str(), "비밀"),
        MemoryKind::Confession => (player.0.as_str(), "고백"),
        MemoryKind::Betrayal => (player.0.as_str(), "배신"),
        MemoryKind::KeyItem => (player.0.as_str(), "key_item"),
        MemoryKind::FirstMeeting => (other.0.as_str(), "first_meeting"),
        MemoryKind::RelationshipChange => (player.0.as_str(), "관계변화"),
        MemoryKind::Other => (player.0.as_str(), "사건"),
    };
    let object = format!("event:{id}");
    let fact = MemoryFact {
        subject: subject.to_string(),
        predicate: predicate.to_string(),
        object,
        known_by,
        certainty: 1.0,
    };
    (vec![fact], importance_for(kind))
}

/// Short deterministic Korean interpretation line for the memory.
fn interpretation_for(id: &str, kind: MemoryKind) -> Option<String> {
    let label = match kind {
        MemoryKind::Promise => "약속",
        MemoryKind::SecretReveal => "비밀 공개",
        MemoryKind::Confession => "고백",
        MemoryKind::Betrayal => "배신",
        MemoryKind::KeyItem => "주요 사물",
        MemoryKind::FirstMeeting => "첫 만남",
        MemoryKind::RelationshipChange => "관계 변화",
        MemoryKind::Other => "사건",
    };
    Some(format!("{label} (event:{id})"))
}

// ---------------------------------------------------------------------------
// Deterministic id helper
// ---------------------------------------------------------------------------

/// Build a stable [`Uuid`] from a string seed via SHA-256 (first 16 bytes).
/// Deterministic: same seed ⇒ same uuid, no RNG.
fn deterministic_uuid(seed: String) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    let bytes = hasher.finalize();
    Uuid::from_slice(&bytes[..16]).expect("sha256 yields 32 bytes; first 16 fit a Uuid")
}

// Allow the helper signature to take a borrowed `LocationId` cleanly in future
// extensions without dragging the import in only one branch.
#[allow(dead_code)]
fn _location_anchor(_l: LocationId) {}

#[cfg(test)]
mod tests {
    //! In-crate unit tests pin the pure classification + helper logic. The
    //! end-to-end `memories_from_turn` flow is exercised by the integration
    //! test in `tests/memory_creation.rs`.

    use super::*;

    #[test]
    fn classification_table() {
        assert_eq!(
            memory_kind_for_event("promise.share_umbrella"),
            Some(MemoryKind::Promise)
        );
        assert_eq!(
            memory_kind_for_event("SECRET.past_letter"),
            Some(MemoryKind::SecretReveal)
        );
        assert_eq!(
            memory_kind_for_event("confession.love"),
            Some(MemoryKind::Confession)
        );
        assert_eq!(
            memory_kind_for_event("betrayal.lie"),
            Some(MemoryKind::Betrayal)
        );
        assert_eq!(
            memory_kind_for_event("first_meeting.yuna"),
            Some(MemoryKind::FirstMeeting)
        );
        assert_eq!(
            memory_kind_for_event("key_item.umbrella"),
            Some(MemoryKind::KeyItem)
        );
        assert_eq!(
            memory_kind_for_event("relationship.truce"),
            Some(MemoryKind::RelationshipChange)
        );
        assert_eq!(memory_kind_for_event("ambient.rain"), None);
    }

    #[test]
    fn korean_substring_classifies() {
        assert_eq!(
            memory_kind_for_event("event.약속_at_river"),
            Some(MemoryKind::Promise)
        );
        assert_eq!(
            memory_kind_for_event("event.비밀_reveal"),
            Some(MemoryKind::SecretReveal)
        );
    }

    #[test]
    fn resolver_ids_classify_and_resolve() {
        assert_eq!(
            resolver_kind_for_event("resolve_promise.share_umbrella"),
            Some(MemoryKind::Promise)
        );
        assert_eq!(
            resolver_kind_for_event("resolve_secret.past_letter"),
            Some(MemoryKind::SecretReveal)
        );
        // A resolver event still maps to its base kind for memory creation,
        // but never starts unresolved.
        let kind = memory_kind_for_event("resolve_promise.share_umbrella").unwrap();
        assert_eq!(kind, MemoryKind::Promise);
        assert!(!starts_unresolved("resolve_promise.share_umbrella", kind));
    }

    #[test]
    fn deterministic_uuid_is_stable() {
        let a = deterministic_uuid("seed-one".to_string());
        let b = deterministic_uuid("seed-one".to_string());
        let c = deterministic_uuid("seed-two".to_string());
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn starts_unresolved_only_for_promises_and_secrets() {
        assert!(starts_unresolved("promise.x", MemoryKind::Promise));
        assert!(starts_unresolved("secret.x", MemoryKind::SecretReveal));
        for k in [
            MemoryKind::Confession,
            MemoryKind::Betrayal,
            MemoryKind::KeyItem,
            MemoryKind::FirstMeeting,
            MemoryKind::RelationshipChange,
            MemoryKind::Other,
        ] {
            assert!(!starts_unresolved("e", k), "{k:?} not unresolved");
        }
    }
}
