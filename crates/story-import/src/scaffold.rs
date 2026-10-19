//! Pack scaffolding: compile a parsed card (+ optional standalone World Info)
//! into a complete StoryTui pack directory.
//!
//! The output always carries the loader's ten required files plus the extras
//! (`lorebook.toml`, `greetings/`, `images/characters/`, `sources/card.json`),
//! and finishes with a `PackLoader` self-check so a bad emit can never ship.

use std::path::{Path, PathBuf};

use serde::Serialize;
use story_model::{
    Lorebook, PackContentPolicy, PackInitialState, PackMetadata, PackRating, TimeSlot,
};

use crate::card::ParsedCard;
use crate::macros;
use crate::report::{ImportReport, MAX_DESCRIPTION_TOKENS, estimate_tokens, non_ascii_ratio};
use crate::worldinfo::NormalizedBook;

/// Import knobs.
pub struct ImportOptions {
    /// `{{user}}` resolves to this address (default `플레이어`).
    pub player_name: String,
    /// BCP-47 tag stamped into `story.toml` (default `ko-KR`).
    pub language: String,
    /// Overwrite an existing output directory.
    pub force: bool,
    /// Optional standalone World Info JSON merged into the book.
    pub lorebook_path: Option<PathBuf>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            player_name: "플레이어".to_string(),
            language: "ko-KR".to_string(),
            force: false,
            lorebook_path: None,
        }
    }
}

/// Emit the pack at `out_dir` and verify it with the pack loader.
pub fn emit_pack(
    parsed: &ParsedCard,
    sidecar_book: Option<NormalizedBook>,
    out_dir: &Path,
    options: &ImportOptions,
    report: &mut ImportReport,
) -> Result<(), String> {
    let card = &parsed.canonical;
    let slug = slugify(&card.name);
    let character_id = slug.clone();

    if out_dir.exists() {
        if !options.force {
            return Err(format!(
                "출력 디렉터리가 이미 존재합니다: {} — 다른 -o 경로를 쓰거나 --force로 덮어쓰세요",
                out_dir.display()
            ));
        }
        std::fs::remove_dir_all(out_dir).map_err(|e| {
            format!(
                "기존 디렉터를 비우지 못했습니다 ({}): {e}",
                out_dir.display()
            )
        })?;
    }
    create_dirs(out_dir)?;

    // --- macro substitution across every prompt-bound text -------------------
    let mut ignored_macros = Vec::new();
    let user_name = options.player_name.trim();
    let mut sub = |text: &str| macros::substitute(text, &card.name, user_name, &mut ignored_macros);
    let description = sub(&card.description);
    let personality_prose = sub(&card.personality);
    let scenario = sub(&card.scenario);
    let opening = sub(&card.first_mes);
    let greetings: Vec<String> = card
        .alternate_greetings
        .iter()
        .map(|text| sub(text))
        .collect();
    let dialogue_examples: Vec<String> = macros::split_dialogue_examples(&sub(&card.mes_example));
    for name in &ignored_macros {
        report.note_ignored_macro(name);
    }

    // --- description token gate ---------------------------------------------
    let mut description_md = compose_description(&description, &personality_prose);
    let description_tokens = estimate_tokens(&description_md);
    if description_tokens > MAX_DESCRIPTION_TOKENS {
        description_md = truncate_tokens(&description_md, MAX_DESCRIPTION_TOKENS);
        report.warn(format!(
            "설명 본문이 토큰 상한({MAX_DESCRIPTION_TOKENS})을 초과해 절단했습니다 \
             (약 {description_tokens} → 상한). 원본은 sources/card.json에 있습니다."
        ));
    }
    report.description_tokens = estimate_tokens(&description_md);

    // --- language gate --------------------------------------------------------
    if options.language.starts_with("ko") {
        let corpus = format!("{description}{scenario}{opening}");
        if !corpus.is_empty() && non_ascii_ratio(&corpus) < 0.1 {
            report.warn(
                "카드 본문이 영어 위주로 보입니다 — language=ko-KR 팩에 영어 캐릭터가 \
                 들어갑니다. --language로 태그를 맞추거나 번역 후 임포트하세요."
                    .to_string(),
            );
        }
    }
    if opening.trim().is_empty() {
        report.warn("카드의 first_mes가 비어 있습니다 — 자리표시자 오프닝을 썼습니다.".to_string());
    }

    // --- merge books ----------------------------------------------------------
    let lorebook = merge_books(card.book.as_ref(), sidecar_book, report);

    // --- emit files -------------------------------------------------------------
    write_story_toml(out_dir, &slug, &card.name, options)?;
    write_characters_toml(
        out_dir,
        &character_id,
        &card.name,
        &description_md,
        &dialogue_examples,
    )?;
    write_rules_toml(out_dir, &character_id)?;
    write_events_toml(out_dir)?;
    write_markdown(
        &out_dir.join("world.md"),
        &world_md(&scenario, &card.name),
        "world.md",
    )?;
    write_markdown(
        &out_dir.join("style.md"),
        include_str!("../assets/style.md"),
        "style.md",
    )?;
    write_markdown(
        &out_dir.join("opening.md"),
        &opening_md(&opening),
        "opening.md",
    )?;
    write_markdown(
        &out_dir.join("prompts/system.md"),
        include_str!("../assets/system.md"),
        "prompts/system.md",
    )?;
    write_markdown(
        &out_dir.join("prompts/director.md"),
        include_str!("../assets/director.md"),
        "prompts/director.md",
    )?;
    write_markdown(
        &out_dir.join("prompts/writer.md"),
        include_str!("../assets/writer.md"),
        "prompts/writer.md",
    )?;
    for (index, greeting) in greetings.iter().enumerate() {
        write_markdown(
            &out_dir.join("greetings").join(format!("{}.md", index + 1)),
            greeting,
            &format!("greetings/{}.md", index + 1),
        )?;
    }
    if !lorebook.entries.is_empty() {
        let toml_text = toml::to_string_pretty(&lorebook)
            .map_err(|e| format!("lorebook.toml을 직렬화하지 못했습니다: {e}"))?;
        std::fs::write(out_dir.join("lorebook.toml"), toml_text)
            .map_err(|e| format!("lorebook.toml을 쓰지 못했습니다: {e}"))?;
        report.lorebook_entries = lorebook.entries.len();
    }

    if let Some(avatar) = &card.avatar {
        let path = out_dir
            .join("images/characters")
            .join(format!("{character_id}.{}", avatar.extension));
        std::fs::write(&path, &avatar.bytes)
            .map_err(|e| format!("아바타를 쓰지 못했습니다 ({}): {e}", path.display()))?;
    }
    if let Some(background) = &card.background {
        // The host's scene-image convention: images/locations/<location-id>
        // wins over the character portrait. Imported packs start at
        // `default-location` (story.toml initial_state).
        let path = out_dir
            .join("images/locations")
            .join(format!("default-location.{}", background.extension));
        std::fs::create_dir_all(out_dir.join("images/locations"))
            .map_err(|e| format!("디렉터를 만들지 못했습니다 ({}): {e}", path.display()))?;
        std::fs::write(&path, &background.bytes)
            .map_err(|e| format!("배경 이미지를 쓰지 못했습니다 ({}): {e}", path.display()))?;
    }

    // --- preserve the raw card ------------------------------------------------
    let raw_json = serde_json::to_string_pretty(&parsed.raw)
        .map_err(|e| format!("원본 카드를 직렬화하지 못했습니다: {e}"))?;
    std::fs::write(out_dir.join("sources/card.json"), raw_json)
        .map_err(|e| format!("sources/card.json을 쓰지 못했습니다: {e}"))?;

    // --- fold parse-time notes ------------------------------------------------
    for (subject, reason) in &parsed.notes {
        report.note_approximation(&format!("{subject} — {reason}"));
    }
    collect_card_policy_notes(&parsed.card_data, report);

    // --- record what mapped ----------------------------------------------------
    report
        .mapped
        .push("description/personality → characters.toml description_md".to_string());
    if !dialogue_examples.is_empty() {
        report.mapped.push(format!(
            "mes_example → dialogue_examples {}개 블록",
            dialogue_examples.len()
        ));
    }
    if !scenario.trim().is_empty() {
        report
            .mapped
            .push("scenario → world.md 시나리오 절".to_string());
    }
    if !opening.trim().is_empty() {
        report.mapped.push("first_mes → opening.md".to_string());
    }
    if !greetings.is_empty() {
        report.mapped.push(format!(
            "alternate_greetings {}개 → greetings/",
            greetings.len()
        ));
    }
    if card.avatar.is_some() {
        report
            .mapped
            .push(format!("아바타 → images/characters/{character_id}.*"));
    }
    if card.background.is_some() {
        report
            .mapped
            .push("배경 자산 → images/locations/default-location.*".to_string());
    }
    if !lorebook.entries.is_empty() {
        report.mapped.push(format!(
            "로어북 엔트리 {}개 → lorebook.toml",
            lorebook.entries.len()
        ));
    }

    // --- self-check: the emit must satisfy the real loader --------------------
    story_persistence::PackLoader::load(out_dir).map_err(|error| {
        format!(
            "임포트된 팩이 로더 검증을 통과하지 못했습니다 ({}) — 디렉터를 남겨 두었으니 \
             확인 후 --force로 다시 임포트하세요:\n{error}",
            out_dir.display()
        )
    })?;
    report.loader_verified = true;
    Ok(())
}

/// Compose `description_md`: description prose, then the personality prose as
/// its own section (the structured lists stay for the normalize pass).
fn compose_description(description: &str, personality_prose: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let description = description.trim();
    if !description.is_empty() {
        parts.push(description.to_string());
    }
    let personality = personality_prose.trim();
    if !personality.is_empty() {
        parts.push(format!("# 성격\n\n{personality}"));
    }
    parts.join("\n\n")
}

/// Cut `text` at roughly `limit` tokens, preferring line boundaries and
/// hard-cutting a single line that alone exceeds the budget.
fn truncate_tokens(text: &str, limit: usize) -> String {
    const MARKER: &str = "…(절단)";
    let mut kept = String::new();
    let mut tokens = 0_usize;
    for line in text.lines() {
        let line_tokens = estimate_tokens(line).max(1);
        let remaining = limit.saturating_sub(tokens);
        if line_tokens <= remaining {
            kept.push_str(line);
            kept.push('\n');
            tokens += line_tokens;
        } else {
            let total_chars = line.chars().count();
            let keep_chars = (total_chars * remaining / line_tokens).max(1);
            let cut: String = line.chars().take(keep_chars).collect();
            kept.push_str(&cut);
            kept.push_str(MARKER);
            break;
        }
    }
    kept.trim_end_matches('\n').to_string()
}

/// Merge the embedded book with the optional sidecar World Info.
fn merge_books(
    embedded: Option<&NormalizedBook>,
    sidecar: Option<NormalizedBook>,
    report: &mut ImportReport,
) -> Lorebook {
    let mut lorebook = Lorebook::default();
    if let Some(embedded) = embedded {
        lorebook.name = embedded.name.clone();
        lorebook.recursive_scanning = embedded.recursive_scanning;
        if let Some(depth) = embedded.scan_depth {
            lorebook.scan_depth = depth;
        }
        if let Some(budget) = embedded.token_budget {
            lorebook.token_budget = budget;
        }
        lorebook.entries.extend(embedded.entries.iter().cloned());
    }
    if let Some(sidecar) = sidecar {
        if lorebook.name.is_empty() {
            lorebook.name = sidecar.name.clone();
        }
        if embedded.is_none() {
            if let Some(depth) = sidecar.scan_depth {
                lorebook.scan_depth = depth;
            }
            if let Some(budget) = sidecar.token_budget {
                lorebook.token_budget = budget;
            }
        }
        report.note_approximation(&format!(
            "World Info {}개 엔트리를 로어북에 병합했습니다",
            sidecar.entries.len()
        ));
        lorebook.entries.extend(sidecar.entries);
    }
    lorebook
}

/// Report the card fields the import policy deliberately ignores.
fn collect_card_policy_notes(card: &crate::card::CardData, report: &mut ImportReport) {
    if !card.system_prompt.trim().is_empty() {
        report.note_ignored_field(
            "system_prompt",
            "기본 무시 정책 — 엔진 안정화 프롬프트 보호 (원본은 sources/에 보존)",
        );
    }
    if !card.post_history_instructions.trim().is_empty() {
        report.note_ignored_field(
            "post_history_instructions",
            "기본 무시 정책 (원본은 sources/에 보존)",
        );
    }
    if !card.creator_notes.trim().is_empty() {
        report.note_ignored_field("creator_notes", "프롬프트 미사용 (V2 스펙 권고)");
    }
    if !card.group_only_greetings.is_empty() {
        report.note_ignored_field("group_only_greetings", "그룹 채팅 미지원");
    }
}

fn create_dirs(out_dir: &Path) -> Result<(), String> {
    for relative in [
        "",
        "prompts",
        "greetings",
        "images",
        "images/characters",
        "sources",
    ] {
        std::fs::create_dir_all(out_dir.join(relative))
            .map_err(|e| format!("디렉터를 만들지 못했습니다 ({}): {e}", out_dir.display()))?;
    }
    Ok(())
}

fn write_story_toml(
    out_dir: &Path,
    slug: &str,
    title: &str,
    options: &ImportOptions,
) -> Result<(), String> {
    let metadata = PackMetadata {
        schema_version: story_model::PACK_SCHEMA_VERSION
            .parse()
            .map_err(|e| format!("PACK_SCHEMA_VERSION을 해석하지 못했습니다: {e}"))?,
        story_id: slug.to_string(),
        story_version: story_model::semver::Version::new(0, 1, 0),
        min_engine_version: story_model::semver::Version::new(0, 1, 0),
        title: title.to_string(),
        language: options.language.clone(),
        genres: vec!["imported-card".to_string()],
        rating: PackRating::Teen,
        default_choice_count: 3,
        initial_state: PackInitialState {
            chapter: "chapter-1".to_string(),
            scene: "opening".to_string(),
            location: story_model::LocationId("default-location".to_string()),
            time_slot: TimeSlot::Evening,
        },
        content: PackContentPolicy {
            violence: "mild".to_string(),
        },
    };
    let toml_text = toml::to_string_pretty(&metadata)
        .map_err(|e| format!("story.toml을 직렬화하지 못했습니다: {e}"))?;
    std::fs::write(out_dir.join("story.toml"), toml_text)
        .map_err(|e| format!("story.toml을 쓰지 못했습니다: {e}"))
}

#[derive(Serialize)]
struct CharactersDoc<'a> {
    schema_version: &'static str,
    characters: Vec<CharacterRow<'a>>,
}

#[derive(Serialize)]
struct CharacterRow<'a> {
    id: &'a str,
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    age: Option<u32>,
    description_md: &'a str,
    initial_player_relationship: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    dialogue_examples: &'a Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    personality: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    appearance: Vec<String>,
    speech_style: String,
    honorific_rule: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    known_facts: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    unknown_facts: Vec<String>,
    current_goal: String,
    long_term_desire: String,
}

fn write_characters_toml(
    out_dir: &Path,
    id: &str,
    name: &str,
    description_md: &str,
    dialogue_examples: &Vec<String>,
) -> Result<(), String> {
    let doc = CharactersDoc {
        schema_version: "1.0.0",
        characters: vec![CharacterRow {
            id,
            name,
            age: None,
            description_md,
            initial_player_relationship: "acquaintance",
            dialogue_examples,
            personality: Vec::new(),
            appearance: Vec::new(),
            speech_style: String::new(),
            honorific_rule: String::new(),
            known_facts: Vec::new(),
            unknown_facts: Vec::new(),
            current_goal: String::new(),
            long_term_desire: String::new(),
        }],
    };
    let toml_text = toml::to_string_pretty(&doc)
        .map_err(|e| format!("characters.toml을 직렬화하지 못했습니다: {e}"))?;
    std::fs::write(out_dir.join("characters.toml"), toml_text)
        .map_err(|e| format!("characters.toml을 쓰지 못했습니다: {e}"))
}

#[derive(Serialize)]
struct RulesDoc {
    schema_version: &'static str,
    limits: LimitsRow,
    locations: Vec<LocationRow>,
    numbers: Vec<NumberRow>,
}

#[derive(Serialize)]
struct LimitsRow {
    affection_min: i16,
    affection_max: i16,
    trust_min: i16,
    trust_max: i16,
    tension_min: i16,
    tension_max: i16,
    normal_delta_min: i16,
    normal_delta_max: i16,
}

#[derive(Serialize)]
struct LocationRow {
    id: &'static str,
    name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    connects_to: Vec<String>,
}

#[derive(Serialize)]
struct NumberRow {
    id: String,
    default: i64,
    min: i64,
    max: i64,
}

fn write_rules_toml(out_dir: &Path, character_id: &str) -> Result<(), String> {
    let doc = RulesDoc {
        schema_version: "1.0.0",
        limits: LimitsRow {
            affection_min: -30,
            affection_max: 30,
            trust_min: -30,
            trust_max: 30,
            tension_min: 0,
            tension_max: 30,
            normal_delta_min: -3,
            normal_delta_max: 3,
        },
        locations: vec![LocationRow {
            id: "default-location",
            name: "이야기의 무대".to_string(),
            connects_to: Vec::new(),
        }],
        numbers: vec![
            NumberRow {
                id: format!("{character_id}.affection"),
                default: 0,
                min: -30,
                max: 30,
            },
            NumberRow {
                id: format!("{character_id}.trust"),
                default: 0,
                min: -30,
                max: 30,
            },
        ],
    };
    let toml_text = toml::to_string_pretty(&doc)
        .map_err(|e| format!("rules.toml을 직렬화하지 못했습니다: {e}"))?;
    std::fs::write(out_dir.join("rules.toml"), toml_text)
        .map_err(|e| format!("rules.toml을 쓰지 못했습니다: {e}"))
}

fn write_events_toml(out_dir: &Path) -> Result<(), String> {
    std::fs::write(
        out_dir.join("events.toml"),
        "schema_version = \"1.0.0\"\n\n",
    )
    .map_err(|e| format!("events.toml을 쓰지 못했습니다: {e}"))
}

fn write_markdown(path: &Path, content: &str, label: &str) -> Result<(), String> {
    std::fs::write(path, content).map_err(|e| format!("{label}을 쓰지 못했습니다: {e}"))
}

fn world_md(scenario: &str, character_name: &str) -> String {
    let mut out = String::from("# 세계관\n\n");
    if scenario.trim().is_empty() {
        out.push_str(
            "카드에 시나리오 정보가 없습니다. 오프닝 장면을 기준으로 세계관을 상상해 진행합니다.\n",
        );
    } else {
        out.push_str("## 시나리오\n\n");
        out.push_str(scenario.trim());
        out.push('\n');
    }
    out.push_str(&format!(
        "\n## 등장인물\n\n- {character_name}: 이 팩의 주요 인물. 캐릭터 정의는 캐논 블록을 따른다.\n"
    ));
    out.push_str(
        "\n## 고정 사실\n\n- 세계의 물리·사회 규칙은 카드가 암시하는 범위 안에서 현실적으로 유지된다.\n",
    );
    out
}

fn opening_md(opening: &str) -> String {
    let mut out = String::from("# 오프닝\n\n");
    if opening.trim().is_empty() {
        out.push_str(
            "카드에 첫 인사가 없다. 캐릭터 정의에 맞는 첫 장면을 스스로 시작하고, \
             플레이어에게 말을 건다.\n",
        );
    } else {
        out.push_str(opening.trim());
        out.push('\n');
    }
    out
}

/// ASCII slug from a card name: lowercase alphanumerics and dashes. Hangul
/// and other non-ASCII names fall back to `card`.
pub fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = true; // trims leading dashes for free
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && (ch.is_whitespace() || ch == '-' || ch == '_') {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        "card".to_string()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_normalizes_and_falls_back() {
        assert_eq!(slugify("Rainy Reunion!!"), "rainy-reunion");
        assert_eq!(slugify("  --Odd_Name--  "), "odd-name");
        assert_eq!(slugify("비가 그친 뒤"), "card", "non-ascii falls back");
        assert_eq!(slugify(""), "card");
    }

    #[test]
    fn description_composes_personality_section() {
        let text = compose_description("카페에서 재회한다.", "차분하고 관찰적이다.");
        assert!(text.contains("카페에서 재회한다."));
        assert!(text.contains("# 성격"));
        assert!(text.contains("차분하고 관찰적이다."));
    }

    #[test]
    fn truncate_keeps_lines_and_marks_the_cut() {
        let long = "가".repeat(50);
        let truncated = truncate_tokens(&long, 10);
        assert!(truncated.ends_with("…(절단)"));
        assert!(truncated.chars().count() < long.chars().count());
    }
}
