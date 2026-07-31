//! Test-support fixtures shared across the `story-core` integration tests.
//!
//! Pure data builders only — no IO, no mocking. Exposed as a `pub` module so
//! the W8 save/load tests can drive the same pack / draft shapes the W5/W7
//! reduce tests use, without duplicating them.

use std::collections::BTreeMap;
use std::path::PathBuf;

use story_model::LocationDef;
use story_model::StatePatch;
use story_model::{
    CharacterDef, Choice, GeneratedTurnDraft, LocationId, PackContentPolicy, PackInitialState,
    PackMetadata, PackRating, PromptFragments, RelationshipStage, RiskLevel, StoryPack, StoryRules,
    TimeSlot, TurnSummary,
};
use story_model::{CharacterId, semver, uuid};

/// The "demo" pack used across reduce tests.
pub fn sample_pack() -> StoryPack {
    let metadata = PackMetadata {
        schema_version: semver::Version::parse("1.0.0").unwrap(),
        story_id: "demo".to_string(),
        story_version: semver::Version::new(0, 1, 0),
        min_engine_version: semver::Version::new(0, 1, 0),
        title: "데모 이야기".to_string(),
        language: "ko-KR".to_string(),
        genres: vec!["drama".to_string()],
        rating: PackRating::General,
        all_major_characters_safe: true,
        default_choice_count: 3,
        initial_state: PackInitialState {
            chapter: "chapter-1".to_string(),
            scene: "scene-1".to_string(),
            location: LocationId("loc-classroom".to_string()),
            time_slot: TimeSlot::Morning,
        },
        content: PackContentPolicy {
            safe_content: "none".to_string(),
            violence: "none".to_string(),
        },
    };

    let character = CharacterDef {
        id: cid(),
        name: "나무".to_string(),
        age: Some(28),
        pronouns: None,
        personality: vec![],
        appearance: vec![],
        values: vec![],
        likes: vec![],
        dislikes: vec![],
        secrets: vec![],
        initial_player_relationship: RelationshipStage::Acquaintance,
        speech_style: String::new(),
        honorific_rule: String::new(),
        frequent_phrases: vec![],
        avoid_phrases: vec![],
        known_facts: vec![],
        unknown_facts: vec![],
        current_goal: String::new(),
        long_term_desire: String::new(),
    };

    let mut characters = BTreeMap::new();
    characters.insert(cid(), character);

    let mut rules = StoryRules::minimal();
    rules.locations.insert(
        LocationId("loc-classroom".to_string()),
        LocationDef {
            id: LocationId("loc-classroom".to_string()),
            display_name: "교실".to_string(),
            adjacent: Default::default(),
        },
    );
    rules
        .allowed_flags
        .extend(["f-brave", "f-met"].map(String::from));

    StoryPack {
        metadata,
        characters,
        rules,
        world_md: String::new(),
        style_md: String::new(),
        opening_md: "오프닝: 비가 그쳤다.".to_string(),
        prompts: PromptFragments {
            system: "sys".to_string(),
            director: "d".to_string(),
            writer: "w".to_string(),
        },
        pack_root: PathBuf::from("/tmp/demo"),
    }
}

/// A draft whose patch applies cleanly against [`sample_pack`]'s rules
/// (`affection +N` for the known character).
pub fn valid_draft(generation_id: uuid::Uuid, affection_delta: i16) -> GeneratedTurnDraft {
    let mut patch = StatePatch::default();
    patch.affection_delta.insert(cid(), affection_delta);
    GeneratedTurnDraft {
        generation_id,
        narration: format!("유나가 고개를 끄덕인다 (affection {affection_delta:+})."),
        choices: vec![
            choice("choice-0", "다시 만난다", RiskLevel::Low),
            choice("choice-1", "돌아선다", RiskLevel::Medium),
            choice("choice-2", "기다린다", RiskLevel::Safe),
        ],
        proposed_patch: patch,
        summary: TurnSummary {
            facts: vec![],
            unresolved: vec![],
            relationship_changes: vec![],
            knowledge_changes: vec![],
            interpretation: None,
            address_form_changes: vec![],
            important_player_actions: vec![],
            source_history_ids: vec![],
        },
        usage: None,
    }
}

fn choice(id: &str, label: &str, risk: RiskLevel) -> Choice {
    Choice {
        id: id.to_string(),
        label: label.to_string(),
        intent: String::new(),
        tone: None,
        target: None,
        risk,
        hidden_requirements: vec![],
        preview: None,
    }
}

fn cid() -> CharacterId {
    CharacterId("c-namu".to_string())
}
