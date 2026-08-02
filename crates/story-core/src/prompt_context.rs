//! Deterministic prompt assembly (ST-018).
//!
//! [`build_prompt`] turns a [`StoryPack`] + [`GameState`] + recent history into
//! a [`PromptBundle`] (system, user, SHA-256 hash). The output is a pure
//! function of its inputs: same pack/state/history ⇒ byte-identical system and
//! user strings ⇒ identical hash, regardless of the order in which the caller
//! built the underlying `BTreeMap`s. This is the contract the snapshot/golden
//! tests pin down.
//!
//! # Ordering rules
//!
//! - Characters, locations, flags, numbers, inventory, and strings are all
//!   stored as `BTreeMap`/`BTreeSet`; iteration is key order.
//! - Recent history is iterated in slice (turn) order.
//! - The state snapshot is serialized via `serde_json` whose default `Map` is
//!   `BTreeMap`-backed, so its keys are alphabetical — independent of the order
//!   in which the snapshot object was constructed.
//!
//! # ADR-003
//!
//! `sha2` is a pure-Rust crypto crate (no IO/UI/HTTP). Adding it to `story-core`
//! does not violate ADR-003, which forbids only `ratatui`/`crossterm`/`reqwest`
//! (and FS). `check-core-deps.sh` continues to pass.

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};
use story_model::serde_json;
use story_model::{Choice, GameState, HistoryEntry, MemoryEntry, ParsedAction, StoryPack};

use crate::memory_selector::{SelectionContext, select};

/// Keep the immediately-played context crisp. Longer-lived information comes
/// back through the scored memory recall below instead of growing forever.
const RECENT_TURN_WINDOW: usize = 4;
/// A deliberately conservative slice of the provider context for recalled
/// memories. The provider still applies its final context bound afterwards.
const MEMORY_RECALL_BUDGET_TOKENS: u32 = 6_000;

/// The assembled prompt: system + user strings plus the canonical hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptBundle {
    /// System prompt (role/safety/output-contract + allowed IDs + rules).
    pub system: String,
    /// User prompt (world/style/opening + state snapshot + recent turns).
    pub user: String,
    /// Lowercase-hex SHA-256 of `system + "\n\n---\n\n" + user`.
    pub hash: String,
}

/// Assemble a deterministic [`PromptBundle`] for one generation call.
///
/// Pure function of (`pack`, `state`, `recent`, `player_action`). When
/// `player_action` is `Some`, the system prompt gets an extra player-
/// sovereignty reinforcement and the user prompt carries the parsed action
/// (with `original_text` verbatim). See the module docs for the ordering rules
/// that make it deterministic and construction-order-independent.
pub fn build_prompt(
    pack: &StoryPack,
    state: &GameState,
    recent: &[HistoryEntry],
    player_action: Option<&ParsedAction>,
) -> PromptBundle {
    build_prompt_with_choice(pack, state, recent, player_action, None)
}

/// As [`build_prompt`], but includes the numbered choice the player just
/// selected. A normal choice is not represented by `ParsedAction`, so omitting
/// it makes successive turns look identical to the model.
pub fn build_prompt_with_choice(
    pack: &StoryPack,
    state: &GameState,
    recent: &[HistoryEntry],
    player_action: Option<&ParsedAction>,
    player_choice: Option<&Choice>,
) -> PromptBundle {
    let system = build_system(pack, player_action);
    let user = build_user(pack, state, recent, player_action, player_choice);

    let mut hasher = Sha256::new();
    hasher.update(system.as_bytes());
    hasher.update(b"\n\n---\n\n");
    hasher.update(user.as_bytes());
    let hash = hex_lower(hasher.finalize().as_slice());

    PromptBundle { system, user, hash }
}

// ---------------------------------------------------------------------------
// system prompt
// ---------------------------------------------------------------------------

fn build_system(pack: &StoryPack, player_action: Option<&ParsedAction>) -> String {
    let mut s = String::new();

    // Role / safety / output contract from the Pack.
    s.push_str(&pack.prompts.system);
    s.push_str("\n\n");

    // Allowed IDs — all iterated in BTreeMap/BTreeSet key order.
    s.push_str("# Allowed IDs\n");
    s.push_str("## Characters\n");
    for id in pack.characters.keys() {
        s.push_str("- ");
        s.push_str(&id.0);
        s.push('\n');
    }
    s.push_str("## Locations\n");
    for id in pack.rules.locations.keys() {
        s.push_str("- ");
        s.push_str(&id.0);
        s.push('\n');
    }
    s.push_str("## Flags\n");
    for id in &pack.rules.allowed_flags {
        s.push_str("- ");
        s.push_str(id);
        s.push('\n');
    }

    // Per-turn delta limits.
    let dl = &pack.rules.delta_limits;
    s.push_str(&format!(
        "\n# Delta limits (per turn, inclusive)\n\
         affection: {a_min}..={a_max}\n\
         trust: {t_min}..={t_max}\n\
         tension: {n_min}..={n_max}\n",
        a_min = dl.affection.min,
        a_max = dl.affection.max,
        t_min = dl.trust.min,
        t_max = dl.trust.max,
        n_min = dl.tension.min,
        n_max = dl.tension.max,
    ));
    s.push_str(
        "\n# Relationship state update policy\n\
         After each scene, inspect the observable consequence for every involved character. \
         When the scene credibly changes warmth, reliability, or immediate strain, put a small \
         signed integer in `state_patch.affection_delta`, `trust_delta`, or `tension_delta` for \
         that character. Do not leave all three maps empty merely because the scene is subtle. \
         Use `{}` only when no observable relationship state changed. Never invent a change just \
         to make a meter move; narration and the patch must describe the same consequence.\n",
    );

    // Choice count.
    s.push_str(&format!(
        "# Choices\noffer exactly {} choices\n",
        pack.default_choice_count()
    ));

    // Player-sovereignty rule (fixed text).
    s.push_str(
        "\n# Player sovereignty\n\
         Do not fix emotions, dialogue, physical contact, forgiveness, confessions, or \
         decisions the player did not specify. Do not invert or shrink the core intent of \
         the player's free-text input.\n",
    );
    s.push_str(
        "\n# Speaker clarity\n\
         Narration is written from a second-person camera: `you` refers only to the player. \
         Never invent a direct quote, inner thought, or spoken reply for the player. The UI \
         already records the player's selected action separately. Put every NPC direct quote on \
         its own line in the exact visible form `Character name: \"dialogue\"` (or the \
         Korean equivalent punctuation). Use an established character name and never invent a \
         new speaker. This is a writing and display rule, not a JSON field.\n",
    );
    s.push_str(
        "\n# Working and display language\n\
         You may use English internally for analysis, retrieval keywords, and planning when it \
         improves understanding. Do not expose that working text or a chain of thought. Every \
         player-visible string in the JSON response, including narration and choice labels, must \
         be natural Korean unless the story content itself requires a foreign proper noun.\n",
    );
    s.push_str(
        "\n# Continuation and variation\n\
         Continue from the current state and the recent turns. Do not restart the opening. \
         Do not copy an exact sentence or repeat the same sensory description from a recent \
         narration. Advance only one small, observable beat in this turn.\n",
    );

    // When this turn originates from a free-text action, reinforce the
    // sovereignty clause in Korean right where the model reads its role.
    if player_action.is_some() {
        s.push_str(
            "\n# Free-text action sovereignty\n\
             이 입력은 플레이어의 의도다. 감정이나 행동을 임의로 확정/왜곡하지 말 것. \
             플레이어가 명시하지 않은 사랑·용서·접촉·고백을 첨가하지 말 것.\n",
        );
    }

    s
}

// ---------------------------------------------------------------------------
// user prompt
// ---------------------------------------------------------------------------

fn build_user(
    pack: &StoryPack,
    state: &GameState,
    recent: &[HistoryEntry],
    player_action: Option<&ParsedAction>,
    player_choice: Option<&Choice>,
) -> String {
    let mut s = String::new();
    let full = recent;
    let recent_start = full.len().saturating_sub(RECENT_TURN_WINDOW);
    // `earlier` is the narrative arc before the crisp recent window. It is
    // rendered below as a compact recap so the model keeps long-term continuity
    // even when event-driven memory recall is empty (the common case).
    let earlier = &full[..recent_start];
    let recent = &full[recent_start..];
    let recall = select(
        state,
        pack,
        // Recent turns are rendered below as a fixed, small window. Passing
        // them to the selector too would let their intentionally higher score
        // consume the entire recall budget before any long-term memory could
        // be selected.
        &[],
        &SelectionContext {
            current_participants: state.characters.keys().cloned().collect(),
            current_location: state.location.clone(),
            active_route: state.active_route.clone(),
            mentioned_proper_nouns: mentioned_proper_nouns(state, player_action, player_choice),
            budget_tokens: MEMORY_RECALL_BUDGET_TOKENS,
        },
    );

    s.push_str("# World\n");
    s.push_str(&pack.world_md);
    s.push_str("\n\n# Style\n");
    s.push_str(&pack.style_md);
    if recent.is_empty() {
        s.push_str("\n\n# Opening (first turn only)\n");
        s.push_str(&pack.opening_md);
    } else {
        s.push_str(
            "\n\n# Opening reference\n\
             The opening has already been shown. Do not reproduce its prose; continue from \
             the current state and recent turns below.\n",
        );
    }

    // Long-term continuity: a compact recap of every turn older than the recent
    // window. Without this the model only sees the last few turns plus state
    // numbers, and tends to repeat or regress earlier beats — especially weaker
    // local models. The recap is bounded; the provider applies its final context
    // bound afterwards.
    s.push_str(&earlier_recap(earlier));

    s.push_str("\n\n# Current state\n");
    s.push_str(&state_snapshot_json(state));

    s.push_str("\n\n# Relationship ledger (engine-owned)\n");
    s.push_str(&relationship_ledger(state));

    s.push_str("\n# Current dramatic beat\n");
    s.push_str(&dramatic_brief(state, &recall.included));
    s.push_str("\n# Autonomous world and NPC initiative\n");
    s.push_str(&world_initiative(pack, state));

    s.push_str("\n# Recent turns\n");
    if recent.is_empty() {
        s.push_str("(none)\n");
    }
    for h in recent {
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
    }

    // Compact semantic digest of the recent arc (model interpretation when
    // stored, else narration first clause) plus an safe repetition flag.
    // Replaces the earlier surface-level anti-repeat phrase list — strictly
    // richer.
    s.push_str(&recent_arc_block(state, full));

    if !recall.included.is_empty() {
        s.push_str("\n# Recalled long-term memory\n");
        s.push_str(&memory_context(&recall.included));
    }

    s.push_str("\n# Choice roles for this turn\n");
    s.push_str(&choice_role_brief(state));

    if let Some(choice) = player_choice {
        s.push_str("\n# Player selected choice\n");
        s.push_str(&format!("id: {}\n", choice.id));
        s.push_str(&format!("label: {}\n", choice.label));
        s.push_str(&format!("intent: {}\n", choice.intent));
        if let Some(tone) = &choice.tone {
            s.push_str(&format!("tone: {tone}\n"));
        }
        if let Some(target) = &choice.target {
            s.push_str(&format!("target: {}\n", target.0));
        }
    }

    // When this turn originates from a free-text action, surface the parsed
    // intent (verbatim `original_text` + classification) to the model. The
    // sovereignty rule lives in the system prompt; this block only conveys the
    // player's input without interpreting it.
    if let Some(pa) = player_action {
        s.push_str("\n# Player free action\n");
        s.push_str(&format!(
            "action_type: {}\n",
            serde_json::to_string(&pa.action_type).unwrap_or_else(|_| "\"?\"".to_string())
        ));
        if let Some(t) = &pa.target {
            s.push_str(&format!("target: {}\n", t.0));
        }
        if let Some(c) = pa.confidence {
            s.push_str(&format!("confidence: {c:.2}\n"));
        }
        s.push_str(&format!("content: {}\n", pa.content));
        s.push_str(&format!("original_text: {}\n", pa.original_text));
    }

    s
}

fn mentioned_proper_nouns(
    state: &GameState,
    player_action: Option<&ParsedAction>,
    player_choice: Option<&Choice>,
) -> BTreeSet<String> {
    let mut nouns = BTreeSet::new();
    if let Some(last) = &state.last_choice {
        nouns.insert(last.label.clone());
    }
    if let Some(action) = player_action {
        nouns.insert(action.content.clone());
        nouns.insert(action.original_text.clone());
    }
    if let Some(choice) = player_choice {
        nouns.insert(choice.label.clone());
        nouns.insert(choice.intent.clone());
        if let Some(target) = &choice.target {
            nouns.insert(target.0.clone());
        }
    }
    nouns.retain(|noun| !noun.trim().is_empty() && noun.chars().count() <= 80);
    nouns
}

fn relationship_ledger(state: &GameState) -> String {
    let mut ledger = String::new();
    for (id, character) in &state.characters {
        ledger.push_str(&format!(
            "- {}: stage={:?}; affection={}; trust={}; tension={}; emotion={:?}",
            id.0,
            character.relationship_stage,
            character.affection,
            character.trust,
            character.tension,
            character.current_emotion,
        ));
        if !character.address_form_overrides.is_empty() {
            let forms = character
                .address_form_overrides
                .iter()
                .map(|(target, form)| format!("{}={}", target.0, form))
                .collect::<Vec<_>>();
            ledger.push_str(&format!("; address_forms={}", forms.join(", ")));
        }
        ledger.push('\n');
    }
    if ledger.is_empty() {
        ledger.push_str("(no active character relationship state)\n");
    }
    ledger
}

fn dramatic_brief(state: &GameState, recalled: &[MemoryEntry]) -> String {
    let unresolved = recalled.iter().find(|memory| memory.unresolved);
    let mode = if state.turn % 3 == 2 {
        "Sequel"
    } else {
        "Scene"
    };
    match unresolved.and_then(memory_summary) {
        Some(thread) => format!(
            "mode={mode}. Advance exactly one observable beat around this unresolved thread: {thread}. Do not resolve it completely unless a validated event makes that possible. End with a concrete new pressure, question, or consequence.\n"
        ),
        None => format!(
            "mode={mode}. Advance exactly one observable relationship or situation beat at {}. Do not restart the premise or repeat the last selected approach. End with a concrete new pressure, question, or consequence.\n",
            state.location.0,
        ),
    }
}

fn world_initiative(pack: &StoryPack, state: &GameState) -> String {
    let phase = crate::narrative::phase(state);
    let pressure =
        crate::narrative::pressure(state).unwrap_or("move the current situation forward");
    let mut text = format!(
        "World clock: turn {} is in the {phase} phase; the next pressure is due by turn {}: {pressure}. Before offering choices, let the setting or an NPC take one observable initiative; do not wait for the player to invent all momentum.\n",
        state.turn,
        crate::narrative::due_turn(state)
    );
    for (id, character) in &pack.characters {
        if state.characters.contains_key(id) && !character.current_goal.trim().is_empty() {
            let agenda = crate::narrative::agenda(state, &id.0).unwrap_or(&character.current_goal);
            text.push_str(&format!("- {} pursues: {}\n", character.name, agenda));
        }
    }
    text.push_str("NPC initiative may create pressure, offer, refusal, departure, or new information, but must not decide the player's response, feelings, speech, or irreversible commitment.\n");
    text
}

fn choice_role_brief(state: &GameState) -> String {
    const ROLES: [[&str; 3]; 3] = [
        [
            "relationship move: directly ask, set a boundary, or offer something that changes interpersonal distance",
            "evidence move: observe, verify, or investigate a concrete clue without forcing a confession",
            "situation move: change the immediate situation through movement, delay, risk, or a third factor",
        ],
        [
            "evidence move: observe, verify, or investigate a concrete clue without forcing a confession",
            "situation move: change the immediate situation through movement, delay, risk, or a third factor",
            "relationship move: directly ask, set a boundary, or offer something that changes interpersonal distance",
        ],
        [
            "situation move: change the immediate situation through movement, delay, risk, or a third factor",
            "relationship move: directly ask, set a boundary, or offer something that changes interpersonal distance",
            "evidence move: observe, verify, or investigate a concrete clue without forcing a confession",
        ],
    ];
    let mut roles = ROLES[(state.turn % ROLES.len() as u32) as usize].to_vec();
    let recent = crate::narrative::recent_choice_roles(state);
    // This is a preference, never a validation gate: a story can legitimately
    // revisit a relationship move when the state demands it.
    if let Some(last_role) = recent.last() {
        roles.sort_by_key(|role| {
            if role.starts_with(last_role.as_str()) {
                1
            } else {
                0
            }
        });
    }
    format!(
        "Return exactly three choices in this order:\n1. {}\n2. {}\n3. {}\nThe three choices must cause different next-scene goals or consequences, not merely different wording. Treat recent chosen directions ({}) as a soft anti-repeat signal; avoid re-offering the player's last selected label: {}\n",
        roles[0],
        roles[1],
        roles[2],
        if recent.is_empty() {
            "none".to_string()
        } else {
            recent.join(", ")
        },
        state
            .last_choice
            .as_ref()
            .map(|choice| choice.label.as_str())
            .unwrap_or("(none)"),
    )
}

fn memory_context(memories: &[MemoryEntry]) -> String {
    let mut text = String::new();
    for memory in memories {
        text.push_str("- ");
        text.push_str(&format!("{:?}", memory.kind));
        if memory.unresolved {
            text.push_str(" [unresolved]");
        }
        if let Some(summary) = memory_summary(memory) {
            text.push_str(": ");
            text.push_str(summary);
        }
        text.push('\n');
    }
    text
}

fn memory_summary(memory: &MemoryEntry) -> Option<&str> {
    memory
        .interpretation
        .as_deref()
        .filter(|text| !text.trim().is_empty())
        .or_else(|| memory.facts.first().map(|fact| fact.object.as_str()))
}

/// Cap the number of earlier turns rendered, so a long session cannot grow the
/// recap without bound. The most recent earlier turns matter most for immediate
/// continuity; the engine's state snapshot persists older consequences.
const EARLIER_RECAP_MAX_ENTRIES: usize = 12;
/// Per-entry narration cap. The recap is continuity, not replay: one beat per
/// earlier turn is enough to prevent regression/repetition.
const EARLIER_RECAP_PER_NARRATION: usize = 160;

/// Render a compact, bounded recap of the turns before the recent window.
/// Returns an empty string when there is nothing earlier (keeps the prompt
/// byte-identical for short sessions, so the golden snapshot is unaffected).
fn earlier_recap(earlier: &[HistoryEntry]) -> String {
    if earlier.is_empty() {
        return String::new();
    }
    let start = earlier.len().saturating_sub(EARLIER_RECAP_MAX_ENTRIES);
    let mut s = String::from(
        "\n\n# Earlier story (recap — continuity only; do not reproduce these lines)\n",
    );
    if start > 0 {
        // Older turns are omitted; flag it so the model knows the arc is longer.
        s.push_str(&format!(
            "(turns before {} are omitted; rely on the current state for their consequences)\n",
            earlier[start].turn
        ));
    }
    for h in &earlier[start..] {
        s.push_str(&format!("## turn {} ({})\n", h.turn, h.scene_id));
        s.push_str(&truncate_prose(&h.narration, EARLIER_RECAP_PER_NARRATION));
        s.push('\n');
        if let Some(act) = &h.player_action {
            s.push_str(&format!("action: {}\n", act.content));
        }
    }
    s
}

/// Trim prose to at most `max_chars` characters (char-based, UTF-8 safe),
/// appending an ellipsis when truncated. Whitespace is collapsed for a stable
/// single-line beat.
fn truncate_prose(text: &str, max_chars: usize) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = collapsed.chars().count();
    if count <= max_chars {
        return collapsed;
    }
    let head: String = collapsed.chars().take(max_chars).collect();
    format!("{head}…")
}

/// Number of most-recent digests rendered in the `# Recent arc` block. Matches
/// the full-prose recent window so each of those turns gets a one-line semantic
/// companion.
const RECENT_ARC_RENDER_TURNS: usize = 4;

/// Render `# Recent arc` (last [`RECENT_ARC_RENDER_TURNS`] digests) and, only
/// when a motif is shared across ≥2 of the last [`REPETITION_DETECT_TURNS`]
/// digests, a `# 반복 감지` note. Returns an empty string when there is no
/// history (keeps byte-identical prompts for the very first turn).
fn recent_arc_block(state: &GameState, full: &[HistoryEntry]) -> String {
    if full.is_empty() {
        return String::new();
    }
    let stored = &state.recent_summaries;
    let aligned = stored.len() == full.len();
    // Digest for the i-th full-history turn: the stored summary when aligned
    // and non-empty, else the narration's first clause (covers old saves whose
    // `recent_summaries` is empty).
    let digest_for = |i: usize| -> String {
        let stored_entry = if aligned { stored.get(i) } else { None };
        if let Some(entry) = stored_entry {
            let trimmed = entry.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        first_clause(&full[i].narration, 120)
    };

    let mut s = String::from("\n# Recent arc\n");
    let render_start = full.len().saturating_sub(RECENT_ARC_RENDER_TURNS);
    for (i, entry) in full.iter().enumerate().skip(render_start) {
        s.push_str(&format!("## turn {} ({})\n", entry.turn, entry.scene_id));
        s.push_str(&digest_for(i));
        s.push('\n');
    }

    // Repetition check over a wider window than the rendered arc.
    let detect_start = full.len().saturating_sub(REPETITION_DETECT_TURNS);
    let digests: Vec<String> = (detect_start..full.len()).map(digest_for).collect();
    let refs: Vec<&str> = digests.iter().map(String::as_str).collect();
    let motifs = shared_motifs(&refs);
    if !motifs.is_empty() {
        s.push_str(&format!(
            "\n# 반복 감지\n\
             최근 흐름에서 이 표현/소재가 반복되고 있다: {}.\n\
             같은 분위기·같은 묘사로 되풀이하지 말고, 한 가지 새로운 관찰 가능한 압력이나 방향으로 전진시킨다.\n",
            motifs.join(", ")
        ));
    }
    s
}

/// Extract the opening clause of a narration: up to the first sentence ender
/// (`.`, `。`, `!`, `?`, or a newline), capped at `max_chars` with an ellipsis.
pub fn first_clause(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let end = trimmed
        .find(['.', '。', '!', '?', '\n'])
        .unwrap_or(trimmed.len());
    let clause = trimmed[..end].trim();
    let count = clause.chars().count();
    if count == 0 {
        return String::new();
    }
    if count <= max_chars {
        return clause.to_string();
    }
    let head: String = clause.chars().take(max_chars).collect();
    format!("{head}…")
}

/// Number of recent digests scanned for repetition. Wider than the rendered arc
/// so a motif that started just outside the prose window is still caught.
const REPETITION_DETECT_TURNS: usize = 6;
const REPETITION_MAX_KEYWORDS: usize = 5;
/// Korean glue words / verb endings to ignore. Korean morphology is not
/// analyzed; this small list plus the "shared across ≥2 turns" bar keeps noise
/// low — the model still has the arc digests themselves to judge from.
const REPETITION_STOPWORDS: &[&str] = &[
    "했다", "있다", "없다", "한다", "하는", "하고", "위해", "그", "이", "저", "등", "및", "수",
    "것", "때", "임", "됨",
];
/// Trailing particles stripped so inflected forms cluster (유나가/유나는 → 유나).
/// Tried in order; the first match wins (`strip_suffix` is applied once).
const REPETITION_PARTICLES: &[&str] = &[
    "들은", "이가", "는", "가", "은", "이", "을", "를", "에", "의", "도", "와", "과", "로",
];

/// Normalize one whitespace token into a comparable content token, or `None`
/// when it is a stopword or non-alphanumeric. A length floor applies only to
/// ASCII tokens: a single Hangul syllable (비, 눈, 밤) is a legitimate motif.
fn normalize_token(word: &str) -> Option<String> {
    let trimmed = word.trim_matches(|c: char| !c.is_alphanumeric());
    let mut w = trimmed.to_string();
    for particle in REPETITION_PARTICLES {
        if let Some(stripped) = w.strip_suffix(particle) {
            w = stripped.to_string();
            break;
        }
    }
    let w = w
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_string();
    if (w.is_ascii() && w.chars().count() < 2) || REPETITION_STOPWORDS.contains(&w.as_str()) {
        return None;
    }
    Some(w.to_lowercase())
}

/// Return content tokens that appear in at least 2 of `digests` (document
/// frequency, not raw count), sorted and capped. Empty when nothing repeats.
fn shared_motifs(digests: &[&str]) -> Vec<String> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut document_frequency: BTreeMap<String, usize> = BTreeMap::new();
    for digest in digests {
        let seen: BTreeSet<String> = digest
            .split_whitespace()
            .filter_map(normalize_token)
            .collect();
        for token in seen {
            *document_frequency.entry(token).or_default() += 1;
        }
    }
    let mut out: Vec<String> = document_frequency
        .into_iter()
        .filter(|(_, count)| *count >= 2)
        .map(|(token, _)| token)
        .collect();
    out.sort_unstable();
    out.truncate(REPETITION_MAX_KEYWORDS);
    out
}

/// Build a deterministic compact-JSON snapshot of the current state. Iteration
/// over `BTreeMap`s guarantees key order; `serde_json`'s default `Map` is also
/// `BTreeMap`-backed so the serialized keys are alphabetical regardless of the
/// order in which the snapshot object was assembled.
fn state_snapshot_json(state: &GameState) -> String {
    let mut characters: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    for (id, c) in &state.characters {
        let mut m = serde_json::Map::new();
        m.insert("affection".to_string(), c.affection.into());
        m.insert("trust".to_string(), c.trust.into());
        m.insert("tension".to_string(), c.tension.into());
        m.insert(
            "emotion".to_string(),
            serde_json::to_value(c.current_emotion).expect("emotion serializes"),
        );
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
        "characters": characters,
        "flags": state.flags,
        "numbers": state.numbers,
        "inventory": state.inventory,
    });
    serde_json::to_string(&root).expect("state snapshot serializes")
}

// ---------------------------------------------------------------------------
// hashing helpers
// ---------------------------------------------------------------------------

/// Lowercase-hex encode a byte slice without pulling in a `hex` crate.
fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    //! The hash-correctness check lives here (rather than in an integration
    //! test) because `sha2` is a regular dependency of this crate and is thus
    //! directly available in in-crate unit tests.

    use super::*;
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    use story_model::{
        CharacterState, Emotion, GameState, LocationId, MemoryEntry, MemoryKind, RelationshipStage,
        SAVE_SCHEMA_VERSION, StoryPack,
    };

    fn minimal_fixture() -> (StoryPack, GameState) {
        let pack = StoryPack {
            metadata: story_model::PackMetadata {
                schema_version: semver::Version::parse("1.0.0").unwrap(),
                story_id: "t".to_string(),
                story_version: semver::Version::new(0, 1, 0),
                min_engine_version: semver::Version::new(0, 1, 0),
                title: "t".to_string(),
                language: "ko-KR".to_string(),
                genres: vec![],
                rating: story_model::PackRating::General,
                all_major_characters_safe: true,
                default_choice_count: 3,
                initial_state: story_model::PackInitialState {
                    chapter: "c".to_string(),
                    scene: "s".to_string(),
                    location: LocationId("l".to_string()),
                    time_slot: story_model::TimeSlot::Morning,
                },
                content: story_model::PackContentPolicy {
                    safe_content: "none".to_string(),
                    violence: "none".to_string(),
                },
            },
            characters: BTreeMap::new(),
            rules: story_model::StoryRules::minimal(),
            world_md: "w".to_string(),
            style_md: "st".to_string(),
            opening_md: "o".to_string(),
            prompts: story_model::PromptFragments {
                system: "sys".to_string(),
                director: "d".to_string(),
                writer: "w".to_string(),
            },
            pack_root: std::path::PathBuf::from("/tmp"),
        };
        let state = GameState {
            save_schema_version: SAVE_SCHEMA_VERSION,
            story_id: "t".to_string(),
            story_version: semver::Version::new(0, 1, 0),
            pack_schema_version: semver::Version::new(1, 0, 0),
            session_id: story_model::uuid::Uuid::nil(),
            current_chapter: "c".to_string(),
            current_scene_id: "s".to_string(),
            turn: 0,
            location: LocationId("l".to_string()),
            time_slot: story_model::TimeSlot::Morning,
            active_route: None,
            characters: BTreeMap::from([(
                story_model::CharacterId("c-1".to_string()),
                CharacterState {
                    affection: 0,
                    trust: 0,
                    tension: 0,
                    relationship_stage: RelationshipStage::Stranger,
                    current_emotion: Emotion::Neutral,
                    known_facts: std::collections::BTreeSet::new(),
                    address_form_overrides: BTreeMap::new(),
                },
            )]),
            flags: BTreeMap::new(),
            numbers: BTreeMap::new(),
            strings: BTreeMap::new(),
            inventory: BTreeMap::new(),
            occurred_events: std::collections::BTreeSet::new(),
            recent_summaries: VecDeque::new(),
            memories: Vec::new(),
            history: Vec::new(),
            last_choice: None,
            revision: 0,
            stable_generation_id: None,
        };
        (pack, state)
    }

    #[test]
    fn hash_matches_manual_sha256_of_concatenation() {
        let (pack, state) = minimal_fixture();
        let bundle = build_prompt(&pack, &state, &[], None);
        // Recompute the expected hash independently.
        let expected_input = format!("{}\n\n---\n\n{}", bundle.system, bundle.user);
        let mut h = Sha256::new();
        h.update(expected_input.as_bytes());
        let expected: String = h
            .finalize()
            .as_slice()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(bundle.hash, expected);
    }

    #[test]
    fn build_prompt_unit_runs_minimally() {
        let (pack, state) = minimal_fixture();
        let b = build_prompt(&pack, &state, &[], None);
        assert!(!b.system.is_empty());
        assert!(!b.user.is_empty());
        assert_eq!(b.hash.len(), 64);
    }

    #[test]
    fn system_prompt_makes_npc_speakers_visible_without_adding_a_json_field() {
        let (pack, state) = minimal_fixture();
        let bundle = build_prompt(&pack, &state, &[], None);

        assert!(bundle.system.contains("# Speaker clarity"));
        assert!(bundle.system.contains("Character name: \"dialogue\""));
        assert!(bundle.system.contains("not a JSON field"));
        assert!(bundle.system.contains("# Working and display language"));
        assert!(bundle.system.contains("must be natural Korean"));
    }

    #[test]
    fn prompt_includes_recalled_memory_relationship_ledger_and_distinct_choice_roles() {
        let (pack, mut state) = minimal_fixture();
        state.memories.push(MemoryEntry {
            id: story_model::uuid::Uuid::nil(),
            kind: MemoryKind::Promise,
            facts: vec![],
            interpretation: Some("유나는 반송된 편지의 이유를 다음에 말하기로 했다".to_string()),
            participants: BTreeSet::new(),
            location: None,
            importance: 10,
            unresolved: true,
            source_history_ids: vec![],
            created_turn: 0,
            last_recalled_turn: None,
        });
        state.memories.push(MemoryEntry {
            id: story_model::uuid::Uuid::from_u128(1),
            kind: MemoryKind::Other,
            facts: vec![],
            interpretation: Some("유나는 오래된 재즈를 좋아한다".to_string()),
            participants: BTreeSet::from([story_model::CharacterId("c-1".to_string())]),
            location: None,
            importance: 9,
            unresolved: false,
            source_history_ids: vec![],
            created_turn: 0,
            last_recalled_turn: None,
        });
        state.last_choice = Some(story_model::ChoiceRef {
            choice_id: "choice-1".to_string(),
            label: "편지를 직접 묻는다".to_string(),
            turn: 0,
        });

        let bundle = build_prompt(&pack, &state, &[], None);

        assert!(bundle.user.contains("# Relationship ledger (engine-owned)"));
        assert!(bundle.user.contains("# Recalled long-term memory"));
        assert!(bundle.user.contains("반송된 편지의 이유"));
        assert!(bundle.user.contains("오래된 재즈를 좋아한다"));
        assert!(bundle.user.contains("# Current dramatic beat"));
        assert!(bundle.user.contains("# Choice roles for this turn"));
        assert!(bundle.user.contains("relationship move"));
        assert!(bundle.user.contains("evidence move"));
        assert!(bundle.user.contains("situation move"));
        assert!(bundle.user.contains("편지를 직접 묻는다"));
    }

    #[test]
    fn normalize_token_strips_particle_and_filters() {
        assert_eq!(normalize_token("유나가"), Some("유나".to_string()));
        assert_eq!(normalize_token("커피를"), Some("커피".to_string()));
        assert_eq!(normalize_token("비가"), Some("비".to_string()));
        assert_eq!(normalize_token("했다"), None); // stopword
        assert_eq!(normalize_token("…"), None); // non-alphanumeric
        assert_eq!(normalize_token("a"), None); // too short
    }

    #[test]
    fn shared_motifs_returns_tokens_in_two_or_more_digests() {
        let digests = ["유나가 커피를 내렸다", "커피가 식었다", "비가 창을 때렸다"];
        let m = shared_motifs(&digests);
        assert!(m.contains(&"커피".to_string()), "{m:?}");
        assert!(!m.contains(&"유나".to_string())); // only in 1 digest
    }

    #[test]
    fn shared_motifs_empty_when_nothing_repeats() {
        let digests = ["유나가 웃었다", "편지가 발견됐다", "문이 닫혔다"];
        assert!(shared_motifs(&digests).is_empty());
    }
}
