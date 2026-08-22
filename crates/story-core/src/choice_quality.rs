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
        "For every choice: write one concrete player action or line; make `intent` name its actual goal; make `preview` a non-empty, spoiler-free Korean hint about what it tests or changes. Use a distinct person, promise, quoted claim, object, place, or time already present in the context as each option's hook. At most one option may be a pure question; the others must manipulate evidence or change the scene conditions. Do not use observation, waiting, silence, small talk, or atmosphere commentary unless it creates a new deadline, discovery, refusal, movement, or relationship consequence. The set must include both a lower-risk option and a medium-or-higher-risk option.\n",
    );
    out
}

/// Compact version of the three dramatic jobs for the low-latency choice-only
/// repair path. The full-turn prompt keeps [`design_prompt`]; this version
/// removes explanatory repetition while preserving objective, cost, and
/// immediate consequence.
pub fn compact_design_prompt(game: &GameState, pack: &StoryPack) -> String {
    let briefs = design_briefs(game, pack);
    let mut out = "# 세 슬롯 (이 순서 고정)\n".to_string();
    for (index, (role, risk)) in [
        (ChoiceRole::Relationship, "high"),
        (ChoiceRole::Evidence, "low"),
        (ChoiceRole::Situation, "medium"),
    ]
    .into_iter()
    .enumerate()
    {
        let brief = briefs
            .iter()
            .find(|brief| brief.role == role)
            .expect("all choice roles are designed");
        let role = match role {
            ChoiceRole::Relationship => "관계",
            ChoiceRole::Evidence => "단서",
            ChoiceRole::Situation => "상황",
        };
        out.push_str(&format!(
            "{}. {role}/{risk}: {} 즉시 결과: {}\n",
            index + 1,
            brief.objective,
            brief.consequence,
        ));
    }
    out
}

/// Deterministic risk metadata for one compact choice slot.
pub fn risk_for_slot(_game: &GameState, _pack: &StoryPack, index: usize) -> RiskLevel {
    match index {
        0 => RiskLevel::High,
        1 => RiskLevel::Low,
        _ => RiskLevel::Medium,
    }
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
    TooGeneric { indices: Vec<usize> },
    TooQuestionHeavy { indices: Vec<usize> },
    MalformedText { indices: Vec<usize> },
    MissingDirections { roles: Vec<String> },
    RoleRiskMismatch { indices: Vec<usize> },
    ImpossibleWorldControl { indices: Vec<usize> },
    MissingPlayerAction { indices: Vec<usize> },
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

    /// Whether a set is safe to display and can advance the game. Subjective
    /// variety notes still trigger one best-effort refresh after a full turn,
    /// but they must not trap a player behind repeated regeneration failures.
    pub fn is_playable(&self) -> bool {
        !self.issues.iter().any(|issue| {
            matches!(
                issue,
                ChoiceQualityIssue::WrongCount { .. }
                    | ChoiceQualityIssue::RepeatsSelected { .. }
                    | ChoiceQualityIssue::DuplicateDirection { .. }
                    | ChoiceQualityIssue::MissingPreview { .. }
                    | ChoiceQualityIssue::MalformedText { .. }
                    | ChoiceQualityIssue::ImpossibleWorldControl { .. }
                    | ChoiceQualityIssue::MissingPlayerAction { .. }
                    | ChoiceQualityIssue::RepeatsRejectedOffer { .. }
            )
        })
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
                ChoiceQualityIssue::TooGeneric { indices } => messages.push(format!(
                    "{}번 선택지가 '솔직히 묻기/왜 지금인지 확인하기' 같은 범용 문장이다. 현재 장면의 인물·약속·물건·장소·시간 중 구체적인 소재를 행동에 넣는다",
                    display_indices(indices)
                )),
                ChoiceQualityIssue::TooQuestionHeavy { indices } => messages.push(format!(
                    "{}번 선택지가 모두 단서 없는 질문이다. 순수한 질문은 하나만 남기고, 나머지는 편지·전시회·시계·마감처럼 현재 장면에 이미 나온 대상을 확인하거나 건네거나 이동시키는 행동으로 바꾼다",
                    display_indices(indices)
                )),
                ChoiceQualityIssue::MalformedText { indices } => messages.push(format!(
                    "{}번 선택지가 잘린 문장, 닫히지 않은 따옴표 또는 영문 역할명(intent)을 포함한다. label은 28자 안의 완결된 한국어 플레이어 행동, intent와 preview도 완결된 한국어 문장으로 다시 쓴다",
                    display_indices(indices)
                )),
                ChoiceQualityIssue::MissingDirections { roles } => messages.push(format!(
                    "선택지 세트에 {} 방향이 없다. 관계 선택지는 고백·사과·경계·직접 질문으로 거리를 바꾸고, 단서 선택지는 편지·메시지·사진·물건을 읽기·대조·꺼내기·보여주기·찾기로 실제 정보를 얻으며, 상황 선택지는 장소·시간·참여자·마감을 바꾼다",
                    roles.join(", ")
                )),
                ChoiceQualityIssue::RoleRiskMismatch { indices } => messages.push(format!(
                    "{}번 선택지의 위험도가 역할과 맞지 않는다. 관계 변화는 medium 이상, 단서는 low 이하, 상황 변화는 medium 이상으로 쓴다",
                    display_indices(indices)
                )),
                ChoiceQualityIssue::ImpossibleWorldControl { indices } => messages.push(format!(
                    "{}번 선택지가 플레이어 권한 밖의 세계를 즉시 바꾼다. 영업시간·날씨·상대 행동을 확정하지 말고 직원에게 부탁하기, 자리를 옮기기, 물건을 건네기처럼 플레이어가 지금 실행할 수 있는 행동으로 바꾼다",
                    display_indices(indices)
                )),
                ChoiceQualityIssue::MissingPlayerAction { indices } => messages.push(format!(
                    "{}번 선택지가 플레이어 행동이 아니라 새 사실이나 상대 반응만 서술한다. 말하기·사과하기·묻기·읽기·대조하기·꺼내기·보여주기·건네기·제안하기·부탁하기·이동하기 중 지금 실행할 행동을 label에 쓴다",
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

    let generic = choices
        .iter()
        .enumerate()
        .filter(|(_, choice)| is_generic(choice))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if generic.len() >= 2 {
        issues.push(ChoiceQualityIssue::TooGeneric { indices: generic });
    }

    let pure_questions = choices
        .iter()
        .enumerate()
        .filter(|(_, choice)| is_pure_question(choice))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if pure_questions.len() >= 2 {
        issues.push(ChoiceQualityIssue::TooQuestionHeavy {
            indices: pure_questions,
        });
    }

    let malformed = choices
        .iter()
        .enumerate()
        .filter(|(_, choice)| has_malformed_text(choice))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if !malformed.is_empty() {
        issues.push(ChoiceQualityIssue::MalformedText { indices: malformed });
    }

    let directions = choices
        .iter()
        .map(crate::narrative::classify_choice)
        .collect::<BTreeSet<_>>();
    let missing_directions = ["relationship", "evidence", "situation"]
        .into_iter()
        .filter(|role| !directions.contains(role))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if !missing_directions.is_empty() {
        issues.push(ChoiceQualityIssue::MissingDirections {
            roles: missing_directions,
        });
    }

    let role_risk_mismatch = choices
        .iter()
        .enumerate()
        .filter(
            |(_, choice)| match crate::narrative::classify_choice(choice) {
                "relationship" | "situation" => {
                    matches!(choice.risk, RiskLevel::Safe | RiskLevel::Low)
                }
                "evidence" => matches!(
                    choice.risk,
                    RiskLevel::Medium | RiskLevel::High | RiskLevel::Severe
                ),
                _ => false,
            },
        )
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if !role_risk_mismatch.is_empty() {
        issues.push(ChoiceQualityIssue::RoleRiskMismatch {
            indices: role_risk_mismatch,
        });
    }

    let impossible_world_control = choices
        .iter()
        .enumerate()
        .filter(|(_, choice)| controls_world_without_an_action(choice))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if !impossible_world_control.is_empty() {
        issues.push(ChoiceQualityIssue::ImpossibleWorldControl {
            indices: impossible_world_control,
        });
    }

    let missing_player_action = choices
        .iter()
        .enumerate()
        .filter(|(_, choice)| !has_player_action(choice))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if !missing_player_action.is_empty() {
        issues.push(ChoiceQualityIssue::MissingPlayerAction {
            indices: missing_player_action,
        });
    }

    if let Some(rejected) = rejected_offers {
        for (index, choice) in choices.iter().enumerate() {
            if rejected
                .iter()
                .any(|old| labels_repeat(&old.label, &choice.label))
            {
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

fn labels_repeat(left: &str, right: &str) -> bool {
    let left_compact = compact_text(left);
    let right_compact = compact_text(right);
    if left_compact.is_empty() || right_compact.is_empty() {
        return false;
    }
    left_compact == right_compact
        || (left_compact.chars().count() >= 10 && right_compact.contains(&left_compact))
        || (right_compact.chars().count() >= 10 && left_compact.contains(&right_compact))
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

fn is_generic(choice: &Choice) -> bool {
    let text = format!("{} {}", choice.label, choice.intent).to_lowercase();
    const GENERIC: &[&str] = &[
        "지금까지의 이야기",
        "솔직하게 묻",
        "조심스럽게 묻",
        "왜 지금",
        "진짜 이유",
        "다른 가능성",
        "대화를 이어",
        "말을 건넨",
        "몇 시인지",
        "얼마 지났는지",
        "현재 상황",
        "반응을 확인",
        "마음을 확인",
        "상황을 확인",
        "의도를 확인",
    ];
    GENERIC.iter().any(|needle| text.contains(needle))
}

/// A question can be a strong relationship choice, but two or more abstract
/// questions make the set feel like a dialogue menu instead of three
/// materially different actions. Questions tied to a visible object, quoted
/// claim, concrete deadline, or named story fact remain valid.
fn is_pure_question(choice: &Choice) -> bool {
    let text = format!("{} {}", choice.label, choice.intent).to_lowercase();
    const QUESTION: &[&str] = &["묻", "물어", "질문", "확인"];
    const CONCRETE_HOOK_OR_ACTION: &[&str] = &[
        "편지",
        "봉투",
        "전시",
        "시계",
        "사진",
        "메시지",
        "휴대전화",
        "핸드폰",
        "우산",
        "영수증",
        "열쇠",
        "주소",
        "날짜",
        "마감",
        "카페",
        "강변",
        "서울",
        "3년",
        "세 번",
        "건넨",
        "꺼낸",
        "보여",
        "열어",
        "찾",
        "전화",
        "주문",
        "이동",
        "나간",
    ];
    QUESTION.iter().any(|needle| text.contains(needle))
        && !CONCRETE_HOOK_OR_ACTION
            .iter()
            .any(|needle| text.contains(needle))
}

fn has_malformed_text(choice: &Choice) -> bool {
    let label = choice.label.trim();
    let intent = choice.intent.trim();
    let preview = choice.preview.as_deref().unwrap_or_default().trim();
    let intent_is_ascii_role = !intent.is_empty()
        && intent
            .chars()
            .all(|character| character.is_ascii() || character.is_whitespace());
    label.chars().count() >= 36
        || intent_is_ascii_role
        || !label.matches('\'').count().is_multiple_of(2)
        || !label.matches('"').count().is_multiple_of(2)
        || !is_complete_korean_phrase(label)
        || !is_complete_korean_phrase(preview)
}

fn controls_world_without_an_action(choice: &Choice) -> bool {
    let text = format!("{} {}", choice.label, choice.intent).to_lowercase();
    let changes_business_hours = text.contains("마감 시간")
        && ["정한다", "잡는다", "바꾼다", "앞당", "늦춘"]
            .iter()
            .any(|needle| text.contains(needle));
    let changes_weather = ["비를 그치게", "날씨를 바꾼", "비가 멈추게"]
        .iter()
        .any(|needle| text.contains(needle));
    let negotiated = ["부탁", "요청", "직원에게", "상의", "협상"]
        .iter()
        .any(|needle| text.contains(needle));
    (changes_business_hours || changes_weather) && !negotiated
}

fn has_player_action(choice: &Choice) -> bool {
    let label = choice.label.to_lowercase();
    const ACTIONS: &[&str] = &[
        "말한다",
        "밝힌다",
        "인정한다",
        "고백한다",
        "사과한다",
        "묻는다",
        "질문한다",
        "읽는다",
        "읽어본다",
        "대조한다",
        "확인한다",
        "찾는다",
        "꺼낸다",
        "보여준다",
        "건넨다",
        "제안한다",
        "부탁한다",
        "요청한다",
        "요구한다",
        "해명한다",
        "약속한다",
        "이동한다",
        "나간다",
        "들어간다",
        "앉는다",
        "일어선다",
        "열어본다",
        "전화한다",
        "적는다",
        "주문한다",
        "계산한다",
        "가리킨다",
        "내민다",
        "돌려준다",
        "놓는다",
        "집어든다",
        "기록한다",
        "거절한다",
    ];
    ACTIONS.iter().any(|action| label.contains(action))
}

fn is_complete_korean_phrase(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    if matches!(
        trimmed.chars().last(),
        Some('.' | '!' | '?' | '…' | '\'' | '"')
    ) {
        return true;
    }
    const COMPLETE_ENDINGS: &[&str] = &[
        "다",
        "요",
        "한다",
        "된다",
        "본다",
        "준다",
        "둔다",
        "낸다",
        "간다",
        "온다",
        "선다",
        "앉는다",
        "묻는다",
        "말한다",
        "확인한다",
        "제안한다",
        "건넨다",
        "꺼낸다",
        "가리킨다",
        "열어본다",
        "나선다",
        "수 있다",
        "지 모른다",
        "음",
        "함",
        "됨",
    ];
    COMPLETE_ENDINGS
        .iter()
        .any(|ending| trimmed.ends_with(ending))
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

    #[test]
    fn two_vague_questions_are_rejected_even_with_distinct_risks() {
        let pack = crate::tests_support::sample_pack();
        let game = crate::engine::EngineState::initial(&pack).game;
        let choices = vec![
            choice(
                "지금까지의 이야기를 솔직하게 묻는다",
                "상대가 찾아온 진짜 이유를 확인한다",
                RiskLevel::High,
                "솔직함이 신뢰를 바꿀지 시험한다",
            ),
            choice(
                "왜 지금 부른 건지 다른 가능성도 확인한다",
                "상대의 의도를 확인한다",
                RiskLevel::Low,
                "새 정보를 얻거나 캐묻는 인상을 준다",
            ),
            choice(
                "카페 마감 방송이 나오기 전에 우산을 건넨다",
                "남은 대화 시간을 행동으로 확보한다",
                RiskLevel::Medium,
                "우산을 받을지 거절할지 즉시 드러난다",
            ),
        ];

        let report = assess_choices(&game, &choices, 3, None);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| matches!(issue, ChoiceQualityIssue::TooGeneric { indices } if indices == &[0, 1]))
        );
        assert!(
            report.is_playable(),
            "subjective variety notes must not block an otherwise usable set"
        );
    }

    #[test]
    fn two_abstract_live_questions_are_rejected_but_a_fact_bound_question_is_allowed() {
        let pack = crate::tests_support::sample_pack();
        let game = crate::engine::EngineState::initial(&pack).game;
        let choices = vec![
            choice(
                "지금 왜 다시 만나야 하는지 직접 물어본다",
                "재회의 이유와 관계 의도를 확인한다",
                RiskLevel::Medium,
                "대답을 요구해 관계의 거리를 바꾼다",
            ),
            choice(
                "유나가 지금 가장 바쁜 일을 묻는다",
                "현재 상황을 확인한다",
                RiskLevel::Low,
                "대화의 실마리를 얻는다",
            ),
            choice(
                "전시회 날 오지 않았다는 말을 정확히 언제 들었는지 묻는다",
                "전시회에 얽힌 시간 불일치를 확인한다",
                RiskLevel::High,
                "기억의 차이를 구체적인 날짜로 좁힌다",
            ),
        ];

        let report = assess_choices(&game, &choices, 3, None);
        assert!(report.issues.iter().any(
            |issue| matches!(issue, ChoiceQualityIssue::TooGeneric { indices } if indices == &[0, 1])
        ));
        assert!(report.issues.iter().any(
            |issue| matches!(issue, ChoiceQualityIssue::TooQuestionHeavy { indices } if indices == &[0, 1])
        ));
    }

    #[test]
    fn truncated_live_choice_text_and_ascii_role_intents_are_rejected() {
        let pack = crate::tests_support::sample_pack();
        let game = crate::engine::EngineState::initial(&pack).game;
        let choices = vec![
            choice(
                "고개를 숙여, '지금 바로 말할 수 있는 건 아니지만, 네가 궁금",
                "safe_advance",
                RiskLevel::Low,
                "유나가 진지한 눈으로 기다리며, 플레이어의 진심은 인정한다",
            ),
            choice(
                "유나가 '지금 바로 말하지 않으면, 내 시선만 봐도 네가',",
                "relationship_risk",
                RiskLevel::High,
                "유나가 손목 시계를 멈추며, 플레이어의 어깨를 살짝 툭",
            ),
            choice(
                "유나가 '강변 산책로, 네가 내게 '왜'를' 먼저",
                "scene_shift",
                RiskLevel::High,
                "유나가 창가 쪽을 바라보며, 비 소리가 더 크게 들",
            ),
        ];

        let report = assess_choices(&game, &choices, 3, None);
        assert!(report.issues.iter().any(
            |issue| matches!(issue, ChoiceQualityIssue::MalformedText { indices } if indices == &[0, 1, 2])
        ));
    }

    #[test]
    fn asking_about_a_prop_does_not_replace_an_evidence_action() {
        let pack = crate::tests_support::sample_pack();
        let game = crate::engine::EngineState::initial(&pack).game;
        let choices = vec![
            choice(
                "왜 지금 찾아왔는지 솔직하게 말한다",
                "유나의 질문에 직접 답한다",
                RiskLevel::High,
                "진심이 관계의 거리를 바꿀 수 있다.",
            ),
            choice(
                "유나가 시계를 보는 이유를 묻는다",
                "유나의 행동에 관해 질문한다",
                RiskLevel::Low,
                "시계가 의미하는 시간을 알 수 있다.",
            ),
            choice(
                "카페가 닫기 전에 강변 산책을 제안한다",
                "장소를 바꾸어 대화 조건을 바꾼다",
                RiskLevel::Medium,
                "지금 질문을 미룬 채 장소가 달라진다.",
            ),
        ];

        let report = assess_choices(&game, &choices, 3, None);
        assert!(
            report.issues.iter().any(|issue| matches!(
                issue,
                ChoiceQualityIssue::MissingDirections { roles } if roles == &["evidence"]
            )),
            "{report:?}"
        );
    }

    #[test]
    fn role_risk_and_player_world_authority_are_enforced() {
        let pack = crate::tests_support::sample_pack();
        let game = crate::engine::EngineState::initial(&pack).game;
        let choices = vec![
            choice(
                "유나의 실망을 인정하며 사과한다",
                "전시회 날의 오해를 직접 사과한다",
                RiskLevel::Low,
                "유나의 방어선이 더 단단해질 수 있다.",
            ),
            choice(
                "유나의 편지를 꺼내 읽어본다",
                "편지의 실제 내용을 확인한다",
                RiskLevel::Low,
                "과거에 관한 새 사실을 얻는다.",
            ),
            choice(
                "카페 마감 시간을 일찍 잡는다",
                "마감 시간을 십 분 앞당겨 바꾼다",
                RiskLevel::Medium,
                "대화할 수 있는 시간이 줄어든다.",
            ),
        ];

        let report = assess_choices(&game, &choices, 3, None);
        assert!(report.issues.iter().any(
            |issue| matches!(issue, ChoiceQualityIssue::RoleRiskMismatch { indices } if indices == &[0])
        ));
        assert!(report.issues.iter().any(
            |issue| matches!(issue, ChoiceQualityIssue::ImpossibleWorldControl { indices } if indices == &[2])
        ));
    }

    #[test]
    fn a_new_fact_is_not_a_player_choice() {
        let pack = crate::tests_support::sample_pack();
        let game = crate::engine::EngineState::initial(&pack).game;
        let choices = vec![
            choice(
                "전시회 약속을 어긴 일을 사과한다",
                "관계의 오해를 직접 다룬다",
                RiskLevel::High,
                "사과를 받아들일지 즉시 드러난다.",
            ),
            choice(
                "보내지 못한 편지를 꺼내 읽어본다",
                "편지의 실제 내용을 확인한다",
                RiskLevel::Low,
                "과거에 관한 새 사실을 얻는다.",
            ),
            choice(
                "카페가 오 분 후 닫는다는 사실을 알게 된다",
                "마감이라는 새 상황이 생긴다",
                RiskLevel::Medium,
                "대화할 수 있는 시간이 줄어든다.",
            ),
        ];

        let report = assess_choices(&game, &choices, 3, None);
        assert!(report.issues.iter().any(
            |issue| matches!(issue, ChoiceQualityIssue::MissingPlayerAction { indices } if indices == &[2])
        ));
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
