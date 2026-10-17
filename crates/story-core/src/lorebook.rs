//! Per-turn lorebook selection and prompt rendering (execution tiers T0–T2).
//!
//! Executed features: keyword matching (case-folded by default), constant
//! entries, `insertion_order`, `before_char`/`after_char` positions, selective
//! secondary-key logic, `scan_depth`, per-turn `probability`, a soft token
//! budget, and recursion (T2): activated entries' content re-scans for further
//! activations, gated by the book's `recursive_scanning` master switch, the
//! per-entry `prevent_recursion` (this entry triggers nothing) and
//! `exclude_recursion` (this entry cannot be triggered by other entries),
//! with a cycle guard and a bounded pass count.
//!
//! Placement policy — prompt-prefix stability: the user message deliberately
//! keeps a turn-invariant prefix (world/style/canon) so local runtimes can
//! reuse it across turns. Therefore **constant** entries render in the stable
//! prefix at their canonical `before_char`/`after_char` position, while
//! **keyword-triggered** entries all render in one turn-variable block right
//! after the canon block (`before_char` entries ordered first within it).
//! Splitting triggered entries back out into the prefix would invalidate the
//! cached prefix every turn for a placement nuance the two-message layout
//! already approximates.

use std::hash::{Hash, Hasher};

use story_model::{
    Choice, GameState, LorebookEntry, LorebookLogic, LorebookPosition, ParsedAction, StoryPack,
};

use crate::memory_selector::estimate_tokens;

/// The turn's lorebook selection, grouped by injection site.
#[derive(Debug, Default, Clone)]
pub struct SelectedLore {
    /// Constant entries injected before the character canon (stable prefix).
    pub stable_before: Vec<LorebookEntry>,
    /// Constant entries injected after the character canon (stable prefix).
    pub stable_after: Vec<LorebookEntry>,
    /// Keyword-triggered entries for this turn, one block after the canon.
    pub triggered: Vec<LorebookEntry>,
}

impl SelectedLore {
    /// `true` when nothing would be injected this turn.
    pub fn is_empty(&self) -> bool {
        self.stable_before.is_empty() && self.stable_after.is_empty() && self.triggered.is_empty()
    }
}

/// Maximum number of recursion re-scan passes after the direct scan. A
/// tight bound keeps pathological books linear and the selection
/// deterministic; chains deeper than this simply do not fire.
pub const MAX_RECURSION_PASSES: u32 = 3;

/// Lexical cosine threshold for `vectorized` entries to fire without a key
/// hit. Deliberately conservative — embeddings approximate, this approximates
/// the approximation.
pub const LEXICAL_ACTIVATION_THRESHOLD: f32 = 0.15;

/// Scan + select lorebook entries for this turn.
///
/// Deterministic: the same inputs always yield the same selection — the
/// probability roll is keyed on `(turn, entry id)` instead of an RNG, so a
/// retry of the same turn re-rolls identically.
pub fn select_lore(
    pack: &StoryPack,
    state: &GameState,
    history: &[story_model::HistoryEntry],
    player_action: Option<&ParsedAction>,
    player_choice: Option<&Choice>,
) -> SelectedLore {
    let book = &pack.lorebook;
    if book.is_empty() {
        return SelectedLore::default();
    }
    let corpus_raw = scan_corpus(
        state,
        history,
        book.scan_depth,
        player_action,
        player_choice,
    );
    let corpus_folded = corpus_raw.to_lowercase();

    let mut stable_before = Vec::new();
    let mut stable_after = Vec::new();
    let mut candidates: Vec<LorebookEntry> = Vec::new();
    // Entries still eligible for (re-)scanning, keyed by id. Constants never
    // re-scan; `exclude_recursion` entries drop out after the direct pass.
    // Removal on activation doubles as the per-turn cycle guard.
    let mut pending: std::collections::BTreeMap<&str, &LorebookEntry> = Default::default();
    // Recursion sources: content of everything active this turn whose
    // `prevent_recursion` flag does not opt it out.
    let mut recursion_corpus = String::new();

    for entry in &book.entries {
        if !entry.enabled {
            continue;
        }
        if entry.constant {
            match entry.position {
                LorebookPosition::BeforeChar => stable_before.push(entry.clone()),
                LorebookPosition::AfterChar => stable_after.push(entry.clone()),
            }
            if !entry.prevent_recursion {
                recursion_corpus.push_str(entry.content.trim());
                recursion_corpus.push('\n');
            }
            continue;
        }
        pending.insert(entry.id.as_str(), entry);
    }

    // Turn the selection is for: the reducer records activations for this
    // turn after building the request, so timers compare against `upcoming`.
    let upcoming = state.turn + 1;

    // Direct pass against the story corpus.
    let direct: Vec<&LorebookEntry> = pending.values().copied().collect();
    for entry in direct {
        // `delay > 0` entries only ever fire from a recursion pass.
        if entry.delay > 0 {
            continue;
        }
        // sticky keeps an entry force-included for its window; cooldown bars
        // re-triggering during the window after that.
        if sticky_window_active(
            &state.lorebook_activations,
            &entry.id,
            upcoming,
            entry.sticky,
        ) {
            pending.remove(entry.id.as_str());
            candidates.push(entry.clone());
            if !entry.prevent_recursion {
                recursion_corpus.push_str(entry.content.trim());
                recursion_corpus.push('\n');
            }
            continue;
        }
        if cooldown_window_active(
            &state.lorebook_activations,
            &entry.id,
            upcoming,
            entry.sticky + entry.cooldown,
        ) {
            pending.remove(entry.id.as_str());
            continue;
        }
        let keyword_hit = entry_matches(&corpus_raw, &corpus_folded, entry);
        if !keyword_hit
            && !(entry.vectorized
                && lexical_similarity(&corpus_raw, &entry.content) >= LEXICAL_ACTIVATION_THRESHOLD)
        {
            continue;
        }
        if entry.probability < 100 && probability_roll(state.turn, &entry.id) >= entry.probability {
            pending.remove(entry.id.as_str());
            continue;
        }
        candidates.push(entry.clone());
        if !entry.prevent_recursion {
            recursion_corpus.push_str(entry.content.trim());
            recursion_corpus.push('\n');
        }
        if entry.exclude_recursion {
            pending.remove(entry.id.as_str());
        }
    }
    // `exclude_recursion` entries that never fired directly are still barred
    // from the recursive passes — only the story corpus may activate them.
    pending.retain(|_, entry| !entry.exclude_recursion);

    // Recursion passes: each pass scans the remaining entries against the
    // corpus frozen at the previous pass's end (breadth-per-pass, so chains
    // grow by exactly one link per pass), until nothing new fires or the
    // pass budget runs out.
    if book.recursive_scanning {
        let mut recursion_folded = recursion_corpus.to_lowercase();
        for pass in 0..MAX_RECURSION_PASSES {
            if pending.is_empty() {
                break;
            }
            // `delay` counts 1-based recursion passes: an entry with delay = N
            // may first fire on pass N (never in the direct pass).
            let round: Vec<&LorebookEntry> = pending
                .values()
                .copied()
                .filter(|entry| entry.delay == 0 || entry.delay <= pass + 1)
                .collect();
            let mut fired_this_pass = false;
            let mut corpus_delta = String::new();
            for entry in round {
                if !entry_matches(&recursion_corpus, &recursion_folded, entry) {
                    continue;
                }
                pending.remove(entry.id.as_str());
                candidates.push(entry.clone());
                fired_this_pass = true;
                if !entry.prevent_recursion {
                    corpus_delta.push_str(entry.content.trim());
                    corpus_delta.push('\n');
                }
            }
            if !fired_this_pass {
                break;
            }
            recursion_corpus.push_str(&corpus_delta);
            recursion_folded = recursion_corpus.to_lowercase();
        }
    }

    // Budget admission: higher priority (lower value) first, ties by lower
    // insertion order; an entry that does not fit is dropped whole — the
    // provider's final bound must never be the one squeezing lore mid-text.
    candidates.sort_by_key(|entry| (entry.priority.unwrap_or(i64::MAX), entry.insertion_order));
    let mut triggered = Vec::new();
    let mut used = 0_usize;
    for entry in candidates {
        let cost = estimate_tokens(&entry.content);
        if used + cost > book.token_budget as usize {
            continue;
        }
        used += cost;
        triggered.push(entry);
    }

    // Inside the triggered block, before_char entries come first; each group
    // keeps its own insertion order.
    triggered.sort_by_key(|entry| (before_first(entry.position), entry.insertion_order));
    stable_before.sort_by_key(|entry| entry.insertion_order);
    stable_after.sort_by_key(|entry| entry.insertion_order);
    SelectedLore {
        stable_before,
        stable_after,
        triggered,
    }
}

/// Render one injection block: raw entry contents separated by blank lines,
/// with no per-entry headers — matching World Info's content-only convention
/// and keeping the prompt free of scaffolding the model would echo.
pub fn render_lore_block(entries: &[LorebookEntry]) -> String {
    let mut out = String::from("# Lore\n");
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(entry.content.trim());
        out.push('\n');
    }
    out
}

/// The text keyword scans run against: the last `scan_depth` narrations plus
/// the live player action and selected choice.
fn scan_corpus(
    state: &GameState,
    history: &[story_model::HistoryEntry],
    scan_depth: u32,
    player_action: Option<&ParsedAction>,
    player_choice: Option<&Choice>,
) -> String {
    let mut corpus = String::new();
    let start = history.len().saturating_sub(scan_depth as usize);
    for entry in &history[start..] {
        corpus.push_str(&entry.narration);
        corpus.push('\n');
    }
    if let Some(action) = player_action {
        corpus.push_str(&action.original_text);
        corpus.push('\n');
        if let Some(intent) = &action.intent {
            corpus.push_str(intent);
            corpus.push('\n');
        }
    }
    if let Some(choice) = player_choice {
        corpus.push_str(&choice.label);
        corpus.push('\n');
        corpus.push_str(&choice.intent);
        corpus.push('\n');
    }
    let _ = state; // reserved: scene/location id keys in a later tier
    corpus
}

/// Evaluate one entry against the corpus: any primary key, then the selective
/// secondary-key gate. An entry without secondary keys is primary-only.
fn entry_matches(corpus_raw: &str, corpus_folded: &str, entry: &LorebookEntry) -> bool {
    if !any_key(corpus_raw, corpus_folded, &entry.keys, entry.case_sensitive) {
        return false;
    }
    if entry.secondary_keys.is_empty() {
        return true;
    }
    let secondary_any = any_key(
        corpus_raw,
        corpus_folded,
        &entry.secondary_keys,
        entry.case_sensitive,
    );
    let secondary_all = entry
        .secondary_keys
        .iter()
        .all(|key| key_in_corpus(corpus_raw, corpus_folded, key, entry.case_sensitive));
    match entry.logic {
        LorebookLogic::AndAny => secondary_any,
        LorebookLogic::AndAll => secondary_all,
        LorebookLogic::NotAny => !secondary_any,
        LorebookLogic::NotAll => !secondary_all,
    }
}

fn any_key(corpus_raw: &str, corpus_folded: &str, keys: &[String], case_sensitive: bool) -> bool {
    keys.iter()
        .any(|key| key_in_corpus(corpus_raw, corpus_folded, key, case_sensitive))
}

fn key_in_corpus(corpus_raw: &str, corpus_folded: &str, key: &str, case_sensitive: bool) -> bool {
    let key = key.trim();
    if key.is_empty() {
        return false;
    }
    if case_sensitive {
        corpus_raw.contains(key)
    } else {
        corpus_folded.contains(&key.to_lowercase())
    }
}

/// Deterministic 0..100 roll for `(turn, entry id)`. Same turn + entry always
/// rolls the same, so engine retries never flip a probability decision.
fn probability_roll(turn: u32, entry_id: &str) -> u32 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    turn.hash(&mut hasher);
    entry_id.hash(&mut hasher);
    (hasher.finish() % 100) as u32
}

/// `true` when an activation inside the sticky window force-includes the
/// entry for `upcoming` (ST `sticky`: stays in context N turns after firing).
fn sticky_window_active(
    ledger: &[story_model::LorebookActivation],
    entry_id: &str,
    upcoming: u32,
    sticky: u32,
) -> bool {
    sticky > 0
        && ledger.iter().any(|record| {
            record.entry_id == entry_id && upcoming.saturating_sub(record.turn) <= sticky
        })
}

/// `true` when the entry is still inside its post-trigger cooldown window
/// (sticky window first, then `cooldown` turns of silence).
fn cooldown_window_active(
    ledger: &[story_model::LorebookActivation],
    entry_id: &str,
    upcoming: u32,
    window: u32,
) -> bool {
    window > 0
        && ledger.iter().any(|record| {
            record.entry_id == entry_id && upcoming.saturating_sub(record.turn) <= window
        })
}

/// Lexical cosine over alphanumeric tokens — the deterministic stand-in for
/// embedding similarity on `vectorized` entries. Conservative threshold:
/// firing on generic overlap would inject lore every turn.
fn lexical_similarity(corpus: &str, content: &str) -> f32 {
    let corpus_tokens = lexical_tokens(corpus);
    if corpus_tokens.is_empty() {
        return 0.0;
    }
    let mut corpus_counts: std::collections::BTreeMap<&str, u32> = Default::default();
    for token in &corpus_tokens {
        *corpus_counts.entry(token.as_str()).or_insert(0) += 1;
    }
    let mut content_counts: std::collections::BTreeMap<String, u32> = Default::default();
    let mut total = 0_u32;
    for token in lexical_tokens(content) {
        total += 1;
        *content_counts.entry(token).or_insert(0) += 1;
    }
    if total == 0 {
        return 0.0;
    }
    let dot: u64 = content_counts
        .iter()
        .map(|(token, count)| {
            let corpus_count = corpus_counts.get(token.as_str()).copied().unwrap_or(0) as u64;
            corpus_count * u64::from(*count)
        })
        .sum();
    let content_norm = f64::from(total).sqrt();
    let corpus_norm = f64::sqrt(corpus_counts.values().map(|c| u64::from(*c)).sum::<u64>() as f64);
    if corpus_norm == 0.0 {
        return 0.0;
    }
    (dot as f64 / (content_norm * corpus_norm)) as f32
}

/// Lowercased alphanumeric tokens of at least two chars (Hangul included via
/// `is_alphanumeric`).
fn lexical_tokens(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .map(|token| token.trim().to_lowercase())
        .filter(|token| token.chars().count() >= 2)
        .collect()
}

fn before_first(position: LorebookPosition) -> i64 {
    match position {
        LorebookPosition::BeforeChar => 0,
        LorebookPosition::AfterChar => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use story_model::{LocationId, Lorebook, LorebookPosition, TimeSlot};

    fn entry(id: &str, keys: &[&str], content: &str) -> LorebookEntry {
        LorebookEntry {
            id: id.to_string(),
            keys: keys.iter().map(|key| key.to_string()).collect(),
            secondary_keys: Vec::new(),
            logic: LorebookLogic::AndAny,
            content: content.to_string(),
            constant: false,
            enabled: true,
            insertion_order: 100,
            position: LorebookPosition::BeforeChar,
            case_sensitive: false,
            probability: 100,
            priority: None,
            comment: None,
            prevent_recursion: false,
            exclude_recursion: false,
            sticky: 0,
            cooldown: 0,
            delay: 0,
            vectorized: false,
        }
    }

    fn state_with(history: &[&str]) -> GameState {
        let mut state = crate::tests_support::sample_game_state();
        state.turn = 7;
        state.history = history
            .iter()
            .enumerate()
            .map(|(index, narration)| {
                crate::tests_support::sample_history_entry(index as u32 + 1, narration)
            })
            .collect();
        state
    }

    fn book(entries: Vec<LorebookEntry>) -> Lorebook {
        Lorebook {
            name: "test".to_string(),
            entries,
            ..Lorebook::default()
        }
    }

    fn pack_with(book: Lorebook) -> StoryPack {
        let mut pack = crate::tests_support::sample_pack();
        pack.lorebook = book;
        pack
    }

    #[test]
    fn keyed_entry_triggers_on_narration_text() {
        let mut pack = pack_with(book(vec![entry("expo", &["전시회"], "전시회의 기억.")]));
        pack.lorebook.entries[0].position = LorebookPosition::AfterChar;
        let state = state_with(&["그날 전시회 이야기가 나왔다."]);

        let selected = select_lore(&pack, &state, &state.history, None, None);
        assert_eq!(selected.triggered.len(), 1);
        assert_eq!(selected.triggered[0].id, "expo");
    }

    #[test]
    fn unmatched_entry_stays_out() {
        let pack = pack_with(book(vec![entry("expo", &["전시회"], "전시회의 기억.")]));
        let state = state_with(&["카페 창밖의 비만 보였다."]);
        assert!(
            select_lore(&pack, &state, &state.history, None, None)
                .triggered
                .is_empty()
        );
    }

    #[test]
    fn matching_is_case_folded_unless_case_sensitive() {
        let pack = pack_with(book(vec![entry("folded", &["Exhibition"], "x"), {
            let mut sensitive = entry("sensitive", &["Exhibition"], "y");
            sensitive.case_sensitive = true;
            sensitive
        }]));
        let state = state_with(&["the exhibition was crowded."]);
        let selected = select_lore(&pack, &state, &state.history, None, None);
        let ids: Vec<&str> = selected.triggered.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["folded"],
            "case-sensitive key must not match lowercase text"
        );
    }

    #[test]
    fn constant_entries_ignore_scans_and_split_by_position() {
        let mut pack = pack_with(book(vec![
            {
                let mut always = entry("always", &["없는키"], "상시 설정.");
                always.constant = true;
                always
            },
            {
                let mut after = entry("after", &["없는키"], "캐논 뒤 상시.");
                after.constant = true;
                after.position = LorebookPosition::AfterChar;
                after
            },
        ]));
        pack.lorebook.entries[0].position = LorebookPosition::BeforeChar;
        let state = state_with(&["아무 키도 없는 이야기."]);

        let selected = select_lore(&pack, &state, &state.history, None, None);
        assert_eq!(selected.stable_before.len(), 1);
        assert_eq!(selected.stable_after.len(), 1);
        assert!(selected.triggered.is_empty());
    }

    #[test]
    fn secondary_keys_gate_by_logic() {
        let mut rain = entry("rain", &["비"], "빗소리.");
        rain.secondary_keys = vec!["우산".to_string()];
        let pack = pack_with(book(vec![rain]));

        let with_both = state_with(&["비가 오고 우산을 챙겼다."]);
        assert_eq!(
            select_lore(&pack, &with_both, &with_both.history, None, None)
                .triggered
                .len(),
            1,
            "AND_ANY passes when a secondary key also matches"
        );

        let rain_only = state_with(&["비만 내렸다."]);
        assert!(
            select_lore(&pack, &rain_only, &rain_only.history, None, None)
                .triggered
                .is_empty(),
            "AND_ANY fails without any secondary key"
        );
    }

    #[test]
    fn budget_drops_lowest_priority_entries_whole() {
        let mut small = entry("small", &["키"], "짧은 내용.");
        small.priority = Some(0);
        let mut big = entry("big", &["키"], &"긴 내용 ".repeat(300));
        big.priority = Some(9);
        let mut lorebook = book(vec![big, small]);
        lorebook.token_budget = 100; // fits `small`, never `big`
        let pack = pack_with(lorebook);
        let state = state_with(&["키워드 등장."]);

        let selected = select_lore(&pack, &state, &state.history, None, None);
        let ids: Vec<&str> = selected.triggered.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["small"],
            "high priority (low value) wins the budget"
        );
    }

    #[test]
    fn probability_roll_is_deterministic_per_turn_and_entry() {
        assert_eq!(probability_roll(7, "expo"), probability_roll(7, "expo"));
        assert_ne!(
            probability_roll(7, "expo"),
            probability_roll(8, "expo"),
            "different turns re-roll (over the same 0..100 space)"
        );
    }

    #[test]
    fn disabled_entries_never_trigger() {
        let mut off = entry("off", &["전시회"], "x");
        off.enabled = false;
        let pack = pack_with(book(vec![off]));
        let state = state_with(&["전시회를 말했다."]);
        assert!(
            select_lore(&pack, &state, &state.history, None, None)
                .triggered
                .is_empty()
        );
    }

    #[test]
    fn empty_book_short_circuits() {
        let pack = pack_with(Lorebook::default());
        let state = state_with(&["전시회를 말했다."]);
        assert!(select_lore(&pack, &state, &state.history, None, None).is_empty());
    }

    // Keep unused-import lints honest for the types only used in signatures.
    #[test]
    fn type_witness() {
        let _map: BTreeMap<String, String> = BTreeMap::new();
        let _loc = LocationId("x".to_string());
        let _slot = TimeSlot::Evening;
    }

    // --- recursion (T2) ------------------------------------------------------

    fn recursive_book(entries: Vec<LorebookEntry>) -> Lorebook {
        Lorebook {
            recursive_scanning: true,
            ..book(entries)
        }
    }

    #[test]
    fn recursion_chains_entries_when_enabled() {
        let mut a = entry("a", &["비"], "그날 전시회 이야기가 떠올랐다.");
        a.prevent_recursion = false;
        let b = entry("b", &["전시회"], "전시회의 기억.");
        let pack = pack_with(recursive_book(vec![a, b]));
        let state = state_with(&["비가 그쳤다."]);

        let selected = select_lore(&pack, &state, &state.history, None, None);
        let mut ids: Vec<&str> = selected.triggered.iter().map(|e| e.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            vec!["a", "b"],
            "b fires from a's content, not the story text"
        );
    }

    #[test]
    fn recursion_requires_the_master_switch() {
        let a = entry("a", &["비"], "그날 전시회 이야기가 떠올랐다.");
        let b = entry("b", &["전시회"], "전시회의 기억.");
        let pack = pack_with(book(vec![a, b])); // recursive_scanning: false
        let state = state_with(&["비가 그쳤다."]);

        let selected = select_lore(&pack, &state, &state.history, None, None);
        let ids: Vec<&str> = selected.triggered.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["a"], "no master switch, no recursive activation");
    }

    #[test]
    fn exclude_recursion_blocks_recursive_activation_but_not_direct() {
        let a = entry("a", &["비"], "그날 전시회 이야기가 떠올랐다.");
        let mut b = entry("b", &["전시회"], "전시회의 기억.");
        b.exclude_recursion = true;
        let pack = pack_with(recursive_book(vec![a, b]));

        // Direct match still fires: the story text itself mentions 전시회.
        let direct = state_with(&["비가 그치고 전시회 포스터가 보였다."]);
        let selected = select_lore(&pack, &direct, &direct.history, None, None);
        let mut ids: Vec<&str> = selected.triggered.iter().map(|e| e.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            vec!["a", "b"],
            "exclude_recursion only gates recursive passes"
        );

        // Recursive activation is blocked: only a's content mentions 전시회.
        let indirect = state_with(&["비가 그쳤다."]);
        let selected = select_lore(&pack, &indirect, &indirect.history, None, None);
        let ids: Vec<&str> = selected.triggered.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["a"]);
    }

    #[test]
    fn prevent_recursion_stops_outgoing_triggers() {
        let mut a = entry("a", &["비"], "그날 전시회 이야기가 떠올랐다.");
        a.prevent_recursion = true;
        let b = entry("b", &["전시회"], "전시회의 기억.");
        let pack = pack_with(recursive_book(vec![a, b]));
        let state = state_with(&["비가 그쳤다."]);

        let selected = select_lore(&pack, &state, &state.history, None, None);
        let ids: Vec<&str> = selected.triggered.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["a"], "a fires but must not trigger b");
    }

    #[test]
    fn recursion_pass_budget_caps_chain_depth() {
        // a → b → c → d → e: each entry's content names the next key.
        let chain = [
            ("a", "비", "그날 전시회 이야기가 떠올랐다."),
            ("b", "전시회", "전시회 다음 날, 우산을 두고 나왔다."),
            ("c", "우산", "두고 간 우산을 찾으러 지하철역으로 갔다."),
            ("d", "지하철", "지하철역 앞에서 마지막 인사를 떠올렸다."),
            ("e", "마지막 인사", "마지막 인사의 온기."),
        ]
        .into_iter()
        .map(|(id, key, content)| entry(id, &[key], content))
        .collect();
        let pack = pack_with(recursive_book(chain));
        let state = state_with(&["비가 그쳤다."]);

        let selected = select_lore(&pack, &state, &state.history, None, None);
        let ids: Vec<&str> = selected.triggered.iter().map(|e| e.id.as_str()).collect();
        // direct + MAX_RECURSION_PASSES(3) activations: a..=d fire, e needs a 4th pass.
        assert_eq!(ids.len(), 4, "chain depth is capped: {ids:?}");
        assert!(!ids.contains(&"e"), "the fifth link stays out: {ids:?}");
    }

    // --- timers (sticky/cooldown/delay) + vectorized -------------------------

    fn ledger(pairs: &[(u32, &str)]) -> Vec<story_model::LorebookActivation> {
        pairs
            .iter()
            .map(|(turn, id)| story_model::LorebookActivation {
                turn: *turn,
                entry_id: id.to_string(),
            })
            .collect()
    }

    #[test]
    fn sticky_keeps_entry_active_inside_its_window() {
        let mut sticky = entry("sticky", &["전시회"], "전시회의 기억.");
        sticky.sticky = 2;
        let pack = pack_with(book(vec![sticky]));
        // Turn 8 upcoming (state turn 7); last fired turn 7 → inside the window.
        let mut state = state_with(&["아무 키도 없는 장면."]);
        state.lorebook_activations = ledger(&[(7, "sticky")]);

        let selected = select_lore(&pack, &state, &state.history, None, None);
        assert_eq!(
            selected.triggered.len(),
            1,
            "sticky fires with no key in the story text"
        );

        // Window expired: last fired turn 5, window 2, upcoming 8 → 8-5 = 3 > 2.
        state.lorebook_activations = ledger(&[(5, "sticky")]);
        assert!(
            select_lore(&pack, &state, &state.history, None, None)
                .triggered
                .is_empty(),
            "sticky outside the window no longer force-fires"
        );
    }

    #[test]
    fn cooldown_blocks_retriggering_after_the_window() {
        let mut gated = entry("gated", &["전시회"], "전시회의 기억.");
        gated.cooldown = 3;
        let pack = pack_with(book(vec![gated]));
        let history = ["전시회 이야기를 다시 꺼냈다."];

        // Inside cooldown (fired turn 7, upcoming 8, window 3): key matches but blocked.
        let mut state = state_with(&history);
        state.lorebook_activations = ledger(&[(7, "gated")]);
        assert!(
            select_lore(&pack, &state, &state.history, None, None)
                .triggered
                .is_empty(),
            "cooldown bars a key match inside the window"
        );

        // Cooldown over (fired turn 4, upcoming 8 → 8-4 = 4 > 3 → allowed).
        state.lorebook_activations = ledger(&[(4, "gated")]);
        assert_eq!(
            select_lore(&pack, &state, &state.history, None, None)
                .triggered
                .len(),
            1,
            "after the cooldown window the entry fires again"
        );
    }

    #[test]
    fn delay_defers_activation_to_a_recursion_pass() {
        let mut deferred = entry("deferred", &["전시회"], "전시회의 기억.");
        deferred.delay = 1;
        let mut opener = entry("opener", &["비"], "그날 전시회 이야기가 떠올랐다.");
        opener.prevent_recursion = false;
        let pack = pack_with(recursive_book(vec![opener, deferred]));

        // Direct story text HAS the key — still blocked in the direct pass.
        let state = state_with(&["전시회를 말했다."]);
        let selected = select_lore(&pack, &state, &state.history, None, None);
        let ids: Vec<&str> = selected.triggered.iter().map(|e| e.id.as_str()).collect();
        assert!(
            !ids.contains(&"deferred"),
            "delay > 0 never fires directly: {ids:?}"
        );

        // Recursion pass 1 satisfies delay = 1 via opener's content.
        let indirect = state_with(&["비가 그쳤다."]);
        let selected = select_lore(&pack, &indirect, &indirect.history, None, None);
        let ids: Vec<&str> = selected.triggered.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["opener", "deferred"],
            "fires on recursion pass 1: {ids:?}"
        );
    }

    #[test]
    fn vectorized_entries_fire_on_lexical_overlap() {
        let mut vector = entry(
            "vector",
            &[],
            "전시회 포스터가 붙은 카페 골목, 두 사람의 추억이 깃든 곳.",
        );
        vector.vectorized = true;
        let mut unrelated = entry("unrelated", &[], "우주 정거장의 무중력 실험실.");
        unrelated.vectorized = true;
        let pack = pack_with(book(vec![vector, unrelated]));
        let state = state_with(&["카페 골목 끝의 전시회 포스터를 바라보았다."]);

        let selected = select_lore(&pack, &state, &state.history, None, None);
        let ids: Vec<&str> = selected.triggered.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["vector"],
            "lexical overlap fires the related entry only: {ids:?}"
        );
    }

    #[test]
    fn reduce_records_activation_ledger_for_turn_starts() {
        use crate::engine_cmd::EngineCommand;

        let mut pack = crate::tests_support::sample_pack();
        // A constant entry: the opening turn has no corpus, so only constants fire.
        let mut always = entry("expo", &[], "항상 포함되는 설정.");
        always.constant = true;
        pack.lorebook = book(vec![always]);
        let state = crate::engine::EngineState::initial(&pack);

        let transition = crate::engine::reduce(&state, EngineCommand::StartNewGame, &pack)
            .expect("start new game");
        assert!(
            transition
                .state
                .game
                .lorebook_activations
                .iter()
                .any(|record| record.entry_id == "expo"),
            "opening request records its lore activations: {:?}",
            transition.state.game.lorebook_activations
        );
    }
}
