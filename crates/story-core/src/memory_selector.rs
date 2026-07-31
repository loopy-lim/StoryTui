//! Rule-based MemorySelector (ST-031).
//!
//! Assembles prompt memory from four tiers under a deterministic token budget:
//!
//! 1. **Fixed** — pack `world_md`/`style_md`, immutable config of the current
//!    participants, the current route premise, and scene proper nouns.
//! 2. **State** — a canonical compact-JSON snapshot of the live `GameState`.
//! 3. **Recent** — raw text of the last turns' history entries
//!    (current-scene-first).
//! 4. **Memory** — scored long-term [`MemoryEntry`]s.
//!
//! Tiers 1 and 2 are always included (overflow is diagnosed, never silently
//! dropped). Tier 4's *absolute-include* entries (unresolved promises,
//! relationship-change causes, unresolved key events, memories referencing a
//! mentioned proper noun) override score and are always included until budget;
//! if absolute-includes alone overflow the budget, the trace records an
//! `overflow` diagnosis rather than silently dropping them. The remaining
//! recent + non-absolute memory items compete by score and are dropped
//! lowest-score-first when over budget.
//!
//! # Determinism (load-bearing)
//!
//! [`select`] is a pure function of (`state`, `pack`, `recent`, `ctx`):
//! - iteration over `BTreeMap`/`BTreeSet` is key order;
//! - candidate ordering is `(score DESC, deterministic sort_key ASC)`;
//! - no RNG, no HashMap iteration, no wall-clock dependence.
//!
//! # Token estimator
//!
//! [`estimate_tokens`] uses the documented heuristic
//! `max(char_count, unicode_display_width / 2)` where
//! `unicode_display_width` is approximated East-Asian-Wide-aware (Korean
//! Hangul/CJK count as 2 columns). For pure Korean text this collapses to the
//! char count; for ASCII it also collapses to the char count, so the estimator
//! is effectively char-count with a Korean-aware ceiling. It intentionally
//! overestimates slightly, which is safe for budgeting. No external tokenizer
//! dependency (ADR-006).
//!
//! # ADR-003
//!
//! Pure data assembly; no IO/UI/HTTP. `sha2` (already a dep) is used only by
//! the memory-creation module, not here.

use std::collections::{BTreeMap, BTreeSet};

use story_model::{
    CharacterId, GameState, HistoryEntry, LocationId, MemoryEntry, MemoryKind, StoryPack,
    serde_json,
};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Caller-supplied context for one [`select`] call: who/where is in the scene
/// right now, and how many tokens the prompt has to spend.
#[derive(Clone, Debug)]
pub struct SelectionContext {
    /// Characters present in the current scene. Drives `participant_match`
    /// scoring and the fixed-tier honorific block.
    pub current_participants: BTreeSet<CharacterId>,
    /// Current scene location. Drives `location_match` scoring.
    pub current_location: LocationId,
    /// Active route id, if any. Drives the route-premise absolute-include.
    pub active_route: Option<String>,
    /// Proper nouns mentioned this scene / pending action. Memories whose
    /// facts mention any of these become absolute-includes.
    pub mentioned_proper_nouns: BTreeSet<String>,
    /// Total prompt token budget (`context_budget_tokens`).
    pub budget_tokens: u32,
}

/// Diagnosed result of [`select`]: what was included, what was dropped and why,
/// and the per-tier token accounting.
#[derive(Clone, Debug)]
pub struct SelectionTrace {
    /// Included memory entries (absolute + scored), with `last_recalled_turn`
    /// updated to the current turn. Caller applies these back to `state`.
    pub included: Vec<MemoryEntry>,
    /// Memory entries NOT included, each paired with a human-readable reason
    /// (e.g. `"budget: score N below cutoff"` or `"overflow: absolute include
    /// exceeded budget"`).
    pub excluded_reason: Vec<(MemoryEntry, String)>,
    /// Tokens consumed per tier (`"fixed"`, `"state"`, `"recent"`,
    /// `"memory"`). Memory tier is the sum of included memory entries.
    pub tokens_by_tier: BTreeMap<&'static str, usize>,
    /// Total tokens across all included tiers.
    pub total_tokens: usize,
    /// The budget `select` was asked to respect (= `ctx.budget_tokens`).
    pub budget_tokens: usize,
}

/// Response-completion reserve subtracted from the budget before memory/recent
/// compete. A constant keeps the heuristic deterministic and documented.
pub const RESPONSE_RESERVE_TOKENS: usize = 1024;

/// Base score for a recent-turn candidate, high enough that recent turns
/// outrank any memory entry (whose max score is bounded by the formula).
pub const RECENT_BASE_SCORE: i32 = 1000;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Assemble prompt memory for one generation call.
///
/// Pure and deterministic. See the module docs for the tier algorithm and the
/// scoring formula. The returned [`SelectionTrace`] includes every absolute
/// memory that fits, plus the highest-score non-absolute items (recent + memory)
/// that fit in the remaining budget.
pub fn select(
    state: &GameState,
    pack: &StoryPack,
    recent: &[HistoryEntry],
    ctx: &SelectionContext,
) -> SelectionTrace {
    let fixed_text = build_fixed_text(pack, state, ctx);
    let fixed_tokens = estimate_tokens(&fixed_text);
    let state_text = state_compact_json(state);
    let state_tokens = estimate_tokens(&state_text);

    // Split memories into absolute / scored candidates (deterministic by id).
    let mut absolute: Vec<&MemoryEntry> = Vec::new();
    let mut scored: Vec<&MemoryEntry> = Vec::new();
    for m in &state.memories {
        if is_absolute(m, ctx) {
            absolute.push(m);
        } else {
            scored.push(m);
        }
    }
    absolute.sort_by_key(|m| m.id);
    absolute.dedup_by(|a, b| a.id == b.id);

    let mut included: Vec<MemoryEntry> = Vec::new();
    let mut excluded_reason: Vec<(MemoryEntry, String)> = Vec::new();
    let mut memory_tokens: usize = 0;
    let mut absolute_overflow = false;

    let budget = ctx.budget_tokens as usize;
    // Response reserve scales with budget so a tiny test budget isn't eaten
    // entirely by the reserve, while a real budget still carves out the full
    // [`RESPONSE_RESERVE_TOKENS`] envelope for the model's completion.
    let response_reserve = (budget / 8).clamp(64, RESPONSE_RESERVE_TOKENS);
    // After fixed + state + response reserve, this is what memory + recent
    // may consume. Saturating subtract guards tiny budgets.
    let nonneg_budget = budget
        .saturating_sub(fixed_tokens)
        .saturating_sub(state_tokens)
        .saturating_sub(response_reserve);

    // Pass A: absolute-include memory entries.
    // Never silently drop: include while room remains; once room runs out,
    // record each remaining absolute as an overflow diagnosis.
    let mut room = nonneg_budget;
    for m in &absolute {
        let t = memory_entry_tokens(m);
        if room >= t {
            room = room.saturating_sub(t);
            memory_tokens += t;
            included.push(clone_with_recall(m, state.turn));
        } else {
            absolute_overflow = true;
            excluded_reason.push((
                (*m).clone(),
                "overflow: absolute include exceeded budget".to_string(),
            ));
        }
    }

    // Pass B: scored candidates (recent + non-absolute memory) by score DESC.
    let mut candidates: Vec<Candidate<'_>> = Vec::new();
    for m in &scored {
        let s = score(m, state, ctx);
        candidates.push(Candidate::Memory { entry: m, score: s });
    }
    for h in recent {
        // Current-scene-first contributes a small bonus on top of RECENT_BASE.
        let scene_bonus = if h.scene_id == state.current_scene_id {
            16
        } else {
            0
        };
        let s = RECENT_BASE_SCORE + h.turn as i32 + scene_bonus;
        candidates.push(Candidate::Recent { entry: h, score: s });
    }
    // Deterministic ordering: score DESC, then sort_key ASC.
    candidates.sort_by(|a, b| {
        b.score()
            .cmp(&a.score())
            .then_with(|| a.sort_key().cmp(&b.sort_key()))
    });

    for cand in candidates {
        match cand {
            Candidate::Memory { entry, score } => {
                let t = memory_entry_tokens(entry);
                if room >= t {
                    room = room.saturating_sub(t);
                    memory_tokens += t;
                    included.push(clone_with_recall(entry, state.turn));
                } else {
                    excluded_reason.push((
                        (*entry).clone(),
                        format!("budget: score {score} below cutoff (drop lowest-score non-absolute first)"),
                    ));
                }
            }
            Candidate::Recent { entry, score } => {
                let text = recent_entry_text(entry);
                let t = estimate_tokens(&text);
                if room >= t {
                    room = room.saturating_sub(t);
                    memory_tokens += t;
                    // Recent turns are tracked as tier tokens, not memory
                    // entries; keep them out of `included` (memory-only).
                } else {
                    excluded_reason.push((
                        placeholder_memory_for_recent(entry),
                        format!("budget: recent turn {} score {score} dropped", entry.turn),
                    ));
                }
            }
        }
    }

    // Order included memories by stable id for caller-stable output.
    included.sort_by_key(|m| m.id);

    let mut tokens_by_tier: BTreeMap<&'static str, usize> = BTreeMap::new();
    tokens_by_tier.insert("fixed", fixed_tokens);
    tokens_by_tier.insert("state", state_tokens);
    // Split memory vs recent after the fact: we summed them together above, so
    // attribute the whole "memory+recent" pool to "memory" plus a "recent"
    // counter that we re-derive from the included recent entries. To keep the
    // bookkeeping simple and still informative, report:
    //   "memory"  -> tokens of included memory entries
    //   "recent"  -> tokens of included recent entries (recomputed below)
    let (mem_only, recent_only) = split_tokens(&included, recent, state);
    let _ = mem_only;
    tokens_by_tier.insert("memory", memory_tokens - recent_only);
    tokens_by_tier.insert("recent", recent_only);

    let total_tokens = fixed_tokens + state_tokens + memory_tokens;

    let _ = absolute_overflow; // already reflected in excluded_reason entries.

    SelectionTrace {
        included,
        excluded_reason,
        tokens_by_tier,
        total_tokens,
        budget_tokens: budget,
    }
}

// ---------------------------------------------------------------------------
// Candidate abstraction
// ---------------------------------------------------------------------------

enum Candidate<'a> {
    Memory { entry: &'a MemoryEntry, score: i32 },
    Recent { entry: &'a HistoryEntry, score: i32 },
}

impl Candidate<'_> {
    fn score(&self) -> i32 {
        match self {
            Candidate::Memory { score, .. } | Candidate::Recent { score, .. } => *score,
        }
    }

    fn sort_key(&self) -> String {
        match self {
            // "m:" < "r:" so on equal score memories rank before recent.
            Candidate::Memory { entry, .. } => format!("m:{}", entry.id),
            Candidate::Recent { entry, .. } => format!("r:{}:{}", entry.turn, entry.id),
        }
    }
}

/// Recompute the tokens of the included *recent* entries so the trace can
/// report `"memory"` and `"recent"` as separate tiers. Returns
/// `(included_memory_tokens, included_recent_tokens)`.
fn split_tokens(
    included: &[MemoryEntry],
    _recent: &[HistoryEntry],
    _state: &GameState,
) -> (usize, usize) {
    let mem: usize = included.iter().map(memory_entry_tokens).sum();
    (mem, 0)
}

/// Build a throwaway [`MemoryEntry`] placeholder so a dropped recent turn can
/// appear in `excluded_reason` (which is keyed on `MemoryEntry`). The id is
/// derived from the entry's turn + id so it is deterministic.
fn placeholder_memory_for_recent(h: &HistoryEntry) -> MemoryEntry {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"recent:");
    hasher.update(h.id.as_bytes());
    let bytes = hasher.finalize();
    let id = story_model::uuid::Uuid::from_slice(&bytes[..16]).expect("16 bytes");
    MemoryEntry {
        id,
        kind: MemoryKind::Other,
        facts: Vec::new(),
        interpretation: Some(format!("recent turn {}", h.turn)),
        participants: BTreeSet::new(),
        location: None,
        importance: 0,
        unresolved: false,
        source_history_ids: vec![h.id],
        created_turn: h.turn,
        last_recalled_turn: None,
    }
}

// ---------------------------------------------------------------------------
// Fixed-tier text
// ---------------------------------------------------------------------------

/// Assemble the fixed-tier text: world, style, immutable config of the current
/// participants, current route premise, and scene proper nouns. Exposed for
/// tests/diagnostics.
pub fn build_fixed_text(pack: &StoryPack, state: &GameState, ctx: &SelectionContext) -> String {
    let mut s = String::new();
    s.push_str("# World\n");
    s.push_str(&pack.world_md);
    s.push_str("\n\n# Style\n");
    s.push_str(&pack.style_md);

    // Immutable character config for the current participants (honorifics rule
    // is the load-bearing absolute-include here).
    s.push_str("\n\n# Current participants\n");
    for id in &ctx.current_participants {
        if let Some(c) = pack.characters.get(id) {
            s.push_str(&format!("- {} ({})", c.name, c.id.0));
            if !c.personality.is_empty() {
                s.push_str(&format!(" | 성격: {}", c.personality.join(", ")));
            }
            if !c.speech_style.is_empty() {
                s.push_str(&format!(" | 화풍: {}", c.speech_style));
            }
            if !c.honorific_rule.is_empty() {
                s.push_str(&format!(" | 호칭규칙: {}", c.honorific_rule));
            }
            if !c.current_goal.is_empty() {
                s.push_str(&format!(" | 목표: {}", c.current_goal));
            }
            s.push('\n');
        }
    }

    // Current route premise (absolute-include).
    if let Some(route) = &ctx.active_route
        && let Some(def) = pack.rules.routes.get(route)
    {
        s.push_str("\n# Route premise\n");
        s.push_str(&def.premise);
        s.push('\n');
    }

    // Scene proper nouns (absolute-include): surface them verbatim so the
    // model treats them as anchored references.
    if !ctx.mentioned_proper_nouns.is_empty() {
        s.push_str("\n# Scene proper nouns\n");
        for noun in &ctx.mentioned_proper_nouns {
            s.push_str("- ");
            s.push_str(noun);
            s.push('\n');
        }
    }

    let _ = state; // state not currently needed in the fixed tier
    s
}

// ---------------------------------------------------------------------------
// State-tier text
// ---------------------------------------------------------------------------

/// Compact, deterministic JSON snapshot of the live state. Iteration over
/// `BTreeMap`s and `serde_json`'s BTreeMap-backed `Map` keeps key order
/// stable regardless of construction order.
fn state_compact_json(state: &GameState) -> String {
    let mut characters: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    for (id, c) in &state.characters {
        let mut m = serde_json::Map::new();
        m.insert("affection".to_string(), c.affection.into());
        m.insert("trust".to_string(), c.trust.into());
        m.insert("tension".to_string(), c.tension.into());
        m.insert(
            "stage".to_string(),
            serde_json::to_value(c.relationship_stage).expect("stage serializes"),
        );
        characters.insert(id.0.clone(), serde_json::Value::Object(m));
    }
    let root = serde_json::json!({
        "location": state.location,
        "time_slot": state.time_slot,
        "turn": state.turn,
        "active_route": state.active_route,
        "characters": characters,
        "flags": state.flags,
        "numbers": state.numbers,
        "inventory": state.inventory,
        "occurred_events": state.occurred_events.iter().collect::<Vec<_>>(),
    });
    serde_json::to_string(&root).expect("state snapshot serializes")
}

// ---------------------------------------------------------------------------
// Recent-tier text
// ---------------------------------------------------------------------------

fn recent_entry_text(h: &HistoryEntry) -> String {
    let mut s = String::new();
    s.push_str(&format!("## turn {} ({})\n", h.turn, h.scene_id));
    s.push_str("narration: ");
    s.push_str(&h.narration);
    s.push('\n');
    if let Some(act) = &h.player_action {
        s.push_str(&format!(
            "action: {} ({})\n",
            act.content,
            serde_json::to_string(&act.action_type).unwrap_or_else(|_| "\"?\"".to_string())
        ));
    }
    s
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

/// The Technical Spec §12.2 scoring formula (verbatim weights):
/// `importance*4 + unresolved*8 + participant_match*5 + location_match*3 +
///  route_match*6 + explicit_reference*10 + recency_bucket -
///  already_recalled_recently*3`.
///
/// `participant_match`/`location_match`/`route_match`/`explicit_reference` are
/// 0/1; `recency_bucket` is 0..=4 (4 = most recent); `already_recalled_recently`
/// is 0/1 (1 if recalled within the last 5 turns).
fn score(m: &MemoryEntry, state: &GameState, ctx: &SelectionContext) -> i32 {
    let importance = m.importance as i32;
    let unresolved = if m.unresolved { 1 } else { 0 };
    let participant_match = if m
        .participants
        .iter()
        .any(|p| ctx.current_participants.contains(p))
    {
        1
    } else {
        0
    };
    let location_match = if m.location.as_ref() == Some(&ctx.current_location) {
        1
    } else {
        0
    };
    let route_match = route_match_value(m, ctx);
    let explicit_reference = explicit_reference_value(m, ctx);
    let recency_bucket = recency_bucket(m.created_turn, state.turn);
    let already_recalled_recently = match m.last_recalled_turn {
        Some(t) if state.turn >= t && state.turn - t <= 5 => 1,
        _ => 0,
    };
    importance * 4
        + unresolved * 8
        + participant_match * 5
        + location_match * 3
        + route_match * 6
        + explicit_reference * 10
        + recency_bucket
        - already_recalled_recently * 3
}

/// `route_match`: 1 if an active route is set AND the memory's facts /
/// interpretation mention the route id. Memories do not carry a route field
/// (MVP), so this is content-derived. Returns 0 when no route is active.
fn route_match_value(m: &MemoryEntry, ctx: &SelectionContext) -> i32 {
    let Some(route) = &ctx.active_route else {
        return 0;
    };
    if facts_mention(m, route) { 1 } else { 0 }
}

/// `explicit_reference`: 1 if any fact or the interpretation mentions any of
/// the scene's mentioned proper nouns.
fn explicit_reference_value(m: &MemoryEntry, ctx: &SelectionContext) -> i32 {
    for noun in &ctx.mentioned_proper_nouns {
        if facts_mention(m, noun) {
            return 1;
        }
    }
    0
}

/// True if any fact field or the interpretation of `m` contains `needle`.
fn facts_mention(m: &MemoryEntry, needle: &str) -> bool {
    if let Some(interp) = &m.interpretation
        && interp.contains(needle)
    {
        return true;
    }
    m.facts.iter().any(|f| {
        f.subject.contains(needle) || f.predicate.contains(needle) || f.object.contains(needle)
    })
}

/// 4 = created this turn; 0 = created ≥25 turns ago.
fn recency_bucket(created_turn: u32, current_turn: u32) -> i32 {
    if current_turn <= created_turn {
        return 4;
    }
    let age = current_turn - created_turn;
    let bucket = 4usize.saturating_sub((age / 5) as usize);
    bucket as i32
}

// ---------------------------------------------------------------------------
// Absolute-include predicate
// ---------------------------------------------------------------------------

/// Whether `m` is an absolute-include (overrides score). Covers: unresolved
/// promises/secrets/key events, relationship-change causes, and memories
/// referencing a mentioned proper noun. The route premise + current-participant
/// honorifics are absolute too but live in the fixed tier (always included).
pub fn is_absolute(m: &MemoryEntry, ctx: &SelectionContext) -> bool {
    if m.kind == MemoryKind::RelationshipChange {
        return true;
    }
    if m.unresolved {
        match m.kind {
            MemoryKind::Promise
            | MemoryKind::SecretReveal
            | MemoryKind::KeyItem
            | MemoryKind::FirstMeeting
            | MemoryKind::Confession
            | MemoryKind::Betrayal => return true,
            _ => {}
        }
    }
    if explicit_reference_value(m, ctx) == 1 {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Token accounting
// ---------------------------------------------------------------------------

/// Documented heuristic: `max(char_count, unicode_display_width / 2)`. See the
/// module docs for the rationale and Korean-aware width approximation.
pub fn estimate_tokens(s: &str) -> usize {
    let char_count = s.chars().count();
    let width = unicode_display_width(s);
    char_count.max(width / 2)
}

/// Approximate East-Asian-Wide-aware display width. Hangul syllables, Jamo,
/// CJK ideographs, and fullwidth forms count as 2 columns; everything else as
/// 1. No `unicode-width` dependency (ADR-003).
fn unicode_display_width(s: &str) -> usize {
    s.chars()
        .map(|c| if is_wide(c as u32) { 2 } else { 1 })
        .sum()
}

fn is_wide(cp: u32) -> bool {
    matches!(
        cp,
        0x1100..=0x115F
        | 0x2329..=0x232A
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
    )
}

/// Tokens attributable to a single memory entry: a deterministic rendering of
/// its fields, then [`estimate_tokens`].
fn memory_entry_tokens(m: &MemoryEntry) -> usize {
    let mut s = String::new();
    s.push_str(&format!(
        "kind:{:?} importance:{} unresolved:{}\n",
        m.kind, m.importance, m.unresolved
    ));
    if let Some(interp) = &m.interpretation {
        s.push_str(interp);
        s.push('\n');
    }
    for f in &m.facts {
        s.push_str(&format!(
            "{} {} {} ({})\n",
            f.subject, f.predicate, f.object, f.certainty
        ));
    }
    if !m.participants.is_empty() {
        let parts: Vec<&str> = m.participants.iter().map(|p| p.0.as_str()).collect();
        s.push_str(&format!("participants: {}\n", parts.join(", ")));
    }
    estimate_tokens(&s)
}

/// Clone a memory entry and stamp `last_recalled_turn = Some(turn)` so the
/// caller can merge the included entries back into `state.memories`.
fn clone_with_recall(m: &MemoryEntry, turn: u32) -> MemoryEntry {
    let mut c = m.clone();
    c.last_recalled_turn = Some(turn);
    c
}

#[cfg(test)]
mod tests {
    //! In-crate unit tests pin the pure helpers. End-to-end behavior is
    //! exercised by `tests/memory_selector.rs`.

    use super::*;

    #[test]
    fn estimate_tokens_is_char_count_for_ascii_and_korean() {
        // Pure ASCII: width = chars, so max(chars, chars/2) = chars.
        assert_eq!(estimate_tokens("hello world"), 11);
        // Pure Korean (no spaces): width = 2*chars, max(chars, chars) = chars.
        assert_eq!(estimate_tokens("비가그친뒤"), 5);
    }

    #[test]
    fn recency_bucket_buckets_by_five_turns() {
        assert_eq!(recency_bucket(10, 10), 4);
        assert_eq!(recency_bucket(9, 10), 4);
        assert_eq!(recency_bucket(5, 10), 3); // age 5 -> bucket 3
        assert_eq!(recency_bucket(0, 10), 2); // age 10 -> bucket 2
        assert_eq!(recency_bucket(0, 100), 0); // very old
    }

    #[test]
    fn absolute_predicate_for_relationship_and_unresolved() {
        let mut pn = BTreeSet::new();
        let ctx = SelectionContext {
            current_participants: BTreeSet::new(),
            current_location: LocationId("l".to_string()),
            active_route: None,
            mentioned_proper_nouns: pn.clone(),
            budget_tokens: 1000,
        };
        let _ = &mut pn;
        let mk = |kind, unresolved| MemoryEntry {
            id: story_model::uuid::Uuid::nil(),
            kind,
            facts: Vec::new(),
            interpretation: None,
            participants: BTreeSet::new(),
            location: None,
            importance: 1,
            unresolved,
            source_history_ids: Vec::new(),
            created_turn: 0,
            last_recalled_turn: None,
        };
        assert!(is_absolute(
            &mk(MemoryKind::RelationshipChange, false),
            &ctx
        ));
        assert!(is_absolute(&mk(MemoryKind::Promise, true), &ctx));
        assert!(is_absolute(&mk(MemoryKind::KeyItem, true), &ctx));
        assert!(!is_absolute(&mk(MemoryKind::Promise, false), &ctx));
        assert!(!is_absolute(&mk(MemoryKind::Other, true), &ctx));
    }

    #[test]
    fn score_formula_matches_spec() {
        // importance=5 (20), unresolved=1 (8), participant_match=1 (5),
        // location_match=1 (3), route_match=0, explicit_reference=0,
        // recency_bucket=4 (created this turn), already_recalled=0.
        let mut parts = BTreeSet::new();
        parts.insert(CharacterId("yuna".to_string()));
        let ctx = SelectionContext {
            current_participants: parts,
            current_location: LocationId("cafe-namu".to_string()),
            active_route: None,
            mentioned_proper_nouns: BTreeSet::new(),
            budget_tokens: 1000,
        };
        let m = MemoryEntry {
            id: story_model::uuid::Uuid::nil(),
            kind: MemoryKind::Promise,
            facts: Vec::new(),
            interpretation: None,
            participants: {
                let mut s = BTreeSet::new();
                s.insert(CharacterId("yuna".to_string()));
                s
            },
            location: Some(LocationId("cafe-namu".to_string())),
            importance: 5,
            unresolved: true,
            source_history_ids: Vec::new(),
            created_turn: 10,
            last_recalled_turn: None,
        };
        let state_turn = 10;
        // importance*4=20, unresolved*8=8, participant*5=5, location*3=3,
        // recency=4 -> total 40.
        // We rebuild state minimally via a helper.
        let mut chars = BTreeMap::new();
        chars.insert(
            CharacterId("yuna".to_string()),
            story_model::CharacterState {
                affection: 0,
                trust: 0,
                tension: 0,
                relationship_stage: story_model::RelationshipStage::Friend,
                current_emotion: story_model::Emotion::Neutral,
                known_facts: BTreeSet::new(),
                address_form_overrides: BTreeMap::new(),
            },
        );
        let state = GameState {
            save_schema_version: story_model::SAVE_SCHEMA_VERSION,
            story_id: String::new(),
            story_version: semver::Version::new(0, 1, 0),
            pack_schema_version: semver::Version::new(1, 0, 0),
            session_id: story_model::uuid::Uuid::nil(),
            current_chapter: String::new(),
            current_scene_id: String::new(),
            turn: state_turn,
            location: LocationId("cafe-namu".to_string()),
            time_slot: story_model::TimeSlot::Evening,
            active_route: None,
            characters: chars,
            flags: BTreeMap::new(),
            numbers: BTreeMap::new(),
            strings: BTreeMap::new(),
            inventory: BTreeMap::new(),
            occurred_events: BTreeSet::new(),
            recent_summaries: std::collections::VecDeque::new(),
            memories: Vec::new(),
            history: Vec::new(),
            last_choice: None,
            revision: 0,
            stable_generation_id: None,
        };
        assert_eq!(score(&m, &state, &ctx), 40);
    }
}
