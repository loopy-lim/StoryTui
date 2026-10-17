//! World Info normalization: both the V2/V3 embedded `character_book`
//! vocabulary and the standalone SillyTavern World Info export collapse into
//! one normalized book of [`story_model::LorebookEntry`] values.
//!
//! Execution tiering: T0+T1 features map directly. ST-only features (numeric
//! depth/AN/EM positions, sticky/cooldown/delay timers, vectorized search)
//! become report notes — their data stays in `sources/`, the engine just does
//! not execute them yet.

use serde::Deserialize;
use story_model::{LorebookEntry, LorebookLogic, LorebookPosition};

use crate::card::{CardBook, CardBookEntry};

/// A normalized lorebook: engine-ready entries plus book-level metadata.
pub struct NormalizedBook {
    pub name: String,
    pub scan_depth: Option<u32>,
    pub token_budget: Option<u32>,
    pub recursive_scanning: bool,
    pub entries: Vec<LorebookEntry>,
}

/// Parse a standalone World Info export (`{"entries": {...}}`, object or
/// array form) into a normalized book. Report notes are appended to `notes`.
pub fn parse_world_info(
    text: &str,
    notes: &mut Vec<(String, String)>,
) -> Result<NormalizedBook, String> {
    let root: serde_json::Value = serde_json::from_str(text.trim())
        .map_err(|e| format!("World Info JSON을 해석하지 못했습니다: {e}"))?;
    let entries_value = root
        .get("entries")
        .ok_or("World Info에 entries가 없습니다")?
        .clone();
    let items: Vec<serde_json::Value> = match entries_value {
        serde_json::Value::Object(map) => map.into_values().collect(),
        serde_json::Value::Array(items) => items,
        _ => return Err("World Info entries의 형식이 올바르지 않습니다".to_string()),
    };
    let mut entries = Vec::new();
    for (index, item) in items.into_iter().enumerate() {
        let entry: WiEntry = serde_json::from_value(item)
            .map_err(|e| format!("World Info 엔트리 #{index}를 해석하지 못했습니다: {e}"))?;
        if let Some(entry) = standalone_entry(index, entry, notes) {
            entries.push(entry);
        }
    }
    Ok(NormalizedBook {
        name: String::new(),
        scan_depth: None,
        token_budget: None,
        recursive_scanning: false,
        entries,
    })
}

/// Normalize an embedded V2 `character_book`.
pub fn normalize_embedded_book(
    book: CardBook,
    notes: &mut Vec<(String, String)>,
) -> NormalizedBook {
    let mut entries = Vec::new();
    for (index, entry) in book.entries.into_iter().enumerate() {
        if let Some(entry) = embedded_entry(index, entry, notes) {
            entries.push(entry);
        }
    }
    NormalizedBook {
        name: book.name,
        scan_depth: book.scan_depth,
        token_budget: book.token_budget,
        recursive_scanning: book.recursive_scanning,
        entries,
    }
}

/// SillyTavern `world_info_logic` codes: 0 AND_ANY, 1 NOT_ALL, 2 NOT_ANY,
/// 3 AND_ALL.
fn logic_from_st_code(code: u8, notes: &mut Vec<(String, String)>) -> LorebookLogic {
    match code {
        0 => LorebookLogic::AndAny,
        1 => LorebookLogic::NotAll,
        2 => LorebookLogic::NotAny,
        3 => LorebookLogic::AndAll,
        other => {
            notes.push((
                format!("selectiveLogic={other}"),
                "알 수 없는 조건 코드 — AND_ANY로 근사".to_string(),
            ));
            LorebookLogic::AndAny
        }
    }
}

/// ST world-info `position` codes: 0 before char, 1 after char, 2+ (author's
/// note / example-message slots) have no equivalent in the two-message prompt
/// and are approximated as after_char.
fn position_from_st_code(code: i64, notes: &mut Vec<(String, String)>) -> LorebookPosition {
    match code {
        0 => LorebookPosition::BeforeChar,
        1 => LorebookPosition::AfterChar,
        other => {
            notes.push((
                format!("position={other}"),
                "AN/EM/depth 위치는 after_char로 근사".to_string(),
            ));
            LorebookPosition::AfterChar
        }
    }
}

fn clean_keys(keys: Vec<String>) -> Vec<String> {
    keys.into_iter()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
        .collect()
}

fn clamp_probability(value: u32) -> u32 {
    value.min(100)
}

/// Normalize one embedded-book entry. Empty-content entries are skipped.
fn embedded_entry(
    index: usize,
    entry: CardBookEntry,
    notes: &mut Vec<(String, String)>,
) -> Option<LorebookEntry> {
    let content = entry.content.trim().to_string();
    if content.is_empty() {
        notes.push((
            format!("entry[{index}]"),
            "내용이 비어 있어 건너뜁니다".to_string(),
        ));
        return None;
    }
    let keys = clean_keys(entry.keys);
    let secondary_keys = if entry.selective == Some(false) {
        Vec::new()
    } else {
        clean_keys(entry.secondary_keys)
    };
    let subject = entry
        .comment
        .clone()
        .unwrap_or_else(|| format!("entry[{index}]"));

    // SillyTavern exports park the numeric vocabulary inside `extensions`.
    let extensions = entry.extensions;
    let mut logic = LorebookLogic::AndAny;
    let mut probability = 100_u32;
    let mut position = None;
    let mut exclude_recursion = false;
    let mut prevent_recursion = false;
    let mut sticky = 0_u32;
    let mut cooldown = 0_u32;
    let mut delay = 0_u32;
    let mut vectorized = false;
    if let Some(map) = extensions.as_object() {
        if let Some(code) = map
            .get("selectiveLogic")
            .and_then(serde_json::Value::as_u64)
        {
            logic = logic_from_st_code(code.min(u8::MAX as u64) as u8, notes);
        }
        match (
            map.get("useProbability")
                .and_then(serde_json::Value::as_bool),
            map.get("probability").and_then(serde_json::Value::as_u64),
        ) {
            (Some(false), _) => probability = 100,
            (_, Some(value)) => probability = clamp_probability(value.min(u32::MAX as u64) as u32),
            _ => {}
        }
        if let Some(code) = map.get("position").and_then(serde_json::Value::as_i64) {
            position = Some(position_from_st_code(code, notes));
        }
        if map
            .get("depth")
            .and_then(serde_json::Value::as_u64)
            .is_some()
        {
            notes.push((
                subject.clone(),
                "depth 삽입은 미지원 — 기본 위치로 주입".to_string(),
            ));
        }
        vectorized = map
            .get("vectorized")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if vectorized {
            notes.push((
                subject.clone(),
                "vectorized는 임베딩 대신 어휘 유사도로 근사 실행".to_string(),
            ));
        }
        // Timers: ST counts messages; saturate into u32.
        sticky = map
            .get("sticky")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
            .clamp(0, u32::MAX as i64) as u32;
        cooldown = map
            .get("cooldown")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
            .clamp(0, u32::MAX as i64) as u32;
        delay = map
            .get("delayUntilRecursion")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
            .clamp(0, u32::MAX as i64) as u32;
        exclude_recursion = map
            .get("excludeRecursion")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        prevent_recursion = map
            .get("preventRecursion")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
    }

    let position = position.unwrap_or({
        match entry.position.as_ref() {
            Some(serde_json::Value::String(text)) => match text.as_str() {
                "before_char" => LorebookPosition::BeforeChar,
                "after_char" => LorebookPosition::AfterChar,
                other => {
                    notes.push((
                        subject.clone(),
                        format!("position '{other}'는 알 수 없어 before_char로 근사"),
                    ));
                    LorebookPosition::BeforeChar
                }
            },
            Some(serde_json::Value::Number(code)) => {
                position_from_st_code(code.as_i64().unwrap_or(0), notes)
            }
            _ => LorebookPosition::BeforeChar,
        }
    });

    Some(LorebookEntry {
        id: format!("book-{index}"),
        keys,
        secondary_keys,
        logic,
        content,
        constant: entry.constant.unwrap_or(false),
        enabled: entry.enabled.unwrap_or(true),
        insertion_order: entry.insertion_order.unwrap_or(100),
        position,
        case_sensitive: entry.case_sensitive.unwrap_or(false),
        probability,
        priority: entry.priority,
        comment: entry.comment,
        prevent_recursion,
        exclude_recursion,
        sticky,
        cooldown,
        delay,
        vectorized,
    })
}

/// Normalize one standalone World Info entry. Empty-content entries skipped.
fn standalone_entry(
    index: usize,
    entry: WiEntry,
    notes: &mut Vec<(String, String)>,
) -> Option<LorebookEntry> {
    let content = entry.content.trim().to_string();
    if content.is_empty() {
        notes.push((
            format!("wi[{index}]"),
            "내용이 비어 있어 건너뜁니다".to_string(),
        ));
        return None;
    }
    let subject = entry
        .comment
        .clone()
        .unwrap_or_else(|| format!("wi[{index}]"));
    let keys = entry.key;
    let secondary_keys = if entry.selective {
        clean_keys(entry.key_secondary)
    } else {
        Vec::new()
    };
    let logic = match entry.selective_logic {
        Some(code) => logic_from_st_code(code, notes),
        None => LorebookLogic::AndAny,
    };
    let probability = match (entry.use_probability, entry.probability) {
        (Some(false), _) => 100,
        (_, Some(value)) => clamp_probability(value),
        _ => 100,
    };
    let position = position_from_st_code(entry.position.unwrap_or(0), notes);
    // Timers execute now: a turn-based approximation of ST's message counters.
    let sticky = entry.sticky.unwrap_or(0).clamp(0, u32::MAX as i64) as u32;
    let cooldown = entry.cooldown.unwrap_or(0).clamp(0, u32::MAX as i64) as u32;
    let delay = entry
        .delay_until_recursion
        .or(entry.delay)
        .unwrap_or(0)
        .clamp(0, u32::MAX as i64) as u32;
    let vectorized = entry.vectorized;
    if vectorized {
        notes.push((
            subject.clone(),
            "vectorized는 임베딩 대신 어휘 유사도로 근사 실행".to_string(),
        ));
    }

    Some(LorebookEntry {
        id: format!("wi-{index}"),
        keys,
        secondary_keys,
        logic,
        content,
        constant: entry.constant,
        enabled: !entry.disable,
        insertion_order: entry.order.unwrap_or(100),
        position,
        case_sensitive: entry.case_sensitive.unwrap_or(false),
        probability,
        priority: None,
        comment: entry.comment,
        prevent_recursion: entry.prevent_recursion,
        exclude_recursion: entry.exclude_recursion,
        sticky,
        cooldown,
        delay,
        vectorized,
    })
}

/// String-or-array key lists: old TavernAI exports use comma-joined strings,
/// current ST uses arrays.
fn string_or_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value {
        serde_json::Value::String(text) => text
            .split(',')
            .map(|part| part.trim().to_string())
            .filter(|part| !part.is_empty())
            .collect(),
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    })
}

/// Standalone World Info entry DTO (camelCase ST export vocabulary).
#[derive(Default, Deserialize)]
#[serde(default)]
struct WiEntry {
    #[serde(deserialize_with = "string_or_vec")]
    key: Vec<String>,
    #[serde(rename = "keysecondary", deserialize_with = "string_or_vec")]
    key_secondary: Vec<String>,
    pub content: String,
    pub comment: Option<String>,
    pub constant: bool,
    pub selective: bool,
    #[serde(rename = "selectiveLogic")]
    pub selective_logic: Option<u8>,
    pub order: Option<i64>,
    pub position: Option<i64>,
    pub disable: bool,
    #[serde(rename = "caseSensitive")]
    pub case_sensitive: Option<bool>,
    pub probability: Option<u32>,
    #[serde(rename = "useProbability")]
    pub use_probability: Option<bool>,
    pub depth: Option<u32>,
    #[serde(rename = "excludeRecursion")]
    pub exclude_recursion: bool,
    #[serde(rename = "preventRecursion")]
    pub prevent_recursion: bool,
    #[serde(rename = "delayUntilRecursion")]
    pub delay_until_recursion: Option<i64>,
    pub sticky: Option<i64>,
    pub cooldown: Option<i64>,
    pub delay: Option<i64>,
    pub vectorized: bool,
}
