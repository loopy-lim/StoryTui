//! Integration tests for ST-029: deterministic event-based memory creation.
//!
//! Exercises [`story_core::memory::memories_from_turn`] end-to-end by feeding
//! hand-built [`ApplyResult`]s (produced by the real reducer) and asserting the
//! derived [`MemoryEntry`]s have the correct kind, participants, `known_by`,
//! `unresolved` flag, and source attribution. Also pins determinism and the
//! resolver-clears-unresolved convention.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use story_core::memory::{memories_from_turn, resolve_memories};
use story_core::reducer::apply_patch;
use story_model::uuid::Uuid;
use story_model::{
    CharacterId, CharacterState, Emotion, EventDef, EventEffects, EventId, GameState, LocationId,
    MemoryKind, RelationshipStage, SAVE_SCHEMA_VERSION, StatePatch, StoryRules,
};

const PLAYER: &str = "player";
const YUNA: &str = "yuna";

fn yuna_id() -> CharacterId {
    CharacterId(YUNA.to_string())
}

fn base_state() -> GameState {
    let mut chars = BTreeMap::new();
    chars.insert(
        yuna_id(),
        CharacterState {
            affection: 0,
            trust: 0,
            tension: 0,
            relationship_stage: RelationshipStage::Acquaintance,
            current_emotion: Emotion::Neutral,
            known_facts: BTreeSet::new(),
            address_form_overrides: BTreeMap::new(),
        },
    );
    GameState {
        save_schema_version: SAVE_SCHEMA_VERSION,
        story_id: "demo".to_string(),
        story_version: semver::Version::new(0, 1, 0),
        pack_schema_version: semver::Version::new(1, 0, 0),
        session_id: Uuid::nil(),
        current_chapter: "ch-1".to_string(),
        current_scene_id: "sc-1".to_string(),
        turn: 0,
        location: LocationId("cafe-namu".to_string()),
        time_slot: story_model::TimeSlot::Evening,
        active_route: None,
        characters: chars,
        flags: BTreeMap::new(),
        numbers: BTreeMap::new(),
        strings: BTreeMap::new(),
        inventory: BTreeMap::new(),
        occurred_events: BTreeSet::new(),
        recent_summaries: VecDeque::new(),
        memories: Vec::new(),
        history: Vec::new(),
        last_choice: None,
        revision: 0,
        stable_generation_id: None,
    }
}

/// Build rules with one event per memory-kind whose id encodes the kind.
fn rules_with_events() -> StoryRules {
    let mut r = StoryRules::minimal();
    // A flag the patches can set so reference-check passes.
    r.allowed_flags
        .extend(["f-meet", "f-resolve"].map(String::from));

    let yuna = yuna_id();
    let mk = |id: &str, effects: EventEffects| EventDef {
        id: EventId(id.to_string()),
        // Empty conjunction -> always fires when add_events references it.
        trigger: story_model::ConditionExpr::All { children: vec![] },
        priority: 1,
        effects,
        allow_relationship_skip: false,
        route_lock: None,
    };
    r.events.insert(
        EventId("promise.share_umbrella".to_string()),
        mk(
            "promise.share_umbrella",
            EventEffects {
                affection_delta: BTreeMap::from([(yuna.clone(), 1)]),
                ..EventEffects::default()
            },
        ),
    );
    r.events.insert(
        EventId("secret.past_letter".to_string()),
        mk(
            "secret.past_letter",
            EventEffects {
                trust_delta: BTreeMap::from([(yuna.clone(), 1)]),
                ..EventEffects::default()
            },
        ),
    );
    r.events.insert(
        EventId("relationship.truce".to_string()),
        mk(
            "relationship.truce",
            EventEffects {
                force_relationship: BTreeMap::from([(yuna.clone(), RelationshipStage::Friend)]),
                ..EventEffects::default()
            },
        ),
    );
    r.events.insert(
        EventId("key_item.umbrella".to_string()),
        mk(
            "key_item.umbrella",
            EventEffects {
                tension_delta: BTreeMap::from([(yuna.clone(), 1)]),
                ..EventEffects::default()
            },
        ),
    );
    r.events.insert(
        EventId("first_meeting.yuna".to_string()),
        mk("first_meeting.yuna", EventEffects::default()),
    );
    r.events.insert(
        EventId("confession.love".to_string()),
        mk(
            "confession.love",
            EventEffects {
                affection_delta: BTreeMap::from([(yuna.clone(), 1)]),
                ..EventEffects::default()
            },
        ),
    );
    r.events.insert(
        EventId("betrayal.lie".to_string()),
        mk(
            "betrayal.lie",
            EventEffects {
                affection_delta: BTreeMap::from([(yuna.clone(), -1)]),
                ..EventEffects::default()
            },
        ),
    );
    // Resolver events.
    r.events.insert(
        EventId("resolve_promise.share_umbrella".to_string()),
        mk("resolve_promise.share_umbrella", EventEffects::default()),
    );
    r.events.insert(
        EventId("resolve_secret.past_letter".to_string()),
        mk("resolve_secret.past_letter", EventEffects::default()),
    );
    r
}

fn fire(
    state: &GameState,
    rules: &StoryRules,
    event_ids: &[&str],
) -> story_core::reducer::ApplyResult {
    let mut patch = StatePatch::default();
    for id in event_ids {
        patch.add_events.insert(id.to_string());
    }
    apply_patch(state, rules, &patch, Uuid::nil()).expect("patch applies")
}

#[test]
fn promise_event_produces_unresolved_promise_memory() {
    let state = base_state();
    let rules = rules_with_events();
    let result = fire(&state, &rules, &["promise.share_umbrella"]);
    assert_eq!(result.fired_events.len(), 1);

    let hid = Uuid::new_v4();
    let mems = memories_from_turn(&result, &dummy_pack(&rules), 3, hid);
    assert_eq!(mems.len(), 1, "exactly one memory for one fired event");
    let m = &mems[0];
    assert_eq!(m.kind, MemoryKind::Promise);
    assert!(m.unresolved, "promise starts unresolved");
    assert_eq!(m.importance, 9);
    assert_eq!(m.created_turn, 3);
    assert_eq!(m.source_history_ids, vec![hid]);
    assert_eq!(
        m.location.as_ref(),
        Some(&LocationId("cafe-namu".to_string()))
    );
    // participants include yuna (from the affection delta).
    assert!(m.participants.contains(&yuna_id()));
    // known_by: player + participants, no invented owners.
    assert!(
        m.facts[0]
            .known_by
            .contains(&CharacterId(PLAYER.to_string()))
    );
    assert!(m.facts[0].known_by.contains(&yuna_id()));
    assert_eq!(m.facts[0].predicate, "약속");
}

#[test]
fn secret_reveal_produces_unresolved_secret_memory() {
    let state = base_state();
    let rules = rules_with_events();
    let result = fire(&state, &rules, &["secret.past_letter"]);
    let mems = memories_from_turn(&result, &dummy_pack(&rules), 1, Uuid::new_v4());
    assert_eq!(mems.len(), 1);
    let m = &mems[0];
    assert_eq!(m.kind, MemoryKind::SecretReveal);
    assert!(m.unresolved);
    assert_eq!(m.facts[0].predicate, "비밀");
}

#[test]
fn relationship_change_is_not_unresolved_but_is_absolute() {
    let state = base_state();
    let rules = rules_with_events();
    let result = fire(&state, &rules, &["relationship.truce"]);
    let mems = memories_from_turn(&result, &dummy_pack(&rules), 1, Uuid::new_v4());
    assert_eq!(mems.len(), 1);
    let m = &mems[0];
    assert_eq!(m.kind, MemoryKind::RelationshipChange);
    assert!(
        !m.unresolved,
        "relationship-change memories are not unresolved threads"
    );
    assert_eq!(m.importance, 7);
}

#[test]
fn key_item_confession_betrayal_first_meeting_classify() {
    let state = base_state();
    let rules = rules_with_events();
    for (ev, kind) in [
        ("key_item.umbrella", MemoryKind::KeyItem),
        ("confession.love", MemoryKind::Confession),
        ("betrayal.lie", MemoryKind::Betrayal),
        ("first_meeting.yuna", MemoryKind::FirstMeeting),
    ] {
        let result = fire(&state, &rules, &[ev]);
        let mems = memories_from_turn(&result, &dummy_pack(&rules), 1, Uuid::new_v4());
        assert_eq!(mems.len(), 1, "{ev} should produce one memory");
        assert_eq!(mems[0].kind, kind, "{ev} -> {kind:?}");
        assert!(!mems[0].unresolved, "{ev} not unresolved");
    }
}

#[test]
fn unclassified_canonical_event_still_produces_other_memory() {
    let state = base_state();
    let mut rules = rules_with_events();
    // A canonical event whose id carries no legacy memory-kind hint.
    rules.events.insert(
        EventId("ambient.rain_starts".to_string()),
        EventDef {
            id: EventId("ambient.rain_starts".to_string()),
            trigger: story_model::ConditionExpr::All { children: vec![] },
            priority: 1,
            effects: EventEffects::default(),
            allow_relationship_skip: false,
            route_lock: None,
        },
    );
    let result = fire(&state, &rules, &["ambient.rain_starts"]);
    let mems = memories_from_turn(&result, &dummy_pack(&rules), 1, Uuid::new_v4());
    assert_eq!(mems.len(), 1);
    assert_eq!(mems[0].kind, MemoryKind::Other);
    assert_eq!(mems[0].facts[0].object, "event:ambient.rain_starts");
    assert!(!mems[0].unresolved);
}

#[test]
fn creation_is_deterministic_same_inputs_same_output() {
    let state = base_state();
    let rules = rules_with_events();
    let pack = dummy_pack(&rules);
    let result = fire(
        &state,
        &rules,
        &["promise.share_umbrella", "secret.past_letter"],
    );
    let hid = Uuid::new_v4();
    let a = memories_from_turn(&result, &pack, 5, hid);
    let b = memories_from_turn(&result, &pack, 5, hid);
    // MemoryEntry isn't PartialEq (story-model W2); compare via canonical JSON.
    let sa = serde_json::to_string(&a).unwrap();
    let sb = serde_json::to_string(&b).unwrap();
    assert_eq!(sa, sb, "identical inputs => identical output (determinism)");
    // Order is stable regardless of fire-order permutation in the result.
    assert_eq!(a.len(), 2);
    assert!(a[0].id < a[1].id, "memories sorted by stable id");
}

#[test]
fn resolving_event_clears_unresolved_promise() {
    let state = base_state();
    let rules = rules_with_events();
    let pack = dummy_pack(&rules);

    // Turn 1: a promise fires.
    let r1 = fire(&state, &rules, &["promise.share_umbrella"]);
    let hid1 = Uuid::new_v4();
    let mut memories = memories_from_turn(&r1, &pack, 1, hid1);
    assert!(memories[0].unresolved);

    // Turn 5: the resolving event fires; resolve_memories clears prior promise.
    let state5 = GameState {
        occurred_events: r1.state.occurred_events.clone(),
        ..r1.state.clone()
    };
    let r5 = fire(&state5, &rules, &["resolve_promise.share_umbrella"]);
    let fired: Vec<EventId> = r5.fired_events.clone();
    resolve_memories(&mut memories, &fired);
    assert!(
        !memories[0].unresolved,
        "resolve_promise event clears the prior promise's unresolved flag"
    );
}

#[test]
fn knowledge_owner_set_never_invented_beyond_participants_and_player() {
    let state = base_state();
    let rules = rules_with_events();
    let result = fire(&state, &rules, &["promise.share_umbrella"]);
    let m = &memories_from_turn(&result, &dummy_pack(&rules), 1, Uuid::new_v4())[0];
    // known_by must be exactly {player, yuna} — no other character invented.
    let mut expected = BTreeSet::new();
    expected.insert(CharacterId(PLAYER.to_string()));
    expected.insert(yuna_id());
    assert_eq!(m.facts[0].known_by, expected);
}

/// Minimal pack wrapper carrying the rules (only `rules.events` is read).
fn dummy_pack(rules: &StoryRules) -> story_model::StoryPack {
    use std::path::PathBuf;
    use story_model::{
        PackContentPolicy, PackInitialState, PackMetadata, PackRating, PromptFragments, StoryPack,
    };
    StoryPack {
        lorebook: story_model::Lorebook::default(),
        metadata: PackMetadata {
            schema_version: semver::Version::parse("1.0.0").unwrap(),
            story_id: "demo".to_string(),
            story_version: semver::Version::new(0, 1, 0),
            min_engine_version: semver::Version::new(0, 1, 0),
            title: "demo".to_string(),
            language: "ko-KR".to_string(),
            genres: vec![],
            rating: PackRating::General,
            default_choice_count: 3,
            initial_state: PackInitialState {
                chapter: "ch-1".to_string(),
                scene: "sc-1".to_string(),
                location: LocationId("cafe-namu".to_string()),
                time_slot: story_model::TimeSlot::Evening,
            },
            content: PackContentPolicy {
                violence: "none".to_string(),
            },
        },
        characters: BTreeMap::new(),
        rules: rules.clone(),
        world_md: String::new(),
        style_md: String::new(),
        opening_md: String::new(),
        prompts: PromptFragments {
            system: String::new(),
            director: String::new(),
            writer: String::new(),
        },
        pack_root: PathBuf::from("/tmp/demo"),
    }
}
