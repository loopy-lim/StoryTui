//! Deterministic character enrichment from card text — no LLM required.
//!
//! Feeds the engine lead ladder (`secrets`, goals, phrase fences) when
//! scaffolding a pack from a SillyTavern / Risu / CAI card.

/// Structured fields derived from free-form card prose.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CharacterEnrichment {
    pub secrets: Vec<String>,
    pub current_goal: String,
    pub long_term_desire: String,
    pub known_facts: Vec<String>,
    pub personality_hints: Vec<String>,
    pub frequent_phrases: Vec<String>,
    pub avoid_phrases: Vec<String>,
}

/// Engine loop-trap phrases that should always land in `avoid_phrases`.
pub const LOOP_PHRASES: &[&str] = &[
    "대답을 정해 두고 묻는",
    "왜 지금 만나자고",
    "As you know",
];

const SECRET_CUES: &[&str] = &[
    "비밀",
    "숨기",
    "아무도",
    "사실은",
    "몰래",
    "secret",
    "hidden",
    "never told",
    "won't tell",
    "wont tell",
];

const MIN_SECRET_LEN: usize = 12;
const MAX_SECRETS: usize = 4;
const MAX_FACTS: usize = 4;
const MAX_PHRASES: usize = 4;
const MAX_PERSONALITY: usize = 6;
const GOAL_SCENARIO_CHARS: usize = 80;

/// Derive lead-ladder fields from card text. `lore_entry_contents` may include
/// key-prefixed lines (`키 — 본문`) so secret-looking lore keys are visible.
pub fn enrich_from_card(
    _name: &str,
    description: &str,
    personality: &str,
    scenario: &str,
    mes_example: &str,
    lore_entry_contents: &[String],
) -> CharacterEnrichment {
    let secrets = extract_secrets(description, personality, scenario, lore_entry_contents);
    let current_goal = derive_current_goal(scenario);
    let long_term_desire = derive_long_term_desire(personality);
    let known_facts = extract_known_facts(description);
    let personality_hints = split_personality_hints(personality);
    let frequent_phrases = extract_frequent_phrases(mes_example);
    let avoid_phrases = build_avoid_phrases(&frequent_phrases);

    CharacterEnrichment {
        secrets,
        current_goal,
        long_term_desire,
        known_facts,
        personality_hints,
        frequent_phrases,
        avoid_phrases,
    }
}

fn extract_secrets(
    description: &str,
    personality: &str,
    scenario: &str,
    lore: &[String],
) -> Vec<String> {
    let mut out = Vec::new();
    for source in [description, personality, scenario] {
        for line in split_candidate_lines(source) {
            push_secret(&mut out, &line);
        }
    }
    for entry in lore {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Whole lore blob, or each line inside it.
        if looks_secret(trimmed) {
            push_secret(&mut out, trimmed);
        } else {
            for line in split_candidate_lines(trimmed) {
                push_secret(&mut out, &line);
            }
        }
    }
    out
}

fn push_secret(out: &mut Vec<String>, raw: &str) {
    if out.len() >= MAX_SECRETS {
        return;
    }
    let cleaned = strip_bullet(raw);
    if cleaned.chars().count() < MIN_SECRET_LEN || !looks_secret(&cleaned) {
        return;
    }
    // Prefer a single sentence / short clause.
    let short = truncate_chars(&cleaned, 120);
    if out.iter().any(|seen| seen.eq_ignore_ascii_case(&short)) {
        return;
    }
    out.push(short);
}

fn looks_secret(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    SECRET_CUES.iter().any(|cue| {
        if cue.is_ascii() {
            lower.contains(&cue.to_ascii_lowercase())
        } else {
            text.contains(cue)
        }
    })
}

fn derive_current_goal(scenario: &str) -> String {
    let scenario = scenario.trim();
    if scenario.is_empty() {
        return "플레이어와 숨긴 정보·관계 거리·장면 조건 중 하나를 실제로 바꾼다"
            .to_string();
    }
    let snippet = truncate_chars(scenario, GOAL_SCENARIO_CHARS);
    format!("시나리오 압력을 밀고 관계·정보를 한 단계 진전시킨다: {snippet}")
}

fn derive_long_term_desire(personality: &str) -> String {
    let personality = personality.trim();
    if personality.is_empty() {
        return "상대가 추측 대신 직접 듣게 되는 관계".to_string();
    }
    // Last sentence of personality prose.
    let last = personality
        .split(|c| c == '.' || c == '!' || c == '?' || c == '。' || c == '！' || c == '？')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or(personality);
    let last = truncate_chars(last, 80);
    if last.chars().count() < 8 {
        "상대가 추측 대신 직접 듣게 되는 관계".to_string()
    } else {
        last
    }
}

fn extract_known_facts(description: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in split_candidate_lines(description) {
        if out.len() >= MAX_FACTS {
            break;
        }
        let cleaned = strip_bullet(&line);
        if cleaned.chars().count() < 8 {
            continue;
        }
        if cleaned.contains('?') || cleaned.contains('？') {
            continue;
        }
        // Skip secret-looking lines — those belong in secrets.
        if looks_secret(&cleaned) {
            continue;
        }
        // Prefer factual tone: avoid first-person commands / pure adjectives.
        if cleaned.ends_with('!') || cleaned.ends_with('！') {
            continue;
        }
        if out.iter().any(|seen| seen == &cleaned) {
            continue;
        }
        out.push(truncate_chars(&cleaned, 100));
    }
    out
}

fn split_personality_hints(personality: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in personality.split('\n') {
        for part in raw.split("- ") {
            let cleaned = strip_bullet(part);
            if cleaned.chars().count() < 4 {
                continue;
            }
            // Also split dense prose on sentence boundaries when no bullets.
            if !raw.contains('-') && cleaned.chars().count() > 40 {
                for sentence in cleaned
                    .split(|c| c == '.' || c == '。' || c == '!' || c == '！')
                    .map(str::trim)
                    .filter(|s| s.chars().count() >= 4)
                {
                    if out.len() >= MAX_PERSONALITY {
                        return out;
                    }
                    if !out.iter().any(|seen| seen == sentence) {
                        out.push(truncate_chars(sentence, 60));
                    }
                }
            } else {
                if out.len() >= MAX_PERSONALITY {
                    break;
                }
                if !out.iter().any(|seen| seen == &cleaned) {
                    out.push(truncate_chars(&cleaned, 60));
                }
            }
        }
    }
    out
}

fn extract_frequent_phrases(mes_example: &str) -> Vec<String> {
    let stripped = strip_macros(mes_example);
    let mut out = Vec::new();
    for line in stripped.lines() {
        if out.len() >= MAX_PHRASES {
            break;
        }
        let line = line.trim();
        // Prefer {{char}} lines (already macro-stripped → bare dialogue after "Name:" or "char:").
        let spoken = char_spoken_line(line);
        let Some(spoken) = spoken else {
            continue;
        };
        if spoken.chars().count() < 2 || spoken.chars().count() > 60 {
            continue;
        }
        if matches_loop_phrase(&spoken) {
            continue;
        }
        if out.iter().any(|seen| seen == &spoken) {
            continue;
        }
        out.push(spoken);
    }
    out
}

fn char_spoken_line(line: &str) -> Option<String> {
    // Match "Speaker: dialogue" where speaker is not the user.
    let (speaker, rest) = line.split_once(':')?;
    let speaker = speaker.trim();
    let rest = rest.trim();
    if rest.is_empty() || speaker.is_empty() {
        return None;
    }
    let speaker_l = speaker.to_ascii_lowercase();
    if speaker_l.contains("user") || speaker == "플레이어" {
        return None;
    }
    Some(rest.to_string())
}

fn build_avoid_phrases(frequent: &[String]) -> Vec<String> {
    let mut out: Vec<String> = LOOP_PHRASES.iter().map(|s| (*s).to_string()).collect();
    for phrase in frequent {
        if matches_loop_phrase(phrase) && !out.iter().any(|seen| seen == phrase) {
            out.push(phrase.clone());
        }
    }
    out
}

fn matches_loop_phrase(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    LOOP_PHRASES.iter().any(|trap| {
        if trap.is_ascii() {
            lower.contains(&trap.to_ascii_lowercase())
        } else {
            text.contains(trap)
        }
    })
}

fn split_candidate_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut sentences: Vec<String> = line
            .split(|c| c == '.' || c == '。' || c == '!' || c == '！' || c == '?' || c == '？')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if sentences.is_empty() {
            out.push(line.to_string());
        } else if sentences.len() == 1 && !line.contains('.') && !line.contains('。') {
            // No sentence ender — keep the raw line (may still be a bullet).
            out.push(line.to_string());
        } else {
            out.append(&mut sentences);
        }
    }
    out
}

fn strip_bullet(text: &str) -> String {
    let text = text.trim();
    let text = text
        .strip_prefix("- ")
        .or_else(|| text.strip_prefix("* "))
        .or_else(|| text.strip_prefix("• "))
        .or_else(|| text.strip_prefix("· "))
        .unwrap_or(text);
    text.trim().trim_start_matches(['-', '*', '•', '·']).trim().to_string()
}

fn strip_macros(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str("{{");
            out.push_str(after);
            return out;
        };
        let name = after[..end].trim().to_ascii_lowercase();
        // Keep a recognizable speaker label for char lines.
        match name.as_str() {
            "char" => out.push_str("char"),
            "user" => out.push_str("user"),
            _ => {}
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

fn truncate_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let mut s: String = text.chars().take(max.saturating_sub(1)).collect();
    s.push('…');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrich_from_card_extracts_korean_secret_and_goal() {
        let enrichment = enrich_from_card(
            "한유나",
            "카페 나무의 단골이다. 사실은 3년 전 보낸 편지를 아직 숨기고 있다. 비가 오는 날을 좋아한다.",
            "차분하고 관찰력이 좋다. 상처를 농담으로 감춘다.",
            "비가 그친 카페에서 3년 만에 재회했다. 마감 전까지 진실을 말할지 정해야 한다.",
            "<START>\n{{user}}: 오래 기다렸어?\n{{char}}: 세 년 정도?\n{{char}}: 일단 앉아요.",
            &[
                "비밀 — 보내지 못한 편지를 코트 안주머니에 넣고 다닌다.".to_string(),
                "카페 — 대학 시절 단골이었다.".to_string(),
            ],
        );

        assert!(
            enrichment
                .secrets
                .iter()
                .any(|s| s.contains("편지") || s.contains("숨기") || s.contains("비밀")),
            "expected a secret line, got {:?}",
            enrichment.secrets
        );
        assert!(
            enrichment.secrets.len() <= 4,
            "secrets capped: {:?}",
            enrichment.secrets
        );
        assert!(
            enrichment.current_goal.contains("시나리오 압력을 밀고"),
            "{}",
            enrichment.current_goal
        );
        assert!(!enrichment.long_term_desire.is_empty());
        assert!(
            enrichment
                .known_facts
                .iter()
                .any(|f| f.contains("단골") || f.contains("비")),
            "facts: {:?}",
            enrichment.known_facts
        );
        assert!(
            enrichment
                .frequent_phrases
                .iter()
                .any(|p| p.contains("앉") || p.contains("세 년")),
            "phrases: {:?}",
            enrichment.frequent_phrases
        );
        for trap in LOOP_PHRASES {
            assert!(
                enrichment.avoid_phrases.iter().any(|p| p == trap),
                "missing avoid {trap}: {:?}",
                enrichment.avoid_phrases
            );
        }
    }

    #[test]
    fn default_goal_is_not_only_why_call() {
        let enrichment = enrich_from_card("X", "", "", "", "", &[]);
        assert!(!enrichment.current_goal.contains("왜 지금"));
        assert!(!enrichment.current_goal.contains("만나자고"));
        assert!(enrichment.current_goal.contains("실제로 바꾼다"));
    }
}
