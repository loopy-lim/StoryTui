//! Pack validation: referential integrity, reachability, teen metadata
//! (W4, ST-014).
//!
//! Each `check_*` walks the canonical [`StoryPack`] and collects errors.
//! [`validate_all`] is the top-level entry the loader calls; it dispatches
//! to every individual check. Checks are non-destructive and total: a
//! missing reference produces a [`PackError::Reference`] but does not
//! short-circuit the remaining checks.

use std::collections::BTreeSet;

use story_model::{CharacterId, ConditionExpr, LocationId, PackRating, StoryPack};

#[cfg(test)]
use std::collections::BTreeMap;

use crate::error::PackError;

/// Run every validation pass against `pack`, appending any errors to `errs`.
pub fn validate_all(pack: &StoryPack, errs: &mut Vec<PackError>) {
    errs.extend(check_references(pack));
    errs.extend(check_reachability(pack));
    errs.extend(check_content(pack));
}

/// Allow-lists of every id the loader knows about. Built once per pack and
/// threaded through the condition walker.
struct RefIndex {
    characters: BTreeSet<CharacterId>,
    locations: BTreeSet<LocationId>,
    flags: BTreeSet<String>,
    numbers: BTreeSet<String>,
    events: BTreeSet<String>,
    routes: BTreeSet<String>,
    endings: BTreeSet<String>,
}

impl RefIndex {
    fn from_pack(pack: &StoryPack) -> Self {
        Self {
            characters: pack.characters.keys().cloned().collect(),
            locations: pack.rules.locations.keys().cloned().collect(),
            flags: pack.rules.allowed_flags.clone(),
            numbers: pack.rules.numbers.keys().cloned().collect(),
            events: pack.rules.events.keys().map(|e| e.0.clone()).collect(),
            routes: pack.rules.routes.keys().cloned().collect(),
            endings: pack.rules.endings.keys().cloned().collect(),
        }
    }
}

/// Referential-integrity pass.
///
/// Collects every character/location/flag/number/string/item/event/route/ending
/// id referenced from:
/// - `initial_state`
/// - location adjacency
/// - event triggers + effects
/// - route `enter_condition`s
/// - ending `condition`s
///
/// and flags unknown ids and adjacency gaps.
pub fn check_references(pack: &StoryPack) -> Vec<PackError> {
    let mut errs = Vec::new();
    let idx = RefIndex::from_pack(pack);

    // 1. initial_state.location must exist in rules.locations.
    if !idx
        .locations
        .contains(&pack.metadata.initial_state.location)
    {
        errs.push(PackError::reference(format!(
            "initial_state.location 이(가) rules.toml 에 정의되지 않았습니다: {}",
            pack.metadata.initial_state.location
        )));
    }

    // 2. location adjacency must reference known locations.
    for (id, loc) in &pack.rules.locations {
        for adj in &loc.adjacent {
            if !idx.locations.contains(adj) {
                errs.push(PackError::reference(format!(
                    "위치 {id} 의 connects_to 가 알 수 없는 위치를 참조합니다: {adj}"
                )));
            }
        }
    }

    // 3. Events: trigger + every effect id-typed field.
    for (eid, ev) in &pack.rules.events {
        check_condition(
            &ev.trigger,
            &idx,
            eid.0.as_str(),
            "events.toml trigger",
            &mut errs,
        );
        for fid in ev.effects.set_flags.keys() {
            if !idx.flags.contains(fid) {
                errs.push(PackError::reference(format!(
                    "이벤트 {eid} 의 set_flags 가 등록되지 않은 flag 를 참조합니다: {fid}"
                )));
            }
        }
        for cid in ev.effects.affection_delta.keys() {
            if !idx.characters.contains(cid) {
                errs.push(PackError::reference(format!(
                    "이벤트 {eid} 의 affection_delta 가 알 수 없는 캐릭터를 참조합니다: {cid}"
                )));
            }
        }
        for cid in ev.effects.trust_delta.keys() {
            if !idx.characters.contains(cid) {
                errs.push(PackError::reference(format!(
                    "이벤트 {eid} 의 trust_delta 가 알 수 없는 캐릭터를 참조합니다: {cid}"
                )));
            }
        }
        for cid in ev.effects.tension_delta.keys() {
            if !idx.characters.contains(cid) {
                errs.push(PackError::reference(format!(
                    "이벤트 {eid} 의 tension_delta 가 알 수 없는 캐릭터를 참조합니다: {cid}"
                )));
            }
        }
        for nid in ev.effects.number_delta.keys() {
            if !idx.numbers.contains(nid) {
                errs.push(PackError::reference(format!(
                    "이벤트 {eid} 의 number_delta 가 등록되지 않은 변수를 참조합니다: {nid}"
                )));
            }
        }
        for added in &ev.effects.add_events {
            if !idx.events.contains(added) {
                errs.push(PackError::reference(format!(
                    "이벤트 {eid} 의 add_events 가 등록되지 않은 이벤트를 참조합니다: {added}"
                )));
            }
        }
        if let Some(loc) = &ev.effects.force_location
            && !idx.locations.contains(loc)
        {
            errs.push(PackError::reference(format!(
                "이벤트 {eid} 의 force_location 이 알 수 없는 위치입니다: {loc}"
            )));
        }
        for cid in ev.effects.force_relationship.keys() {
            if !idx.characters.contains(cid) {
                errs.push(PackError::reference(format!(
                    "이벤트 {eid} 의 force_relationship 이 알 수 없는 캐릭터입니다: {cid}"
                )));
            }
        }
        if let Some(route) = &ev.route_lock
            && !idx.routes.contains(route)
        {
            errs.push(PackError::reference(format!(
                "이벤트 {eid} 의 route_lock 이 알 수 없는 루트입니다: {route}"
            )));
        }
        if let Some(ending) = &ev.effects.ending
            && !idx.endings.contains(ending)
        {
            errs.push(PackError::reference(format!(
                "이벤트 {eid} 의 ending 이 알 수 없는 엔딩입니다: {ending}"
            )));
        }
    }

    // 4. Routes — enter_condition must reference known ids.
    for (rid, route) in &pack.rules.routes {
        check_condition(
            &route.enter_condition,
            &idx,
            rid,
            "rules.toml routes enter_condition",
            &mut errs,
        );
    }

    // 5. Endings — condition must reference known ids.
    for (eid, ending) in &pack.rules.endings {
        check_condition(
            &ending.condition,
            &idx,
            eid,
            "rules.toml endings condition",
            &mut errs,
        );
    }

    errs
}

/// Recursive walker for [`ConditionExpr`]. Each variant's id-typed field is
/// checked against the relevant allow-list in [`RefIndex`].
///
/// `ctx` is a label like `"events.toml trigger"` used in the error message.
fn check_condition(
    cond: &ConditionExpr,
    idx: &RefIndex,
    ctx_id: &str,
    ctx: &str,
    errs: &mut Vec<PackError>,
) {
    match cond {
        ConditionExpr::All { children } | ConditionExpr::Any { children } => {
            for c in children {
                check_condition(c, idx, ctx_id, ctx, errs);
            }
        }
        ConditionExpr::Not { child } => {
            check_condition(child, idx, ctx_id, ctx, errs);
        }
        ConditionExpr::FlagEquals { id, value: _ } => {
            if !idx.flags.contains(id) {
                errs.push(PackError::reference(format!(
                    "{ctx} ({ctx_id}): 등록되지 않은 flag 참조: {id}"
                )));
            }
        }
        ConditionExpr::NumberCompare {
            id,
            op: _,
            value: _,
        } => {
            if !idx.numbers.contains(id) {
                errs.push(PackError::reference(format!(
                    "{ctx} ({ctx_id}): 등록되지 않은 number 참조: {id}"
                )));
            }
        }
        ConditionExpr::AffectionAtLeast {
            character,
            value: _,
        }
        | ConditionExpr::TrustAtLeast {
            character,
            value: _,
        } => {
            if !idx.characters.contains(character) {
                errs.push(PackError::reference(format!(
                    "{ctx} ({ctx_id}): 알 수 없는 캐릭터 참조: {character}"
                )));
            }
        }
        ConditionExpr::EventOccurred { id } => {
            if !idx.events.contains(id) {
                errs.push(PackError::reference(format!(
                    "{ctx} ({ctx_id}): 등록되지 않은 event 참조: {id}"
                )));
            }
        }
        ConditionExpr::AtLocation { location } => {
            if !idx.locations.contains(location) {
                errs.push(PackError::reference(format!(
                    "{ctx} ({ctx_id}): 알 수 없는 위치 참조: {location}"
                )));
            }
        }
        ConditionExpr::TurnAtLeast { value: _ } => {}
    }
}

/// Reachability pass.
///
/// W4 is intentionally permissive — the engine evaluates conditions at
/// runtime. We keep the function as a seam so future static reachability
/// analysis (e.g. unreachable required events) can land without changing
/// the loader's call sites.
pub fn check_reachability(_pack: &StoryPack) -> Vec<PackError> {
    Vec::new()
}

/// teen-metadata pass.
///
/// If `rating == teen`, requires:
/// - `all_major_characters_safe == true`, AND
/// - every character `age` present and `>= 18`.
///
/// Any character lacking `age` is flagged.
pub fn check_content(pack: &StoryPack) -> Vec<PackError> {
    let mut errs = Vec::new();
    if pack.metadata.rating != PackRating::teen {
        return errs;
    }
    if !pack.metadata.all_major_characters_safe {
        errs.push(PackError::ContentPolicyViolation {
            detail: "rating=teen 이지만 all_major_characters_safe=false 입니다".to_string(),
        });
    }
    // MVP: every character in the `characters` map is treated as major.
    for (id, def) in &pack.characters {
        match def.age {
            Some(a) if a >= 18 => {}
            Some(a) => {
                errs.push(PackError::ContentPolicyViolation {
                    detail: format!("모든 인물 Pack 의 주요 캐릭터 {id} 나이가 18 미만입니다: {a}"),
                });
            }
            None => {
                errs.push(PackError::ContentPolicyViolation {
                    detail: format!(
                        "모든 인물 Pack 의 주요 캐릭터 {id} 나이(age)가 선언되지 않았습니다"
                    ),
                });
            }
        }
    }
    errs
}

/// Helper used by tests to build a minimal canonical pack from parts.
#[cfg(test)]
pub(crate) fn minimal_pack(
    metadata: story_model::PackMetadata,
    characters: BTreeMap<CharacterId, story_model::CharacterDef>,
    rules: story_model::StoryRules,
) -> StoryPack {
    use story_model::PromptFragments;
    StoryPack {
        metadata,
        characters,
        rules,
        world_md: String::new(),
        style_md: String::new(),
        opening_md: String::new(),
        prompts: PromptFragments {
            system: String::new(),
            director: String::new(),
            writer: String::new(),
        },
        pack_root: std::path::PathBuf::from("/tmp/test"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use story_model::{
        CharacterDef, EventDef, EventEffects, EventId, LocationDef, PackContentPolicy,
        PackInitialState, PackMetadata, RelationshipStage, TimeSlot,
    };

    fn md(rating: PackRating, teens: bool) -> PackMetadata {
        PackMetadata {
            schema_version: semver::Version::new(1, 0, 0),
            story_id: "t".into(),
            story_version: semver::Version::new(0, 1, 0),
            min_engine_version: semver::Version::new(0, 1, 0),
            title: "x".into(),
            language: "ko-KR".into(),
            genres: Vec::new(),
            rating,
            all_major_characters_safe: teens,
            default_choice_count: 3,
            initial_state: PackInitialState {
                chapter: "c1".into(),
                scene: "s1".into(),
                location: LocationId("loc1".into()),
                time_slot: TimeSlot::Evening,
            },
            content: PackContentPolicy {
                safe_content: "none".into(),
                violence: "none".into(),
            },
        }
    }

    fn char(id: &str, age: Option<u32>) -> (CharacterId, CharacterDef) {
        (
            CharacterId(id.into()),
            CharacterDef {
                id: CharacterId(id.into()),
                name: id.into(),
                age,
                pronouns: None,
                personality: Vec::new(),
                appearance: Vec::new(),
                values: Vec::new(),
                likes: Vec::new(),
                dislikes: Vec::new(),
                secrets: Vec::new(),
                initial_player_relationship: RelationshipStage::Acquaintance,
                speech_style: String::new(),
                honorific_rule: String::new(),
                frequent_phrases: Vec::new(),
                avoid_phrases: Vec::new(),
                known_facts: Vec::new(),
                unknown_facts: Vec::new(),
                current_goal: String::new(),
                long_term_desire: String::new(),
            },
        )
    }

    fn rules_with_loc1() -> story_model::StoryRules {
        let mut r = story_model::StoryRules::minimal();
        r.locations.insert(
            LocationId("loc1".into()),
            LocationDef {
                id: LocationId("loc1".into()),
                display_name: "L1".into(),
                adjacent: BTreeSet::new(),
            },
        );
        r
    }

    #[test]
    fn empty_pack_no_refs_no_errors() {
        let pack = minimal_pack(
            md(PackRating::General, false),
            BTreeMap::new(),
            rules_with_loc1(),
        );
        let errs = check_references(&pack);
        assert!(errs.is_empty(), "{errs:?}");
    }

    #[test]
    fn initial_state_missing_location_is_reference_error() {
        let mut metadata = md(PackRating::General, false);
        metadata.initial_state.location = LocationId("nope".into());
        let pack = minimal_pack(
            metadata,
            BTreeMap::new(),
            story_model::StoryRules::minimal(),
        );
        let errs = check_references(&pack);
        assert!(
            errs.iter()
                .any(|e| matches!(e, PackError::Reference { .. }))
        );
    }

    #[test]
    fn adjacency_unknown_target_is_reference_error() {
        let mut rules = rules_with_loc1();
        let loc = rules.locations.get_mut(&LocationId("loc1".into())).unwrap();
        loc.adjacent.insert(LocationId("nope".into()));
        let mut metadata = md(PackRating::General, false);
        metadata.initial_state.location = LocationId("loc1".into());
        let pack = minimal_pack(metadata, BTreeMap::new(), rules);
        let errs = check_references(&pack);
        assert!(
            errs.iter()
                .any(|e| matches!(e, PackError::Reference { .. }))
        );
    }

    #[test]
    fn event_trigger_unknown_flag_is_reference_error() {
        let mut rules = rules_with_loc1();
        let trigger = ConditionExpr::FlagEquals {
            id: "ghost".into(),
            value: true,
        };
        rules.events.insert(
            EventId("event.x".into()),
            EventDef {
                id: EventId("event.x".into()),
                trigger,
                priority: 0,
                effects: EventEffects::default(),
                allow_relationship_skip: false,
                route_lock: None,
            },
        );
        let mut metadata = md(PackRating::General, false);
        metadata.initial_state.location = LocationId("loc1".into());
        let pack = minimal_pack(metadata, BTreeMap::new(), rules);
        let errs = check_references(&pack);
        assert!(
            errs.iter()
                .any(|e| matches!(e, PackError::Reference { .. }))
        );
    }

    #[test]
    fn teen_pack_requires_teens_flag_true() {
        let pack = minimal_pack(
            md(PackRating::teen, false),
            BTreeMap::new(),
            rules_with_loc1(),
        );
        let errs = check_content(&pack);
        assert!(
            errs.iter()
                .any(|e| matches!(e, PackError::ContentPolicyViolation { .. }))
        );
    }

    #[test]
    fn teen_pack_rejects_under_18_age() {
        let mut characters = BTreeMap::new();
        let (id, def) = char("yuna", Some(17));
        characters.insert(id, def);
        let pack = minimal_pack(
            md(PackRating::teen, true),
            characters,
            story_model::StoryRules::minimal(),
        );
        let errs = check_content(&pack);
        assert!(
            errs.iter()
                .any(|e| matches!(e, PackError::ContentPolicyViolation { .. }))
        );
    }

    #[test]
    fn teen_pack_rejects_missing_age() {
        let mut characters = BTreeMap::new();
        let (id, def) = char("yuna", None);
        characters.insert(id, def);
        let pack = minimal_pack(
            md(PackRating::teen, true),
            characters,
            story_model::StoryRules::minimal(),
        );
        let errs = check_content(&pack);
        assert!(
            errs.iter()
                .any(|e| matches!(e, PackError::ContentPolicyViolation { .. }))
        );
    }

    #[test]
    fn teen_pack_with_18_plus_is_ok() {
        let mut characters = BTreeMap::new();
        let (id, def) = char("yuna", Some(29));
        characters.insert(id, def);
        let pack = minimal_pack(
            md(PackRating::teen, true),
            characters,
            story_model::StoryRules::minimal(),
        );
        let errs = check_content(&pack);
        assert!(errs.is_empty(), "{errs:?}");
    }

    #[test]
    fn general_pack_ignores_age_policy() {
        let mut characters = BTreeMap::new();
        let (id, def) = char("kid", Some(8));
        characters.insert(id, def);
        let pack = minimal_pack(
            md(PackRating::General, false),
            characters,
            story_model::StoryRules::minimal(),
        );
        let errs = check_content(&pack);
        assert!(errs.is_empty());
    }

    #[test]
    fn nested_condition_unknown_number_is_caught() {
        let mut rules = rules_with_loc1();
        let trigger = ConditionExpr::All {
            children: vec![
                ConditionExpr::NumberCompare {
                    id: "ghost.counter".into(),
                    op: story_model::CompareOp::Ge,
                    value: 1,
                },
                ConditionExpr::TurnAtLeast { value: 2 },
            ],
        };
        rules.events.insert(
            EventId("event.x".into()),
            EventDef {
                id: EventId("event.x".into()),
                trigger,
                priority: 0,
                effects: EventEffects::default(),
                allow_relationship_skip: false,
                route_lock: None,
            },
        );
        let mut metadata = md(PackRating::General, false);
        metadata.initial_state.location = LocationId("loc1".into());
        let pack = minimal_pack(metadata, BTreeMap::new(), rules);
        let errs = check_references(&pack);
        assert!(
            errs.iter()
                .any(|e| matches!(e, PackError::Reference { .. }))
        );
    }
}
