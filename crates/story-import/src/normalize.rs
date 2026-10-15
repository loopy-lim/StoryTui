//! Director normalization pass (`--normalize`): compile the card's free-form
//! description into the structured `CharacterDef` fields with one local-model
//! generation.
//!
//! Contract: the deterministic scaffold is always the floor. Normalization is
//! opt-in, runs after a verified pack exists, only *fills* structured fields
//! (it never touches `description_md` or `dialogue_examples`), and any failure
//! leaves the pack playable with a report note.

use std::path::Path;

use story_persistence::pack::dto::CharactersToml;
use story_provider::OpenAiCompatibleProvider;

use crate::report::ImportReport;

/// JSON Schema for the structured extraction. Kept narrow: short arrays with
/// bounded items, so a chatty local model cannot bloat the pack.
pub const NORMALIZE_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["personality", "likes", "dislikes", "speech_style", "honorific_rule", "current_goal", "long_term_desire", "known_facts", "unknown_facts"],
  "additionalProperties": false,
  "properties": {
    "personality": { "type": "array", "items": { "type": "string" }, "minItems": 2, "maxItems": 6 },
    "likes": { "type": "array", "items": { "type": "string" }, "maxItems": 6 },
    "dislikes": { "type": "array", "items": { "type": "string" }, "maxItems": 6 },
    "speech_style": { "type": "string" },
    "honorific_rule": { "type": "string" },
    "current_goal": { "type": "string" },
    "long_term_desire": { "type": "string" },
    "known_facts": { "type": "array", "items": { "type": "string" }, "maxItems": 8 },
    "unknown_facts": { "type": "array", "items": { "type": "string" }, "maxItems": 8 }
  }
}"#;

/// What the pass did, for the CLI summary.
#[derive(Debug, Default)]
pub struct NormalizeSummary {
    /// `true` when characters.toml was rewritten.
    pub applied: bool,
    /// Filled field names with counts (e.g. `personality 4개`).
    pub filled: Vec<String>,
}

/// Normalize the first character of the pack at `pack_dir` via `provider`.
/// On any failure the deterministic pack stays untouched and the reason lands
/// in the report as a warning.
pub async fn normalize_pack(
    pack_dir: &Path,
    provider: &OpenAiCompatibleProvider,
    report: &mut ImportReport,
) -> NormalizeSummary {
    match try_normalize(pack_dir, provider, report).await {
        Ok(summary) => summary,
        Err(reason) => {
            report.warn(format!(
                "Director 정규화 실패 — 결정적 임포트 결과를 유지합니다: {reason}"
            ));
            NormalizeSummary::default()
        }
    }
}

async fn try_normalize(
    pack_dir: &Path,
    provider: &OpenAiCompatibleProvider,
    report: &mut ImportReport,
) -> Result<NormalizeSummary, String> {
    let characters_path = pack_dir.join("characters.toml");
    let text = std::fs::read_to_string(&characters_path)
        .map_err(|e| format!("characters.toml을 읽지 못했습니다: {e}"))?;
    let mut doc: CharactersToml =
        toml::from_str(&text).map_err(|e| format!("characters.toml을 해석하지 못했습니다: {e}"))?;
    if doc.characters.is_empty() {
        return Err("characters.toml에 캐릭터가 없습니다".to_string());
    }

    let (system, user) = normalize_prompts(&doc.characters[0]);
    let schema: serde_json::Value =
        serde_json::from_str(NORMALIZE_SCHEMA).expect("NORMALIZE_SCHEMA is a static, valid schema");
    let raw = provider
        .generate_completion(&system, &user, 1_200, Some(schema))
        .await
        .map_err(|e| format!("생성 호출 실패: {e}"))?;
    let value = salvage_json(&raw).ok_or("응답에서 JSON 객체를 찾지 못했습니다")?;
    let normalized: NormalizedCharacter = serde_json::from_value(value)
        .map_err(|e| format!("응답 필드가 스키마와 맞지 않습니다: {e}"))?;

    let summary = apply(&mut doc.characters[0], &normalized);
    let out = toml::to_string_pretty(&doc)
        .map_err(|e| format!("characters.toml을 직렬화하지 못했습니다: {e}"))?;
    std::fs::write(&characters_path, out)
        .map_err(|e| format!("characters.toml을 쓰지 못했습니다: {e}"))?;

    report.mapped.push(format!(
        "Director 정규화: {}",
        if summary.filled.is_empty() {
            "채울 필드가 없었습니다".to_string()
        } else {
            summary.filled.join(", ")
        }
    ));
    Ok(summary)
}

/// The extraction instructions plus the card material to work from.
fn normalize_prompts(character: &story_persistence::pack::dto::CharacterToml) -> (String, String) {
    let system = "당신은 캐릭터 카드 구조화기다. 제공된 캐릭터 설명을 읽어 지정된 JSON 스키마의 \
필드를 한국어로 채운다.\n\n규칙:\n- 설명에 근거가 있는 내용만 쓴다. 근거가 없는 필드는 비워 둔다(빈 문자열·빈 배열).\n\
- personality는 2~6개의 짧은 단문 특성으로 나눈다.\n- speech_style은 말투 규칙(문장 길이, 어미, 특수 습관)을 한두 문장으로 요약한다.\n\
- honorific_rule은 존댓말/반말·호칭 전환 조건을 한 문장으로 쓴다. 설명에 없으면 빈 문자열.\n\
- known_facts는 이 캐릭터가 알고 있는 사실, unknown_facts는 모르는 사실이다. 각각 독립된 짧은 단문으로 쓴다 — \
엔진이 문자열 정확 매칭으로 사용하므로 한 문장에 두 사실을 섞지 않는다.\n\
- 지정된 JSON 객체 외의 텍스트(설명, 코드펜스)를 출력하지 않는다.";
    let mut user = format!(
        "캐릭터 이름: {}\n\n설명:\n{}",
        character.name,
        character.description_md.as_deref().unwrap_or("(설명 없음)")
    );
    if !character.dialogue_examples.is_empty() {
        user.push_str("\n\n대화 예시:\n");
        for example in &character.dialogue_examples {
            user.push_str(example);
            user.push('\n');
        }
    }
    (system.to_string(), user)
}

/// The model's structured extraction.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct NormalizedCharacter {
    personality: Vec<String>,
    likes: Vec<String>,
    dislikes: Vec<String>,
    speech_style: String,
    honorific_rule: String,
    current_goal: String,
    long_term_desire: String,
    known_facts: Vec<String>,
    unknown_facts: Vec<String>,
}

/// Fill the structured fields. Empty responses never overwrite the scaffold.
fn apply(
    character: &mut story_persistence::pack::dto::CharacterToml,
    normalized: &NormalizedCharacter,
) -> NormalizeSummary {
    let mut summary = NormalizeSummary::default();
    let mut record = |field: &str, count: usize| {
        if count > 0 {
            summary.filled.push(format!("{field} {count}개"));
        } else {
            summary.filled.push(field.to_string());
        }
    };

    if !normalized.personality.is_empty() {
        character.personality = normalized.personality.clone();
    }
    if !normalized.likes.is_empty() {
        character.likes = normalized.likes.clone();
    }
    if !normalized.dislikes.is_empty() {
        character.dislikes = normalized.dislikes.clone();
    }
    if !normalized.speech_style.trim().is_empty() {
        character.speech_style = normalized.speech_style.trim().to_string();
    }
    if !normalized.honorific_rule.trim().is_empty() {
        character.honorific_rule = normalized.honorific_rule.trim().to_string();
    }
    if !normalized.current_goal.trim().is_empty() {
        character.current_goal = normalized.current_goal.trim().to_string();
    }
    if !normalized.long_term_desire.trim().is_empty() {
        character.long_term_desire = normalized.long_term_desire.trim().to_string();
    }
    if !normalized.known_facts.is_empty() {
        character.known_facts = normalized.known_facts.clone();
    }
    if !normalized.unknown_facts.is_empty() {
        character.unknown_facts = normalized.unknown_facts.clone();
    }

    record("personality", character.personality.len());
    record("known_facts", character.known_facts.len());
    summary.applied = true;
    summary
}

/// Parse the model output as JSON, tolerating prose or fences around the
/// object (same spirit as `story-provider::recovery::locate_json`).
fn salvage_json(raw: &str) -> Option<serde_json::Value> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw.trim()) {
        return Some(value);
    }
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str(&raw[start..=end]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salvage_tolerates_prose_and_fences() {
        let fenced = "```json\n{\"a\": 1}\n```";
        assert_eq!(salvage_json(fenced).unwrap()["a"], 1);
        let prose = "결과는 다음과 같습니다: {\"b\": [2]} 입니다.";
        assert_eq!(salvage_json(prose).unwrap()["b"][0], 2);
        assert!(salvage_json("JSON이 아닙니다").is_none());
    }

    #[test]
    fn apply_fills_structured_fields_but_never_description() {
        let mut character = story_persistence::pack::dto::CharacterToml {
            id: "yuna".to_string(),
            name: "유나".to_string(),
            description_md: Some("원문 설명".to_string()),
            age: None,
            pronouns: None,
            dialogue_examples: Vec::new(),
            personality: Vec::new(),
            appearance: Vec::new(),
            values: Vec::new(),
            likes: Vec::new(),
            dislikes: Vec::new(),
            secrets: Vec::new(),
            initial_player_relationship: "acquaintance".to_string(),
            speech_style: String::new(),
            honorific_rule: String::new(),
            frequent_phrases: Vec::new(),
            avoid_phrases: Vec::new(),
            known_facts: Vec::new(),
            unknown_facts: Vec::new(),
            current_goal: String::new(),
            long_term_desire: String::new(),
        };
        let normalized = NormalizedCharacter {
            personality: vec!["차분함".to_string(), "관찰력".to_string()],
            known_facts: vec!["플레이어가 서울에 산다".to_string()],
            speech_style: "짧은 문장.".to_string(),
            ..Default::default()
        };
        let summary = apply(&mut character, &normalized);

        assert!(summary.applied);
        assert_eq!(character.personality.len(), 2);
        assert_eq!(character.known_facts.len(), 1);
        assert_eq!(character.speech_style, "짧은 문장.");
        assert_eq!(character.description_md.as_deref(), Some("원문 설명"));
        assert!(
            summary
                .filled
                .iter()
                .any(|line| line.contains("personality 2개")),
            "{:?}",
            summary.filled
        );
    }
}
