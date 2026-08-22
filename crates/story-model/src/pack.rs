//! Canonical Story Pack types (W4, ST-011).
//!
//! These are the pure-data result of loading a Story Pack from disk: the
//! loader in `story-persistence` parses TOML/Markdown and produces a
//! [`StoryPack`], which the engine then consumes. Everything here is plain
//! data with a stable serde shape — no behavior that touches the terminal,
//! network, or filesystem (ADR-003).
//!
//! # Serde / version policy
//!
//! - Struct fields serialize as snake_case (serde default).
//! - Enums use `#[serde(rename_all = "snake_case")]`.
//! - `semver::Version` round-trips as a string (e.g. `"1.0.0"`).
//!
//! These types intentionally do NOT carry `deny_unknown_fields`: pack
//! authors may include exploratory keys the engine does not yet read.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{CharacterId, LocationId, RelationshipStage, StoryRules, TimeSlot};

/// Pack schema version this build of `story-model` writes and understands.
///
/// Bumped on every incompatible change to the on-disk Pack layout. The W4
/// loader compares the major component of a Pack's `schema_version` against
/// this; same-major is loaded (with in-memory default migration for missing
/// optional fields), cross-major is rejected unless an explicit migrator
/// exists (W8+, not in W4 scope).
pub const PACK_SCHEMA_VERSION: &str = "1.0.0";

/// Coarse audience rating stamped onto a Pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackRating {
    /// All-ages.
    General,
    /// Teen and up.
    Teen,
}

/// Initial scene/location the engine drops the player into.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackInitialState {
    /// Starting chapter id.
    pub chapter: String,
    /// Starting scene id within `chapter`.
    pub scene: String,
    /// Starting location id; must exist in `rules.locations`.
    pub location: LocationId,
    /// Starting time-of-day bucket.
    pub time_slot: TimeSlot,
}

/// Structured content policy for supported story themes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackContentPolicy {
    /// Declared violence policy string (e.g. `"mild"`).
    pub violence: String,
}

/// Top-level Pack metadata, sourced from `story.toml`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackMetadata {
    /// On-disk Pack schema version (controls loader compat).
    pub schema_version: semver::Version,
    /// Author-facing story identifier (e.g. `"rainy-reunion"`).
    pub story_id: String,
    /// Author-facing story version, independent of the Pack schema version.
    pub story_version: semver::Version,
    /// Minimum engine version this Pack requires to play.
    pub min_engine_version: semver::Version,
    /// Human-readable title (Korean allowed).
    pub title: String,
    /// BCP-47 language tag (e.g. `"ko-KR"`).
    pub language: String,
    /// Genre tags.
    pub genres: Vec<String>,
    /// Content rating.
    pub rating: PackRating,
    /// Default number of choices offered per turn (MVP = 3).
    pub default_choice_count: u8,
    /// Starting scene/location.
    pub initial_state: PackInitialState,
    /// Declared content policy.
    pub content: PackContentPolicy,
}

/// An invariant character definition loaded from `characters.toml`.
///
/// The per-character mutable state (affection, trust, emotion, ...) lives
/// in [`crate::CharacterState`]; this struct holds only the Pack-time
/// constants: identity, voice, knowledge, goals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterDef {
    /// Identifier matching `rules.toml`/event references.
    pub id: CharacterId,
    /// Display name.
    pub name: String,
    /// Stated age. `None` means "unspecified".
    pub age: Option<u32>,
    /// Optional pronouns / address-form hint string.
    pub pronouns: Option<String>,
    /// Personality bullet lines.
    pub personality: Vec<String>,
    /// Appearance bullet lines.
    pub appearance: Vec<String>,
    /// Values bullet lines.
    pub values: Vec<String>,
    /// Likes bullet lines.
    pub likes: Vec<String>,
    /// Dislikes bullet lines.
    pub dislikes: Vec<String>,
    /// Secret bullet lines. Director gates their reveal.
    pub secrets: Vec<String>,
    /// Initial relationship stage from the player's perspective.
    pub initial_player_relationship: RelationshipStage,
    /// Voice/speech style guideline (Korean prose).
    pub speech_style: String,
    /// Honorific/formality rule (Korean prose).
    pub honorific_rule: String,
    /// Stock phrases this character tends to use.
    pub frequent_phrases: Vec<String>,
    /// Phrases this character avoids (anti-style).
    pub avoid_phrases: Vec<String>,
    /// Facts this character knows.
    pub known_facts: Vec<String>,
    /// Facts this character does NOT know (used by Director must-not-reveal).
    pub unknown_facts: Vec<String>,
    /// Current concrete goal.
    pub current_goal: String,
    /// Long-term desire that motivates the character.
    pub long_term_desire: String,
}

/// The three prompt fragments a Pack ships, concatenated into the system /
/// director / writer roles by the prompt builder (W6+).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptFragments {
    /// System prompt: absolute role, safety, output contract.
    pub system: String,
    /// Director prompt: planning, conflict checks, patch proposal.
    pub director: String,
    /// Writer prompt: realizing the plan as Korean scene prose.
    pub writer: String,
}

/// The loaded, canonical Story Pack — the loader's output and the engine's
/// input for a session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoryPack {
    /// Top-level metadata from `story.toml`.
    pub metadata: PackMetadata,
    /// Major character definitions, keyed by id.
    pub characters: BTreeMap<CharacterId, CharacterDef>,
    /// Canonical rule set from `rules.toml` + `events.toml`.
    pub rules: StoryRules,
    /// `world.md` body.
    pub world_md: String,
    /// `style.md` body.
    pub style_md: String,
    /// `opening.md` body.
    pub opening_md: String,
    /// `prompts/{system,director,writer}.md` bodies.
    pub prompts: PromptFragments,
    /// Canonical absolute root directory of the Pack on disk. Carried for
    /// diagnostics only — never used by the deterministic engine.
    pub pack_root: PathBuf,
}

impl StoryPack {
    /// Convenience accessor: `default_choice_count` from metadata.
    pub fn default_choice_count(&self) -> u8 {
        self.metadata.default_choice_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_pack_metadata() -> PackMetadata {
        PackMetadata {
            schema_version: semver::Version::parse(PACK_SCHEMA_VERSION).unwrap(),
            story_id: "rainy-reunion".to_string(),
            story_version: semver::Version::new(0, 1, 0),
            min_engine_version: semver::Version::new(0, 1, 0),
            title: "비가 그친 뒤".to_string(),
            language: "ko-KR".to_string(),
            genres: vec!["modern-romance".to_string(), "drama".to_string()],
            rating: PackRating::Teen,
            default_choice_count: 3,
            initial_state: PackInitialState {
                chapter: "chapter-1".to_string(),
                scene: "cafe-reunion".to_string(),
                location: LocationId("cafe-namu".to_string()),
                time_slot: TimeSlot::Evening,
            },
            content: PackContentPolicy {
                violence: "mild".to_string(),
            },
        }
    }

    fn sample_character() -> CharacterDef {
        CharacterDef {
            id: CharacterId("yuna".to_string()),
            name: "한유나".to_string(),
            age: Some(29),
            pronouns: Some("그녀".to_string()),
            personality: vec!["관찰력이 좋다".to_string()],
            appearance: vec!["짧은 검은 머리".to_string()],
            values: vec!["솔직함".to_string()],
            likes: vec!["오래된 재즈".to_string()],
            dislikes: vec!["감정의 강요".to_string()],
            secrets: vec!["3년 전 떠난 이유".to_string()],
            initial_player_relationship: RelationshipStage::Acquaintance,
            speech_style: "짧고 정확한 문장.".to_string(),
            honorific_rule: "초기 해요체.".to_string(),
            frequent_phrases: vec!["일단 앉아요.".to_string()],
            avoid_phrases: vec!["운명".to_string()],
            known_facts: vec!["player가 늦었다".to_string()],
            unknown_facts: vec!["player가 편지를 보냈다".to_string()],
            current_goal: "재회의 이유 확인".to_string(),
            long_term_desire: "사라지지 않는 관계".to_string(),
        }
    }

    #[test]
    fn pack_rating_serde_snake_case() {
        let json = serde_json::to_string(&PackRating::Teen).unwrap();
        assert_eq!(json, "\"teen\"");
        let g: PackRating = serde_json::from_str("\"general\"").unwrap();
        assert_eq!(g, PackRating::General);
        let t: PackRating = serde_json::from_str("\"teen\"").unwrap();
        assert_eq!(t, PackRating::Teen);
    }

    #[test]
    fn metadata_round_trip_serializes_semver_as_strings() {
        let md = sample_pack_metadata();
        let json = serde_json::to_string(&md).unwrap();
        assert!(
            json.contains("\"schema_version\":\"1.0.0\""),
            "schema_version must serialize as a string: {json}"
        );
        let back: PackMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(back, md);
    }

    #[test]
    fn character_def_round_trip() {
        let c = sample_character();
        let json = serde_json::to_string_pretty(&c).unwrap();
        let back: CharacterDef = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
        // transparent id (allow for the space pretty-json emits after `:`)
        assert!(
            json.contains("\"id\": \"yuna\"") || json.contains("\"id\":\"yuna\""),
            "expected transparent id serialization: {json}"
        );
    }

    #[test]
    fn story_pack_round_trip_keeps_markdown_bodies() {
        let pack = StoryPack {
            metadata: sample_pack_metadata(),
            characters: {
                let mut m = BTreeMap::new();
                m.insert(CharacterId("yuna".to_string()), sample_character());
                m
            },
            rules: StoryRules::minimal(),
            world_md: "# 세계관\n".to_string(),
            style_md: "# 문체\n".to_string(),
            opening_md: "# 오프닝\n".to_string(),
            prompts: PromptFragments {
                system: "system.".to_string(),
                director: "director.".to_string(),
                writer: "writer.".to_string(),
            },
            pack_root: PathBuf::from("/tmp/rainy-reunion"),
        };
        let json = serde_json::to_string(&pack).unwrap();
        let back: StoryPack = serde_json::from_str(&json).unwrap();
        assert_eq!(back.metadata, pack.metadata);
        assert_eq!(back.world_md, pack.world_md);
        assert_eq!(back.prompts.director, pack.prompts.director);
        assert_eq!(back.default_choice_count(), 3);
    }

    #[test]
    fn pack_schema_version_parses() {
        let v = semver::Version::parse(PACK_SCHEMA_VERSION).unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 0);
        assert_eq!(v.patch, 0);
    }
}
