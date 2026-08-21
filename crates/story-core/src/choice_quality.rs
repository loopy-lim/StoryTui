//! Engine-owned choice design and fun-quality checks.
//!
//! A local model is good at phrasing an option, but it should not own the
//! dramatic shape of the entire choice set. This module gives every turn three
//! deterministic dramatic jobs (relationship, evidence, situation) and rejects
//! output that is technically valid yet offers no real trade-off.

use std::collections::BTreeSet;

use story_model::{Choice, GameState, RiskLevel, StoryPack};

/// Dramatic job assigned to one choice slot before the model writes its label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceRole {
    Relationship,
    Evidence,
    Situation,
}

impl ChoiceRole {
    fn label(self) -> &'static str {
        match self {
            Self::Relationship => "relationship",
            Self::Evidence => "evidence",
            Self::Situation => "situation",
        }
    }
}

/// One engine-authored design brief. The model may phrase it naturally but may
/// not remove its cost or collapse its consequence into another slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceDesignBrief {
    pub role: ChoiceRole,
    pub objective: String,
    pub cost: String,
    pub consequence: String,
    pub risk_hint: &'static str,
}

/// Deterministically design three materially different directions for the
/// current state. Order rotates by turn to avoid a permanent "best first"
/// position while the roles and their dramatic obligations remain stable.
pub fn design_briefs(game: &GameState, pack: &StoryPack) -> Vec<ChoiceDesignBrief> {
    let pressure = crate::narrative::pressure(game)
        .filter(|text| !text.trim().is_empty())
        .unwrap_or("현재 관계의 미해결 질문")
        .to_string();
    let character_goal = pack
        .characters
        .iter()
        .find(|(id, _)| game.characters.contains_key(*id))
        .map(|(_, character)| {
            if character.current_goal.trim().is_empty() {
                "상대의 현재 의도".to_string()
            } else {
                character.current_goal.clone()
            }
        })
        .unwrap_or_else(|| "상대의 현재 의도".to_string());
    let next_place = pack
        .rules
        .locations
        .get(&game.location)
        .and_then(|location| location.adjacent.iter().next())
        .and_then(|id| pack.rules.locations.get(id))
        .map(|location| location.display_name.clone())
        .unwrap_or_else(|| "현재 장소의 시간 압력이나 제삼의 변수".to_string());

    let mut briefs = vec![
        ChoiceDesignBrief {
            role: ChoiceRole::Relationship,
            objective: format!(
                "현재 압력: {pressure}. 여기에 직접 닿는 질문, 솔직한 인정, 경계 설정 또는 제안으로 관계의 거리를 실제로 바꾼다"
            ),
            cost: "상대가 거절하거나 긴장이 높아질 수 있고, 플레이어도 취약함을 드러낸다"
                .to_string(),
            consequence: format!(
                "상대는 중립적인 맞장구가 아니라 답변·거절·역질문 중 하나로 자신의 목표({character_goal})를 드러낸다"
            ),
            risk_hint: "medium 또는 high",
        },
        ChoiceDesignBrief {
            role: ChoiceRole::Evidence,
            objective: format!(
                "현재 압력: {pressure}. 이와 연결된 구체적인 말, 물건, 시간 또는 불일치를 확인해 새 정보를 얻거나 가능성 하나를 지운다"
            ),
            cost: "지나치게 캐묻거나 본론을 피하는 사람으로 보일 수 있다".to_string(),
            consequence: "다음 장면에서 사용할 수 있는 구체적 단서 하나가 생기거나 기존 추측 하나가 틀렸음이 드러난다"
                .to_string(),
            risk_hint: "low 또는 medium",
        },
        ChoiceDesignBrief {
            role: ChoiceRole::Situation,
            objective: format!(
                "사용 가능한 장면 변화 자원: {next_place}. 이동, 마감, 제삼자, 제안 또는 즉시 행동으로 장면 조건을 바꾼다"
            ),
            cost: "현재 질문을 미뤄 오해를 남기거나 대화할 수 있는 시간을 잃을 수 있다".to_string(),
            consequence: "장소·시간·참여자·즉시 목표 중 하나가 달라져 다음 장면이 같은 대화의 반복이 되지 않는다"
                .to_string(),
            risk_hint: "medium 또는 high",
        },
    ];
    let len = briefs.len();
    briefs.rotate_left((game.turn as usize) % len);
    briefs
}

/// Render the engine-owned design into the model prompt.
pub fn design_prompt(
    game: &GameState,
    pack: &StoryPack,
    selected_choice: Option<&Choice>,
) -> String {
    let briefs = design_briefs(game, pack);
    let recent_roles = crate::narrative::recent_choice_roles(game);
    let last_selected = selected_choice
        .map(|choice| choice.label.as_str())
        .or_else(|| {
            game.last_choice
                .as_ref()
                .map(|choice| choice.label.as_str())
        })
        .unwrap_or("(none)");
    let mut out = String::from(
        "The engine assigned the dramatic job of each choice slot. Keep this order and do not collapse two slots into the same likely next scene.\n",
    );
    for (index, brief) in briefs.iter().enumerate() {
        out.push_str(&format!(
            "{}. role={}; objective={}; cost={}; immediate_consequence={}; risk={}\n",
            index + 1,
            brief.role.label(),
            brief.objective,
            brief.cost,
            brief.consequence,
            brief.risk_hint,
        ));
    }
    out.push_str(&format!(
        "Recently chosen directions: {}. Never offer the last selected action again, even as a paraphrase: {}.\n",
        if recent_roles.is_empty() {
            "none".to_string()
        } else {
            recent_roles.join(", ")
        },
        last_selected,
    ));
    out.push_str(
        "For every choice: write one concrete player action or line; make `intent` name its actual goal; make `preview` a non-empty, spoiler-free Korean hint about what it tests or changes. Do not use observation, waiting, silence, small talk, or atmosphere commentary unless it creates a new deadline, discovery, refusal, movement, or relationship consequence. The set must include both a lower-risk option and a medium-or-higher-risk option.\n",
    );
    out
}

/// A technically valid but dramatically weak property of a choice set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChoiceQualityIssue {
    WrongCount { expected: usize, actual: usize },
    RepeatsSelected { index: usize },
    DuplicateDirection { left: usize, right: usize },
    MissingPreview { indices: Vec<usize> },
    NoRiskTradeoff,
    TooPassive { indices: Vec<usize> },
    RepeatsRejectedOffer { index: usize },
}

/// Result of the deterministic pre-display choice quality gate.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChoiceQualityReport {
    pub issues: Vec<ChoiceQualityIssue>,
}

impl ChoiceQualityReport {
    pub fn is_ok(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn repair_instructions(&self) -> String {
        let mut messages = Vec::new();
        for issue in &self.issues {
            match issue {
                ChoiceQualityIssue::WrongCount { expected, actual } => messages.push(format!(
                    "선택지는 정확히 {expected}개여야 하지만 {actual}개다"
                )),
                ChoiceQualityIssue::RepeatsSelected { index } => messages.push(format!(
                    "{}번 선택지가 플레이어가 방금 고른 행동을 다시 제시한다",
                    index + 1
                )),
                ChoiceQualityIssue::DuplicateDirection { left, right } => messages.push(format!(
                    "{}번과 {}번 선택지가 표현만 다르고 같은 결과로 이어진다",
                    left + 1,
                    right + 1
                )),
                ChoiceQualityIssue::MissingPreview { indices } => messages.push(format!(
                    "{}번 선택지의 preview가 비어 있다. 결과를 확정하지 않는 범위에서 무엇을 시험하거나 바꾸는지 쓴다",
                    display_indices(indices)
                )),
                ChoiceQualityIssue::NoRiskTradeoff => messages.push(
                    "세 선택지의 위험도가 모두 비슷하다. 최소 하나는 low 이하, 최소 하나는 medium 이상으로 서로 다른 대가를 만든다"
                        .to_string(),
                ),
                ChoiceQualityIssue::TooPassive { indices } => messages.push(format!(
                    "{}번 선택지가 관찰·기다리기·분위기 언급에 머문다. 적어도 둘은 즉각적인 답변, 단서, 이동, 마감 또는 관계 변화를 일으켜야 한다",
                    display_indices(indices)
                )),
                ChoiceQualityIssue::RepeatsRejectedOffer { index } => messages.push(format!(
                    "{}번 선택지가 이미 거절된 제안을 다시 표현했다",
                    index + 1
                )),
            }
        }
        messages.join("\n- ")
    }
}

/// Assess choices against count, immediate history and dramatic trade-off.
///
/// Empty intents are used only by old core fixtures and pre-contract saves.
/// Runtime provider responses always receive a non-empty intent during parsing,
/// so those compatibility values get only the count check instead of being
/// mistaken for real model output.
pub fn assess_choices(
    game: &GameState,
    choices: &[Choice],
    expected_count: usize,
    rejected_offers: Option<&[Choice]>,
) -> ChoiceQualityReport {
    let mut issues = Vec::new();
    if choices.len() != expected_count {
        issues.push(ChoiceQualityIssue::WrongCount {
            expected: expected_count,
            actual: choices.len(),
        });
    }
    let structured = !choices.is_empty()
        && choices
            .iter()
            .all(|choice| !choice.intent.trim().is_empty());
    if !structured {
        return ChoiceQualityReport { issues };
    }

    if let Some(selected) = &game.last_choice {
        for (index, choice) in choices.iter().enumerate() {
            if text_matches(
                &selected.label,
                &format!("{} {}", choice.label, choice.intent),
            ) {
                issues.push(ChoiceQualityIssue::RepeatsSelected { index });
            }
        }
    }

    'pairs: for left in 0..choices.len() {
        for right in left + 1..choices.len() {
            if choices_match(&choices[left], &choices[right]) {
                issues.push(ChoiceQualityIssue::DuplicateDirection { left, right });
                break 'pairs;
            }
        }
    }

    let missing_preview = choices
        .iter()
        .enumerate()
        .filter(|(_, choice)| {
            choice
                .preview
                .as_deref()
                .is_none_or(|preview| preview.trim().is_empty())
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if !missing_preview.is_empty() {
        issues.push(ChoiceQualityIssue::MissingPreview {
            indices: missing_preview,
        });
    }

    let has_lower_risk = choices
        .iter()
        .any(|choice| matches!(choice.risk, RiskLevel::Safe | RiskLevel::Low));
    let has_meaningful_risk = choices.iter().any(|choice| {
        matches!(
            choice.risk,
            RiskLevel::Medium | RiskLevel::High | RiskLevel::Severe
        )
    });
    if !has_lower_risk || !has_meaningful_risk {
        issues.push(ChoiceQualityIssue::NoRiskTradeoff);
    }

    let passive = choices
        .iter()
        .enumerate()
        .filter(|(_, choice)| is_passive(choice))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if passive.len() >= 2 {
        issues.push(ChoiceQualityIssue::TooPassive { indices: passive });
    }

    if let Some(rejected) = rejected_offers {
        for (index, choice) in choices.iter().enumerate() {
            if rejected.iter().any(|old| choices_match(old, choice)) {
                issues.push(ChoiceQualityIssue::RepeatsRejectedOffer { index });
            }
        }
    }

    ChoiceQualityReport { issues }
}

fn choices_match(left: &Choice, right: &Choice) -> bool {
    if text_matches(&left.label, &right.label) {
        return true;
    }
    let left_role = crate::narrative::classify_choice(left);
    let right_role = crate::narrative::classify_choice(right);
    left_role == right_role && text_matches(&left.intent, &right.intent)
}

fn text_matches(left: &str, right: &str) -> bool {
    let left_compact = compact_text(left);
    let right_compact = compact_text(right);
    if left_compact.is_empty() || right_compact.is_empty() {
        return false;
    }
    if left_compact == right_compact
        || (left_compact.chars().count() >= 8 && right_compact.contains(&left_compact))
        || (right_compact.chars().count() >= 8 && left_compact.contains(&right_compact))
    {
        return true;
    }
    let left_words = choice_words(left);
    let right_words = choice_words(right);
    if left_words.is_empty() || right_words.is_empty() {
        return false;
    }
    let shared = left_words.intersection(&right_words).count();
    shared >= 2 && shared * 2 >= left_words.len().min(right_words.len())
}

fn compact_text(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn choice_words(text: &str) -> BTreeSet<String> {
    text.split_whitespace().filter_map(normalize_word).collect()
}

fn normalize_word(word: &str) -> Option<String> {
    const PARTICLES: &[&str] = &[
        "에서는",
        "으로는",
        "에게는",
        "까지는",
        "부터는",
        "에게",
        "에서",
        "으로",
        "처럼",
        "보다",
        "은",
        "는",
        "이",
        "가",
        "을",
        "를",
        "에",
        "의",
        "도",
        "와",
        "과",
        "로",
        "만",
    ];
    const STOPWORDS: &[&str] = &[
        "유나", "당신", "그녀", "먼저", "다시", "조금", "현재", "대화", "선택", "상태",
    ];
    let mut normalized = word
        .trim_matches(|character: char| !character.is_alphanumeric())
        .to_lowercase();
    for particle in PARTICLES {
        if let Some(stem) = normalized.strip_suffix(particle)
            && stem.chars().count() >= 2
        {
            normalized = stem.to_string();
            break;
        }
    }
    if normalized.chars().count() < 2 || STOPWORDS.contains(&normalized.as_str()) {
        None
    } else {
        Some(normalized)
    }
}

fn is_passive(choice: &Choice) -> bool {
    let text = format!("{} {}", choice.label, choice.intent).to_lowercase();
    const PASSIVE: &[&str] = &[
        "기다",
        "바라본",
        "살핀",
        "관찰",
        "침묵",
        "언급",
        "분위기",
        "대답하지",
        "가만히",
        "지켜본",
        "생각한다",
        "고개를 끄덕",
    ];
    const CONSEQUENCE: &[&str] = &[
        "묻",
        "말한다",
        "밝힌",
        "제안",
        "요구",
        "거절",
        "사과",
        "이동",
        "나간",
        "건넨",
        "찾",
        "열어",
        "보여",
        "부른",
        "약속",
        "정한다",
        "바꾼",
    ];
    PASSIVE.iter().any(|needle| text.contains(needle))
        && !CONSEQUENCE.iter().any(|needle| text.contains(needle))
}

fn display_indices(indices: &[usize]) -> String {
    indices
        .iter()
        .map(|index| (index + 1).to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_live_bland_choices_are_rejected_for_specific_reasons() {
        let fixture: story_model::serde_json::Value = story_model::serde_json::from_str(
            include_str!("../tests/fixtures/uninteresting_choices_latest.json"),
        )
        .expect("live choice fixture");
        let selected_label = fixture["selected_label"]
            .as_str()
            .expect("selected label")
            .to_string();
        let choices: Vec<Choice> = story_model::serde_json::from_value(fixture["choices"].clone())
            .expect("fixture choices");
        let pack = crate::tests_support::sample_pack();
        let mut game = crate::engine::EngineState::initial(&pack).game;
        game.last_choice = Some(story_model::ChoiceRef {
            choice_id: "choice-2".to_string(),
            label: selected_label,
            turn: 1,
        });

        let report = assess_choices(&game, &choices, 3, None);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| matches!(issue, ChoiceQualityIssue::RepeatsSelected { index: 0 }))
        );
        assert!(
            report
                .issues
                .iter()
                .any(|issue| matches!(issue, ChoiceQualityIssue::MissingPreview { .. }))
        );
        assert!(report.issues.contains(&ChoiceQualityIssue::NoRiskTradeoff));
        assert!(
            report
                .issues
                .iter()
                .any(|issue| matches!(issue, ChoiceQualityIssue::TooPassive { .. }))
        );
    }

    #[test]
    fn a_costed_three_direction_set_passes() {
        let pack = crate::tests_support::sample_pack();
        let game = crate::engine::EngineState::initial(&pack).game;
        let choices = vec![
            choice(
                "3년 전 일을 먼저 사과해 유나의 답을 요구한다",
                "회피하던 갈등을 직접 다룬다",
                RiskLevel::High,
                "솔직함이 신뢰와 긴장 중 어느 쪽을 움직일지 시험한다",
            ),
            choice(
                "젖은 편지 봉투의 발신인을 확인한다",
                "탁자 위 단서의 출처를 확인한다",
                RiskLevel::Low,
                "새 사실 하나를 얻는 대신 캐묻는 인상을 줄 수 있다",
            ),
            choice(
                "카페가 닫기 전에 강변을 걷자고 제안한다",
                "장소와 대화의 조건을 바꾼다",
                RiskLevel::Medium,
                "현재 질문을 미룬 채 다음 장면의 장소가 달라진다",
            ),
        ];
        assert!(assess_choices(&game, &choices, 3, None).is_ok());
    }

    fn choice(label: &str, intent: &str, risk: RiskLevel, preview: &str) -> Choice {
        Choice {
            id: label.to_string(),
            label: label.to_string(),
            intent: intent.to_string(),
            tone: None,
            target: None,
            risk,
            hidden_requirements: Vec::new(),
            preview: Some(preview.to_string()),
        }
    }
}
