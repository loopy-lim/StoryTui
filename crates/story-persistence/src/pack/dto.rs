//! Raw TOML DTOs for a Story Pack (W4, ST-012).
//!
//! These structs mirror the on-disk TOML field names from the Technical Spec
//! §6.2 / §6.3 and the example doc `docs/StoryTUI_Example_Story_Pack_KO.md`.
//! They are intentionally permissive (no `deny_unknown_fields`) so that
//! exploratory author-supplied keys do not break loading. Converters in
//! [`crate::pack::dto`] lift these into the canonical `story-model` types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use story_model::{
    CharacterId, CompareOp, ConditionExpr, EventId, LocationId, PackContentPolicy,
    PackInitialState, PackMetadata, PackRating, RelationshipStage, TimeSlot,
};

// ---------------------------------------------------------------------------
// story.toml
// ---------------------------------------------------------------------------

/// Raw `[content]` section of `story.toml`. Unsupported extra keys are
/// accepted and dropped for forward compatibility.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContentToml {
    pub violence: Option<String>,
}

/// Raw `[files]` section of `story.toml`. Every entry is optional; the
/// loader fills in canonical defaults. Strings are validated for path
/// safety (no `..`, no absolute paths) before use.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FilesToml {
    #[serde(default)]
    pub characters: Option<String>,
    #[serde(default)]
    pub rules: Option<String>,
    #[serde(default)]
    pub events: Option<String>,
    #[serde(default)]
    pub world: Option<String>,
    #[serde(default)]
    pub style: Option<String>,
    #[serde(default)]
    pub opening: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub director_prompt: Option<String>,
    #[serde(default)]
    pub writer_prompt: Option<String>,
}

/// Raw `[initial_state]` section of `story.toml`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InitialStateToml {
    pub chapter: String,
    pub scene: String,
    pub location: String,
    pub time_slot: String,
    #[serde(default)]
    pub active_route: Option<String>,
}

/// Raw top-level `story.toml` DTO. Carries every field the loader needs;
/// other author-supplied keys (e.g. `author`, `[files]`, `[generation]`)
/// are tolerated.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoryToml {
    pub schema_version: String,
    pub story_id: String,
    pub story_version: String,
    pub min_engine_version: String,
    pub title: String,
    pub language: String,
    #[serde(default)]
    pub genres: Vec<String>,
    pub rating: String,
    #[serde(default = "default_choice_count")]
    pub default_choice_count: u8,
    pub initial_state: InitialStateToml,
    #[serde(default)]
    pub content: ContentToml,
    #[serde(default)]
    pub files: FilesToml,
}

fn default_choice_count() -> u8 {
    3
}

const MIN_CHOICE_COUNT: u8 = 1;
const MAX_CHOICE_COUNT: u8 = 6;

// ---------------------------------------------------------------------------
// characters.toml
// ---------------------------------------------------------------------------

/// One `[[characters]]` entry in `characters.toml`. The DTO uses a plain
/// string for `initial_player_relationship`; the converter parses it into a
/// [`RelationshipStage`]. Extra keys (`role`, `is_major`,
/// `emotion_examples`, `relationships`, `pronouns`, ...) are accepted.
/// `pronouns` is read into the canonical type.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CharacterToml {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub age: Option<u32>,
    #[serde(default)]
    pub pronouns: Option<String>,
    /// Free-form description prose (imported card `description`).
    #[serde(default)]
    pub description_md: Option<String>,
    /// Example dialogue blocks (imported card `mes_example`).
    #[serde(default)]
    pub dialogue_examples: Vec<String>,
    #[serde(default)]
    pub personality: Vec<String>,
    #[serde(default)]
    pub appearance: Vec<String>,
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub likes: Vec<String>,
    #[serde(default)]
    pub dislikes: Vec<String>,
    #[serde(default)]
    pub secrets: Vec<String>,
    /// DTO form: a snake_case [`RelationshipStage`] string. Acceptable values
    /// are the same as `RelationshipStage` serde form, e.g. `"acquaintance"`.
    pub initial_player_relationship: String,
    #[serde(default)]
    pub speech_style: String,
    #[serde(default)]
    pub honorific_rule: String,
    #[serde(default)]
    pub frequent_phrases: Vec<String>,
    #[serde(default)]
    pub avoid_phrases: Vec<String>,
    #[serde(default)]
    pub known_facts: Vec<String>,
    #[serde(default)]
    pub unknown_facts: Vec<String>,
    #[serde(default)]
    pub current_goal: String,
    #[serde(default)]
    pub long_term_desire: String,
}

/// Top-level `characters.toml` DTO.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CharactersToml {
    pub schema_version: String,
    pub characters: Vec<CharacterToml>,
}

// ---------------------------------------------------------------------------
// rules.toml
// ---------------------------------------------------------------------------

/// `[limits]` section of `rules.toml`. All fields optional; missing values
/// fall back to the canonical defaults.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LimitsToml {
    #[serde(default)]
    pub affection_min: Option<i16>,
    #[serde(default)]
    pub affection_max: Option<i16>,
    #[serde(default)]
    pub trust_min: Option<i16>,
    #[serde(default)]
    pub trust_max: Option<i16>,
    #[serde(default)]
    pub tension_min: Option<i16>,
    #[serde(default)]
    pub tension_max: Option<i16>,
    #[serde(default)]
    pub normal_delta_min: Option<i16>,
    #[serde(default)]
    pub normal_delta_max: Option<i16>,
}

/// One `[[locations]]` entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocationToml {
    pub id: String,
    pub name: String,
    /// Adjacent location ids.
    #[serde(default)]
    pub connects_to: Vec<String>,
}

/// One `[[flags]]` entry. The canonical StoryRules only carries the
/// allow-list of flag ids; `default`/`description` are DTO-only.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlagToml {
    pub id: String,
    #[serde(default)]
    pub default: Option<bool>,
    #[serde(default)]
    pub description: Option<String>,
}

/// One `[[numbers]]` entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NumberVarToml {
    pub id: String,
    #[serde(default)]
    pub default: Option<i64>,
    #[serde(default)]
    pub min: Option<i64>,
    #[serde(default)]
    pub max: Option<i64>,
}

/// One `[[strings]]` entry (optional in MVP).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StringVarToml {
    pub id: String,
    #[serde(default)]
    pub max_len: Option<u32>,
    #[serde(default)]
    pub allowed: Option<Vec<String>>,
}

/// One `[[items]]` entry — only the id is needed for the canonical allow-list.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ItemToml {
    pub id: String,
}

/// One `[[routes]]` entry. `enter_condition` is a TOML-encoded
/// [`ConditionExpr`] so the pack author writes the engine's DSL directly.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteToml {
    pub id: String,
    #[serde(default)]
    pub premise: String,
    pub enter_condition: ConditionExpr,
}

/// One `[[endings]]` entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EndingToml {
    pub id: String,
    pub label: String,
    pub condition: ConditionExpr,
}

/// Top-level `rules.toml` DTO.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RulesToml {
    #[serde(default)]
    pub schema_version: Option<String>,
    #[serde(default)]
    pub limits: LimitsToml,
    #[serde(default)]
    pub locations: Vec<LocationToml>,
    #[serde(default)]
    pub flags: Vec<FlagToml>,
    #[serde(default)]
    pub numbers: Vec<NumberVarToml>,
    #[serde(default)]
    pub strings: Vec<StringVarToml>,
    #[serde(default)]
    pub items: Vec<ItemToml>,
    #[serde(default)]
    pub routes: Vec<RouteToml>,
    #[serde(default)]
    pub endings: Vec<EndingToml>,
}

// ---------------------------------------------------------------------------
// events.toml
// ---------------------------------------------------------------------------

/// Raw `[events.effects]` DTO. Maps directly onto
/// [`story_model::EventEffects`]; the converter collects its fields into
/// the canonical shape. Extra keys (e.g. `summary`, `record_memory`) are
/// tolerated and dropped.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EventEffectsToml {
    #[serde(default)]
    pub affection_delta: BTreeMap<String, i16>,
    #[serde(default)]
    pub trust_delta: BTreeMap<String, i16>,
    #[serde(default)]
    pub tension_delta: BTreeMap<String, i16>,
    #[serde(default)]
    pub set_flags: BTreeMap<String, bool>,
    #[serde(default)]
    pub number_delta: BTreeMap<String, i64>,
    #[serde(default)]
    pub add_events: Vec<String>,
    #[serde(default)]
    pub force_location: Option<String>,
    #[serde(default)]
    pub force_relationship: BTreeMap<String, String>,
    #[serde(default)]
    pub ending: Option<String>,
}

/// One `[[events]]` entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventToml {
    pub id: String,
    pub trigger: ConditionExpr,
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub effects: EventEffectsToml,
    #[serde(default)]
    pub allow_relationship_skip: bool,
    #[serde(default)]
    pub route_lock: Option<String>,
}

/// Top-level `events.toml` DTO. May be empty (no events).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EventsToml {
    #[serde(default)]
    pub schema_version: Option<String>,
    #[serde(default)]
    pub events: Vec<EventToml>,
}

// ===========================================================================
// Converters — DTO -> canonical
// ===========================================================================

/// Parse a snake_case rating string into a [`PackRating`].
pub fn parse_rating(s: &str) -> Result<PackRating, String> {
    match s {
        "general" => Ok(PackRating::General),
        "teen" => Ok(PackRating::Teen),
        other => Err(format!("알 수 없는 rating 값: {other:?}")),
    }
}

/// Parse a snake_case time-slot string into a [`TimeSlot`].
pub fn parse_time_slot(s: &str) -> Result<TimeSlot, String> {
    match s {
        "dawn" => Ok(TimeSlot::Dawn),
        "morning" => Ok(TimeSlot::Morning),
        "afternoon" => Ok(TimeSlot::Afternoon),
        "evening" => Ok(TimeSlot::Evening),
        "night" => Ok(TimeSlot::Night),
        other => Err(format!("알 수 없는 time_slot 값: {other:?}")),
    }
}

/// Parse a snake_case relationship-stage string into a [`RelationshipStage`].
pub fn parse_relationship_stage(s: &str) -> Result<RelationshipStage, String> {
    match s {
        "stranger" => Ok(RelationshipStage::Stranger),
        "acquaintance" => Ok(RelationshipStage::Acquaintance),
        "friend" => Ok(RelationshipStage::Friend),
        "close_friend" => Ok(RelationshipStage::CloseFriend),
        "romantic_interest" => Ok(RelationshipStage::RomanticInterest),
        "partner" => Ok(RelationshipStage::Partner),
        "estranged" => Ok(RelationshipStage::Estranged),
        other => Err(format!("알 수 없는 relationship 값: {other:?}")),
    }
}

/// Convert `story.toml` DTO into canonical [`PackMetadata`]. The schema
/// versions are parsed here; parsing failures bubble up as `Err(detail)`.
pub fn story_to_metadata(dto: &StoryToml) -> Result<PackMetadata, String> {
    let schema_version = semver::Version::parse(&dto.schema_version)
        .map_err(|e| format!("schema_version 파싱 실패: {e}"))?;
    let story_version = semver::Version::parse(&dto.story_version)
        .map_err(|e| format!("story_version 파싱 실패: {e}"))?;
    let min_engine_version = semver::Version::parse(&dto.min_engine_version)
        .map_err(|e| format!("min_engine_version 파싱 실패: {e}"))?;
    let rating = parse_rating(&dto.rating)?;
    let time_slot = parse_time_slot(&dto.initial_state.time_slot)?;
    if !(MIN_CHOICE_COUNT..=MAX_CHOICE_COUNT).contains(&dto.default_choice_count) {
        return Err(format!(
            "default_choice_count 범위 오류: {} (허용 {}..={})",
            dto.default_choice_count, MIN_CHOICE_COUNT, MAX_CHOICE_COUNT
        ));
    }
    Ok(PackMetadata {
        schema_version,
        story_id: dto.story_id.clone(),
        story_version,
        min_engine_version,
        title: dto.title.clone(),
        language: dto.language.clone(),
        genres: dto.genres.clone(),
        rating,
        default_choice_count: dto.default_choice_count,
        initial_state: PackInitialState {
            chapter: dto.initial_state.chapter.clone(),
            scene: dto.initial_state.scene.clone(),
            location: LocationId(dto.initial_state.location.clone()),
            time_slot,
        },
        content: PackContentPolicy {
            violence: dto.content.violence.clone().unwrap_or_default(),
        },
    })
}

/// Convert one `characters.toml` entry into a canonical
/// [`story_model::CharacterDef`]. Returns `Err(detail)` on bad enum values.
pub fn character_dto_to_canonical(
    dto: &CharacterToml,
) -> Result<story_model::CharacterDef, String> {
    if dto.id.trim().is_empty() {
        return Err("character id는 비어 있을 수 없습니다".to_string());
    }
    Ok(story_model::CharacterDef {
        id: CharacterId(dto.id.clone()),
        name: dto.name.clone(),
        age: dto.age,
        pronouns: dto.pronouns.clone(),
        description_md: dto.description_md.clone(),
        dialogue_examples: dto.dialogue_examples.clone(),
        personality: dto.personality.clone(),
        appearance: dto.appearance.clone(),
        values: dto.values.clone(),
        likes: dto.likes.clone(),
        dislikes: dto.dislikes.clone(),
        secrets: dto.secrets.clone(),
        initial_player_relationship: parse_relationship_stage(&dto.initial_player_relationship)?,
        speech_style: dto.speech_style.clone(),
        honorific_rule: dto.honorific_rule.clone(),
        frequent_phrases: dto.frequent_phrases.clone(),
        avoid_phrases: dto.avoid_phrases.clone(),
        known_facts: dto.known_facts.clone(),
        unknown_facts: dto.unknown_facts.clone(),
        current_goal: dto.current_goal.clone(),
        long_term_desire: dto.long_term_desire.clone(),
    })
}

/// Convert the `rules.toml` DTO into a canonical [`story_model::StoryRules`].
/// Pure data shaping — does not check references (validate.rs does that).
pub fn rules_dto_to_canonical(dto: &RulesToml) -> Result<story_model::StoryRules, String> {
    use story_model::{
        DEFAULT_DELTA_MAX, DEFAULT_DELTA_MIN, DEFAULT_STAT_MAX, DEFAULT_STAT_MIN, DeltaLimits,
        Int16Range, LocationDef, NumberVarDef, RouteDef, StoryRules, StringVarDef,
    };

    let mut rules = StoryRules::minimal();

    validate_unique_ids(
        "location",
        dto.locations.iter().map(|item| item.id.as_str()),
    )?;
    validate_unique_ids("flag", dto.flags.iter().map(|item| item.id.as_str()))?;
    validate_unique_ids("number", dto.numbers.iter().map(|item| item.id.as_str()))?;
    validate_unique_ids("string", dto.strings.iter().map(|item| item.id.as_str()))?;
    validate_unique_ids("item", dto.items.iter().map(|item| item.id.as_str()))?;
    validate_unique_ids("route", dto.routes.iter().map(|item| item.id.as_str()))?;
    validate_unique_ids("ending", dto.endings.iter().map(|item| item.id.as_str()))?;

    // Delta limits
    let normal_min = dto.limits.normal_delta_min.unwrap_or(DEFAULT_DELTA_MIN);
    let normal_max = dto.limits.normal_delta_max.unwrap_or(DEFAULT_DELTA_MAX);
    validate_i16_range("limits.normal_delta", normal_min, normal_max)?;
    validate_i16_range(
        "limits.affection",
        dto.limits.affection_min.unwrap_or(DEFAULT_STAT_MIN),
        dto.limits.affection_max.unwrap_or(DEFAULT_STAT_MAX),
    )?;
    validate_i16_range(
        "limits.trust",
        dto.limits.trust_min.unwrap_or(DEFAULT_STAT_MIN),
        dto.limits.trust_max.unwrap_or(DEFAULT_STAT_MAX),
    )?;
    validate_i16_range(
        "limits.tension",
        dto.limits.tension_min.unwrap_or(DEFAULT_STAT_MIN),
        dto.limits.tension_max.unwrap_or(DEFAULT_STAT_MAX),
    )?;
    rules.delta_limits = DeltaLimits {
        affection: Int16Range {
            min: normal_min,
            max: normal_max,
        },
        trust: Int16Range {
            min: normal_min,
            max: normal_max,
        },
        tension: Int16Range {
            min: normal_min,
            max: normal_max,
        },
    };

    // Keep the broad legacy bound for old consumers, and retain the actual
    // per-axis Pack limits for authoritative validation.
    let stat_min = dto
        .limits
        .affection_min
        .iter()
        .chain(dto.limits.trust_min.iter())
        .chain(dto.limits.tension_min.iter())
        .copied()
        .min()
        .unwrap_or(DEFAULT_STAT_MIN);
    let stat_max = dto
        .limits
        .affection_max
        .iter()
        .chain(dto.limits.trust_max.iter())
        .chain(dto.limits.tension_max.iter())
        .copied()
        .max()
        .unwrap_or(DEFAULT_STAT_MAX);
    rules.stat_limits = Int16Range {
        min: stat_min,
        max: stat_max,
    };
    rules.stat_limits_by_axis = Some(DeltaLimits {
        affection: Int16Range {
            min: dto.limits.affection_min.unwrap_or(DEFAULT_STAT_MIN),
            max: dto.limits.affection_max.unwrap_or(DEFAULT_STAT_MAX),
        },
        trust: Int16Range {
            min: dto.limits.trust_min.unwrap_or(DEFAULT_STAT_MIN),
            max: dto.limits.trust_max.unwrap_or(DEFAULT_STAT_MAX),
        },
        tension: Int16Range {
            min: dto.limits.tension_min.unwrap_or(DEFAULT_STAT_MIN),
            max: dto.limits.tension_max.unwrap_or(DEFAULT_STAT_MAX),
        },
    });

    // Locations + adjacency
    for loc in &dto.locations {
        let id = LocationId(loc.id.clone());
        let mut adjacent = std::collections::BTreeSet::new();
        for c in &loc.connects_to {
            adjacent.insert(LocationId(c.clone()));
        }
        rules.locations.insert(
            id.clone(),
            LocationDef {
                id,
                display_name: loc.name.clone(),
                adjacent,
            },
        );
    }

    // Flags allow-list
    for f in &dto.flags {
        rules.allowed_flags.insert(f.id.clone());
    }

    // Numbers
    for n in &dto.numbers {
        let min = n.min.unwrap_or(i64::MIN);
        let max = n.max.unwrap_or(i64::MAX);
        let default = n.default.unwrap_or(0);
        if min > max {
            return Err(format!("number {} 범위 오류: min {min} > max {max}", n.id));
        }
        if !(min..=max).contains(&default) {
            return Err(format!(
                "number {} default {default}가 범위 {min}..={max} 밖입니다",
                n.id
            ));
        }
        rules.numbers.insert(
            n.id.clone(),
            NumberVarDef {
                id: n.id.clone(),
                min,
                max,
                default,
            },
        );
    }

    // Strings
    for s in &dto.strings {
        let allowed = s
            .allowed
            .as_ref()
            .map(|v| v.iter().cloned().collect::<std::collections::BTreeSet<_>>());
        rules.strings.insert(
            s.id.clone(),
            StringVarDef {
                id: s.id.clone(),
                max_len: s.max_len.unwrap_or(u32::MAX),
                allowed,
            },
        );
    }

    // Items allow-list
    for it in &dto.items {
        rules.allowed_items.insert(it.id.clone());
    }

    // Routes
    for r in &dto.routes {
        rules.routes.insert(
            r.id.clone(),
            RouteDef {
                id: r.id.clone(),
                enter_condition: r.enter_condition.clone(),
                premise: r.premise.clone(),
            },
        );
    }

    // Endings
    for e in &dto.endings {
        rules.endings.insert(
            e.id.clone(),
            story_model::EndingDef {
                id: e.id.clone(),
                condition: e.condition.clone(),
                label: e.label.clone(),
            },
        );
    }

    Ok(rules)
}

/// Convert the `events.toml` DTO into canonical
/// `BTreeMap<EventId, EventDef>` entries and merge them into `rules.events`.
pub fn merge_events_into_rules(
    dto: &EventsToml,
    rules: &mut story_model::StoryRules,
) -> Result<(), String> {
    use story_model::{EventDef, EventEffects};
    validate_unique_ids("event", dto.events.iter().map(|item| item.id.as_str()))?;
    for ev in &dto.events {
        let id = EventId(ev.id.clone());

        // Translate DTO effects into canonical EventEffects.
        let mut effects = EventEffects::default();
        for (cid, v) in &ev.effects.affection_delta {
            effects.affection_delta.insert(CharacterId(cid.clone()), *v);
        }
        for (cid, v) in &ev.effects.trust_delta {
            effects.trust_delta.insert(CharacterId(cid.clone()), *v);
        }
        for (cid, v) in &ev.effects.tension_delta {
            effects.tension_delta.insert(CharacterId(cid.clone()), *v);
        }
        for (fid, v) in &ev.effects.set_flags {
            effects.set_flags.insert(fid.clone(), *v);
        }
        for (nid, v) in &ev.effects.number_delta {
            effects.number_delta.insert(nid.clone(), *v);
        }
        for ev_id in &ev.effects.add_events {
            effects.add_events.insert(ev_id.clone());
        }
        if let Some(loc) = &ev.effects.force_location {
            effects.force_location = Some(LocationId(loc.clone()));
        }
        for (cid, stage) in &ev.effects.force_relationship {
            let stage = parse_relationship_stage(stage)?;
            effects
                .force_relationship
                .insert(CharacterId(cid.clone()), stage);
        }
        effects.ending = ev.effects.ending.clone();

        let def = EventDef {
            id: id.clone(),
            trigger: ev.trigger.clone(),
            priority: ev.priority,
            effects,
            allow_relationship_skip: ev.allow_relationship_skip,
            route_lock: ev.route_lock.clone(),
        };
        rules.events.insert(id, def);
    }
    Ok(())
}

fn validate_unique_ids<'a>(kind: &str, ids: impl Iterator<Item = &'a str>) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for id in ids {
        if id.trim().is_empty() {
            return Err(format!("{kind} id는 비어 있을 수 없습니다"));
        }
        if !seen.insert(id) {
            return Err(format!("중복 {kind} id: {id}"));
        }
    }
    Ok(())
}

fn validate_i16_range(label: &str, min: i16, max: i16) -> Result<(), String> {
    if min > max {
        return Err(format!("{label} 범위 오류: min {min} > max {max}"));
    }
    Ok(())
}

/// Parse a snake_case compare-operator string. Used by tests that build
/// ConditionExpr via string tables.
#[allow(dead_code)]
pub fn parse_compare_op(s: &str) -> Result<CompareOp, String> {
    match s {
        "eq" => Ok(CompareOp::Eq),
        "ne" => Ok(CompareOp::Ne),
        "lt" => Ok(CompareOp::Lt),
        "le" => Ok(CompareOp::Le),
        "gt" => Ok(CompareOp::Gt),
        "ge" => Ok(CompareOp::Ge),
        other => Err(format!("알 수 없는 CompareOp 값: {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_story_toml() -> StoryToml {
        toml::from_str(
            r#"
schema_version = "1.0.0"
story_id = "rainy-reunion"
story_version = "0.1.0"
min_engine_version = "0.1.0"
title = "비가 그친 뒤"
language = "ko-KR"
genres = ["modern-romance"]
rating = "teen"
default_choice_count = 3

[initial_state]
chapter = "chapter-1"
scene = "cafe-reunion"
location = "cafe-namu"
time_slot = "evening"

[content]
violence = "mild"
"#,
        )
        .unwrap()
    }

    #[test]
    fn story_toml_round_trip_and_convert() {
        let dto = sample_story_toml();
        let md = story_to_metadata(&dto).expect("metadata");
        assert_eq!(md.story_id, "rainy-reunion");
        assert_eq!(md.rating, PackRating::Teen);
        assert_eq!(md.initial_state.location, LocationId("cafe-namu".into()));
        assert_eq!(md.initial_state.time_slot, TimeSlot::Evening);
        assert_eq!(md.content.violence, "mild");
    }

    #[test]
    fn bad_rating_is_error() {
        let mut dto = sample_story_toml();
        dto.rating = "restricted".to_string();
        let err = story_to_metadata(&dto).expect_err("error");
        assert!(err.contains("rating"));
    }

    #[test]
    fn characters_toml_parses_and_converts() {
        let toml_str = r#"
schema_version = "1.0.0"

[[characters]]
id = "yuna"
name = "한유나"
age = 29
pronouns = "그녀"
initial_player_relationship = "acquaintance"
speech_style = "짧고 정확한 문장."
honorific_rule = "해요체"
current_goal = "재회 확인"
long_term_desire = "사라지지 않는 관계"
"#;
        let dto: CharactersToml = toml::from_str(toml_str).expect("parse");
        let c = character_dto_to_canonical(&dto.characters[0]).expect("convert");
        assert_eq!(c.id.0, "yuna");
        assert_eq!(c.age, Some(29));
        assert_eq!(
            c.initial_player_relationship,
            RelationshipStage::Acquaintance
        );
    }

    #[test]
    fn rules_dto_converts_locations_and_limits() {
        let toml_str = r#"
[limits]
normal_delta_min = -3
normal_delta_max = 3
affection_min = -30
affection_max = 30

[[locations]]
id = "cafe-namu"
name = "카페 나무"
connects_to = ["riverside-walk"]

[[locations]]
id = "riverside-walk"
name = "강변 산책로"
connects_to = ["cafe-namu"]

[[flags]]
id = "apologized"

[[numbers]]
id = "yuna.affection"
default = 0
min = -30
max = 30
"#;
        let dto: RulesToml = toml::from_str(toml_str).expect("parse");
        let rules = rules_dto_to_canonical(&dto).expect("convert");
        assert_eq!(rules.locations.len(), 2);
        assert!(rules.allowed_flags.contains("apologized"));
        assert_eq!(rules.delta_limits.affection.min, -3);
        assert_eq!(rules.stat_limits.max, 30);
        assert_eq!(rules.affection_stat_limits().min, -30);
        assert_eq!(rules.affection_stat_limits().max, 30);
        assert_eq!(
            rules.tension_stat_limits().min,
            story_model::DEFAULT_STAT_MIN
        );
        assert!(rules.numbers.contains_key("yuna.affection"));
    }

    #[test]
    fn events_dto_merges_into_rules() {
        let toml_str = r#"
[[events]]
id = "event.first_honest_exchange"
priority = 10
trigger = { kind = "all", children = [
  { kind = "flag_equals", id = "apologized", value = true },
  { kind = "trust_at_least", character = "yuna", value = 2 },
]}

[events.effects]
set_flags = { "mentioned_returned_letters" = true }
add_events = ["event.x"]
"#;
        let dto: EventsToml = toml::from_str(toml_str).expect("parse");
        let mut rules = story_model::StoryRules::minimal();
        merge_events_into_rules(&dto, &mut rules).expect("merge");
        let ev = rules
            .events
            .get(&EventId("event.first_honest_exchange".into()))
            .expect("event present");
        assert_eq!(ev.priority, 10);
        assert!(matches!(ev.trigger, ConditionExpr::All { .. }));
        assert!(
            ev.effects
                .set_flags
                .contains_key("mentioned_returned_letters")
        );
        assert!(ev.effects.add_events.contains("event.x"));
    }

    #[test]
    fn route_ending_dto_use_condition_expr() {
        let toml_str = r#"
[[routes]]
id = "reconciliation"
premise = "재회"
enter_condition = { kind = "all", children = [
  { kind = "flag_equals", id = "apologized", value = true },
]}

[[endings]]
id = "ending-new-beginning"
label = "새로운 시작"
condition = { kind = "event_occurred", id = "event.reconciled" }
"#;
        let dto: RulesToml = toml::from_str(toml_str).expect("parse");
        let rules = rules_dto_to_canonical(&dto).expect("convert");
        assert_eq!(rules.routes.len(), 1);
        assert_eq!(rules.endings.len(), 1);
        assert!(matches!(
            rules.endings["ending-new-beginning"].condition,
            ConditionExpr::EventOccurred { .. }
        ));
    }

    #[test]
    fn unknown_keys_are_tolerated() {
        let toml_str = r#"
schema_version = "1.0.0"
story_id = "rainy-reunion"
story_version = "0.1.0"
min_engine_version = "0.1.0"
title = "비가 그친 뒤"
language = "ko-KR"
rating = "general"
author = "StoryTUI Example"   # not modeled

[content]
romance = true               # extra key

[initial_state]
chapter = "c1"
scene = "s1"
location = "loc1"
time_slot = "evening"

[generation]
narration_min_chars = 450    # extra section
"#;
        let dto: StoryToml = toml::from_str(toml_str).expect("must tolerate unknown");
        assert_eq!(dto.story_id, "rainy-reunion");
    }
}
