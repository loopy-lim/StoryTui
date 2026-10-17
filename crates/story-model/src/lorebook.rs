//! Lorebook types: keyword-triggered world-info entries injected into the
//! per-turn prompt context.
//!
//! The shape is a normalized subset of the SillyTavern World Info /
//! character-book vocabularies (see `crates/story-import` for the parsers):
//! both the V2 `character_book` (arrays, `enabled`, `before_char`) and the
//! standalone World Info export (string-or-array `key`, `disable`, numeric
//! positions) collapse into these types.
//!
//! Execution tiering (import policy): the engine currently executes the T0+T1
//! features (keys, constant, order, case, positions, selective logic,
//! scan_depth, probability). Recursion fields are parsed and preserved for the
//! upcoming T2 pass; sticky/cooldown/delay and vectorized search are not
//! represented here at all — the importer reports them as ignored.

use serde::{Deserialize, Serialize};

/// Entries are scanned against the most recent N turns of narration plus the
/// live player action.
pub const DEFAULT_LOREBOOK_SCAN_DEPTH: u32 = 2;
/// Soft per-turn token budget for injected entries (CJK-aware estimate; the
/// admission loop pre-trims so the provider's hard bound never has to).
pub const DEFAULT_LOREBOOK_TOKEN_BUDGET: u32 = 2_000;

/// How `secondary_keys` gate a keyed entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LorebookLogic {
    /// Any primary key AND any secondary key must match (ST `AND_ANY`).
    AndAny,
    /// Any primary key AND all secondary keys must match (ST `AND_ALL`).
    AndAll,
    /// Any primary key and NO secondary key may match (ST `NOT_ANY`).
    NotAny,
    /// Any primary key and NOT all secondary keys may match (ST `NOT_ALL`).
    NotAll,
}

/// Where an activated entry is placed relative to the character canon block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LorebookPosition {
    /// Before the character canon (ST `before_char`, world-info position 0).
    BeforeChar,
    /// After the character canon (ST `after_char`, world-info position 1).
    AfterChar,
}

/// One keyword-triggered lorebook entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LorebookEntry {
    /// Stable in-pack identifier (used for deterministic probability seeding
    /// and diagnostics).
    pub id: String,
    /// Primary trigger keys; an entry activates when any key matches.
    #[serde(default)]
    pub keys: Vec<String>,
    /// Secondary keys gating `logic`.
    #[serde(default)]
    pub secondary_keys: Vec<String>,
    /// Secondary-key gate (default [`LorebookLogic::AndAny`]).
    #[serde(default = "default_logic")]
    pub logic: LorebookLogic,
    /// Prompt text injected when the entry activates.
    pub content: String,
    /// Always included (within budget), ignoring keyword scans.
    #[serde(default)]
    pub constant: bool,
    /// Disabled entries are never selected.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Lower inserts earlier within an injection block.
    #[serde(default = "default_order")]
    pub insertion_order: i64,
    /// Injection block placement (default [`LorebookPosition::BeforeChar`]).
    #[serde(default = "default_position")]
    pub position: LorebookPosition,
    /// Match keys case-sensitively (default: case-insensitive).
    #[serde(default)]
    pub case_sensitive: bool,
    /// Percent chance to activate per turn, 0–100 (default: always).
    #[serde(default = "default_probability")]
    pub probability: u32,
    /// Budget-pressure discard hint: lower is discarded first. `None` means
    /// "same tier as everything else; `insertion_order` decides".
    #[serde(default)]
    pub priority: Option<i64>,
    /// Author note; never injected.
    #[serde(default)]
    pub comment: Option<String>,
    /// This entry's content must not trigger other entries (T2 recursion,
    /// ST `preventRecursion`).
    #[serde(default)]
    pub prevent_recursion: bool,
    /// Other entries' content must not trigger this entry (T2 recursion,
    /// ST `excludeRecursion`).
    #[serde(default)]
    pub exclude_recursion: bool,
    /// Turns the entry stays force-included after its last trigger
    /// (ST `sticky`). `0` disables.
    #[serde(default)]
    pub sticky: u32,
    /// Turns after the sticky window before the entry may trigger again
    /// (ST `cooldown`). `0` disables.
    #[serde(default)]
    pub cooldown: u32,
    /// 1-based recursion pass the entry may first fire in. `0` also fires in
    /// the direct pass (ST `delayUntilRecursion`, pass-based approximation).
    #[serde(default)]
    pub delay: u32,
    /// ST `vectorized`: keys first, then a lexical-similarity fallback against
    /// the scan corpus (embedding search approximated lexically).
    #[serde(default)]
    pub vectorized: bool,
}

/// One lorebook activation recorded per turn for sticky/cooldown timers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LorebookActivation {
    /// Turn the entry was selected for (the turn whose prompt includes it).
    pub turn: u32,
    /// Activated entry id.
    pub entry_id: String,
}

/// Activation ledger cap: newest entries kept, oldest dropped.
pub const LOREBOOK_LEDGER_CAP: usize = 128;

fn default_logic() -> LorebookLogic {
    LorebookLogic::AndAny
}
fn default_true() -> bool {
    true
}
fn default_order() -> i64 {
    100
}
fn default_position() -> LorebookPosition {
    LorebookPosition::BeforeChar
}
fn default_probability() -> u32 {
    100
}

/// A pack's lorebook, loaded from the optional `lorebook.toml`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lorebook {
    /// Display name (imported book name; may be empty).
    #[serde(default)]
    pub name: String,
    /// How many recent turns of narration the scan covers.
    #[serde(default = "default_scan_depth")]
    pub scan_depth: u32,
    /// Per-turn soft token budget for all injected entries combined.
    #[serde(default = "default_token_budget")]
    pub token_budget: u32,
    /// Whether entry content may trigger other entries (T2 recursion). Parsed
    /// and preserved now; honored by the engine's upcoming recursion pass.
    #[serde(default)]
    pub recursive_scanning: bool,
    /// Triggered entries.
    #[serde(default)]
    pub entries: Vec<LorebookEntry>,
}

fn default_scan_depth() -> u32 {
    DEFAULT_LOREBOOK_SCAN_DEPTH
}
fn default_token_budget() -> u32 {
    DEFAULT_LOREBOOK_TOKEN_BUDGET
}

impl Default for Lorebook {
    fn default() -> Self {
        Self {
            name: String::new(),
            scan_depth: DEFAULT_LOREBOOK_SCAN_DEPTH,
            token_budget: DEFAULT_LOREBOOK_TOKEN_BUDGET,
            recursive_scanning: false,
            entries: Vec::new(),
        }
    }
}

impl Lorebook {
    /// `true` when the book cannot contribute anything to a prompt.
    pub fn is_empty(&self) -> bool {
        self.entries.iter().all(|entry| !entry.enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> LorebookEntry {
        LorebookEntry {
            id: id.to_string(),
            keys: vec!["비".to_string()],
            secondary_keys: Vec::new(),
            logic: LorebookLogic::AndAny,
            content: "빗소리가 커진다.".to_string(),
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

    #[test]
    fn entry_defaults_fill_from_minimal_toml() {
        let toml_text = r#"
id = "rain"
content = "빗소리가 커진다."
"#;
        let parsed: LorebookEntry = toml::from_str(toml_text).expect("parse");
        assert_eq!(parsed.logic, LorebookLogic::AndAny);
        assert!(parsed.enabled);
        assert_eq!(parsed.insertion_order, 100);
        assert_eq!(parsed.position, LorebookPosition::BeforeChar);
        assert_eq!(parsed.probability, 100);
    }

    #[test]
    fn lorebook_book_defaults_and_empty_check() {
        let book: Lorebook = toml::from_str("name = \"book\"\n").expect("parse");
        assert_eq!(book.scan_depth, DEFAULT_LOREBOOK_SCAN_DEPTH);
        assert_eq!(book.token_budget, DEFAULT_LOREBOOK_TOKEN_BUDGET);
        assert!(book.is_empty(), "no entries means empty");

        let mut with_disabled = book.clone();
        let mut disabled = entry("off");
        disabled.enabled = false;
        with_disabled.entries.push(disabled);
        assert!(with_disabled.is_empty(), "only-disabled books are empty");
    }
}
