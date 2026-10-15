//! Pack validation: referential integrity and reachability (W4, ST-014).
//!
//! Each `check_*` walks the canonical [`StoryPack`] and collects errors.
//! [`validate_all`] is the top-level entry the loader calls; it dispatches
//! to every individual check. Checks are non-destructive and total: a
//! missing reference produces a [`PackError::Reference`] but does not
//! short-circuit the remaining checks.

use std::collections::{BTreeSet, VecDeque};

use story_model::{CharacterId, ConditionExpr, LocationId, StoryPack};

#[cfg(test)]
use std::collections::BTreeMap;

use crate::error::PackError;

/// Run every validation pass against `pack`, appending any errors to `errs`.
pub fn validate_all(pack: &StoryPack, errs: &mut Vec<PackError>) {
    errs.extend(check_references(pack));
    errs.extend(check_reachability(pack));
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
pub fn check_reachability(pack: &StoryPack) -> Vec<PackError> {
    if !pack
        .rules
        .locations
        .contains_key(&pack.metadata.initial_state.location)
    {
        // `check_references` owns the clearer missing-initial-location error.
        return Vec::new();
    }

    let mut reachable = BTreeSet::new();
    let mut queue = VecDeque::from([pack.metadata.initial_state.location.clone()]);
    while let Some(location) = queue.pop_front() {
        if !reachable.insert(location.clone()) {
            continue;
        }
        if let Some(definition) = pack.rules.locations.get(&location) {
            queue.extend(definition.adjacent.iter().cloned());
        }
    }

    pack.rules
        .locations
        .keys()
        .filter(|location| !reachable.contains(*location))
        .map(|location| PackError::Unreachable {
            detail: format!(
                "초기 위치 {}에서 인접 경로로 도달할 수 없는 location: {location}",
                pack.metadata.initial_state.location
            ),
        })
        .collect()
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
        lorebook: story_model::Lorebook::default(),
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
        EventDef, EventEffects, EventId, LocationDef, PackContentPolicy, PackInitialState,
        PackMetadata, PackRating, TimeSlot,
    };

    fn md(rating: PackRating) -> PackMetadata {
        PackMetadata {
            schema_version: semver::Version::new(1, 0, 0),
            story_id: "t".into(),
            story_version: semver::Version::new(0, 1, 0),
            min_engine_version: semver::Version::new(0, 1, 0),
            title: "x".into(),
            language: "ko-KR".into(),
            genres: Vec::new(),
            rating,
            default_choice_count: 3,
            initial_state: PackInitialState {
                chapter: "c1".into(),
                scene: "s1".into(),
                location: LocationId("loc1".into()),
                time_slot: TimeSlot::Evening,
            },
            content: PackContentPolicy {
                violence: "none".into(),
            },
        }
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
        let pack = minimal_pack(md(PackRating::General), BTreeMap::new(), rules_with_loc1());
        let errs = check_references(&pack);
        assert!(errs.is_empty(), "{errs:?}");
    }

    #[test]
    fn initial_state_missing_location_is_reference_error() {
        let mut metadata = md(PackRating::General);
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
        let mut metadata = md(PackRating::General);
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
        let mut metadata = md(PackRating::General);
        metadata.initial_state.location = LocationId("loc1".into());
        let pack = minimal_pack(metadata, BTreeMap::new(), rules);
        let errs = check_references(&pack);
        assert!(
            errs.iter()
                .any(|e| matches!(e, PackError::Reference { .. }))
        );
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
        let mut metadata = md(PackRating::General);
        metadata.initial_state.location = LocationId("loc1".into());
        let pack = minimal_pack(metadata, BTreeMap::new(), rules);
        let errs = check_references(&pack);
        assert!(
            errs.iter()
                .any(|e| matches!(e, PackError::Reference { .. }))
        );
    }

    #[test]
    fn disconnected_location_is_unreachable() {
        let mut rules = rules_with_loc1();
        rules.locations.insert(
            LocationId("isolated".into()),
            LocationDef {
                id: LocationId("isolated".into()),
                display_name: "고립".into(),
                adjacent: BTreeSet::new(),
            },
        );
        let pack = minimal_pack(md(PackRating::General), BTreeMap::new(), rules);
        let errors = check_reachability(&pack);
        assert!(matches!(errors.as_slice(), [PackError::Unreachable { .. }]));
    }
}
