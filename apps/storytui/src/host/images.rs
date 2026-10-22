//! Scene image discovery for packs that ship an `images/` folder.
//!
//! Convention (no pack schema change): `images/locations/<location-id>.png`
//! wins, then `images/characters/<character-id>.png` for the primary
//! character. Expression sprites extend the same convention:
//! `images/expressions/<character-id>/<emotion>.png` — the keyword-classified
//! turn emotion picks the sprite, the character portrait is the fallback.
//! Extensions are probed in [`IMAGE_EXTENSIONS`] order. The pack loader stays
//! text-only (`ADR-003`); the host owns all file access.
//!
//! The transcript shows one image per scene boundary (and one sprite per
//! emotion change), so resolution is cached per key in [`LocationImageStore`]
//! / [`ExpressionImageStore`] — each decodes at most once per pack session.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use image::DynamicImage;
use story_model::{Emotion, StoryPack};

pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];

/// Candidate image paths for `location`, most specific first: location art,
/// then the primary character's portrait.
pub fn location_image_candidates(
    pack: &StoryPack,
    location: &str,
    primary_character: Option<&str>,
) -> Vec<PathBuf> {
    let images = pack.pack_root.join("images");
    let mut candidates: Vec<PathBuf> = Vec::with_capacity(IMAGE_EXTENSIONS.len() * 2);
    if is_safe_stem(location) {
        for ext in IMAGE_EXTENSIONS {
            candidates.push(images.join("locations").join(format!("{location}.{ext}")));
        }
    }
    // A portrait is the fallback when the location has no art.
    if let Some(character_id) = primary_character
        && is_safe_stem(character_id)
    {
        for ext in IMAGE_EXTENSIONS {
            candidates.push(
                images
                    .join("characters")
                    .join(format!("{character_id}.{ext}")),
            );
        }
    }
    candidates
}

/// First candidate that actually DECODES. A corrupt location image must not
/// permanently shadow a valid character portrait, so decode failures fall
/// through to the next candidate instead of ending the search.
pub fn load_location_image(
    pack: &StoryPack,
    location: &str,
    primary_character: Option<&str>,
) -> Option<(PathBuf, DynamicImage)> {
    location_image_candidates(pack, location, primary_character)
        .into_iter()
        .filter(|path| path.is_file())
        .find_map(|path| decode_scene_image(&path).map(|decoded| (path, decoded)))
}

/// Decode the image at `path`. Failures are logged and become "no image" — a
/// broken asset must never block play.
pub fn decode_scene_image(path: &Path) -> Option<image::DynamicImage> {
    match image::ImageReader::open(path) {
        Ok(reader) => match reader.decode() {
            Ok(decoded) => Some(decoded),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "scene image decode failed");
                None
            }
        },
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "scene image open failed");
            None
        }
    }
}

/// Pack ids reach this path joiner; refuse anything that could escape the
/// pack's `images/` directory.
fn is_safe_stem(stem: &str) -> bool {
    !stem.is_empty() && stem != "." && stem != ".." && !stem.contains(['/', '\\', '\0'])
}

/// Per-location decode cache. The value is the winning candidate's path
/// (`None` = resolved but no usable art — an authoritative absence).
///
/// Invariant: this store and the App's per-location image map are cleared
/// together on a pack switch (`host::reset_scene_images`). A fresh store is
/// authoritative — after a clear, every location resolves again and reports
/// its outcome, so a stale image from the previous pack can never survive.
#[derive(Default)]
pub struct LocationImageStore {
    resolved: HashMap<String, Option<PathBuf>>,
}

impl LocationImageStore {
    /// Resolve + decode every location in `locations` not already cached
    /// under a still-existing path. Returns ONLY locations whose outcome
    /// changed (freshly resolved, or re-resolved differently after their
    /// file vanished) so the host pushes them into the app exactly once.
    pub fn load_missing(
        &mut self,
        pack: &StoryPack,
        locations: &[String],
        primary_character: Option<&str>,
    ) -> Vec<(String, Option<(PathBuf, DynamicImage)>)> {
        let mut updates = Vec::new();
        for location in locations {
            // A known winner skips re-decoding while its file exists; this
            // keeps every-publish refreshes decode-free.
            if let Some(Some(path)) = self.resolved.get(location)
                && path.is_file()
            {
                continue;
            }
            let usable = load_location_image(pack, location, primary_character);
            let path = usable.as_ref().map(|(path, _)| path.clone());
            if self.resolved.get(location) != Some(&path) {
                updates.push((location.clone(), usable));
            }
            self.resolved.insert(location.clone(), path);
        }
        updates
    }

    /// Drop every resolution. The caller MUST also clear the App's image map
    /// in the same step (see the invariant on [`LocationImageStore`]).
    pub fn clear(&mut self) {
        self.resolved.clear();
    }
}

/// `Emotion` variant name as a sprite file stem. Every variant is a single
/// CamelCase word, so lowercase matches the serde `snake_case` spelling the
/// pack convention documents (`Affectionate` → `affectionate`).
pub fn emotion_stem(emotion: Emotion) -> String {
    format!("{emotion:?}").to_lowercase()
}

/// Candidate sprite paths for `emotion` of `character`, most specific first:
/// the emotion sprite, then the character's portrait (the neutral stand-in
/// when a pack ships no art for this emotion).
pub fn expression_image_candidates(
    pack: &StoryPack,
    character: &str,
    emotion: Emotion,
) -> Vec<PathBuf> {
    let expressions = pack.pack_root.join("images").join("expressions");
    let mut candidates: Vec<PathBuf> =
        Vec::with_capacity(IMAGE_EXTENSIONS.len() * (1 + IMAGE_EXTENSIONS.len()));
    if is_safe_stem(character) {
        let stem = emotion_stem(emotion);
        for ext in IMAGE_EXTENSIONS {
            candidates.push(expressions.join(character).join(format!("{stem}.{ext}")));
        }
        // A portrait is the fallback when the emotion has no sprite.
        for ext in IMAGE_EXTENSIONS {
            candidates.push(
                pack.pack_root
                    .join("images")
                    .join("characters")
                    .join(format!("{character}.{ext}")),
            );
        }
    }
    candidates
}

/// First candidate that actually DECODES — same fall-through contract as
/// [`load_location_image`]: a corrupt sprite must not shadow the portrait.
pub fn load_expression_image(
    pack: &StoryPack,
    character: &str,
    emotion: Emotion,
) -> Option<(PathBuf, DynamicImage)> {
    expression_image_candidates(pack, character, emotion)
        .into_iter()
        .filter(|path| path.is_file())
        .find_map(|path| decode_scene_image(&path).map(|decoded| (path, decoded)))
}

/// Per-emotion sprite cache. Same authoritative-absence invariant as
/// [`LocationImageStore`]: `None` means resolved-but-unusable, and the paired
/// `App::reset_expression_images` must run in lockstep on a pack switch.
#[derive(Default)]
pub struct ExpressionImageStore {
    resolved: HashMap<Emotion, Option<PathBuf>>,
}

impl ExpressionImageStore {
    /// Resolve + decode every `emotion` in `emotions` not already cached
    /// under a still-existing path, for `character` (the pack's primary).
    /// Returns ONLY emotions whose outcome changed, so the host pushes them
    /// into the app exactly once.
    pub fn load_missing(
        &mut self,
        pack: &StoryPack,
        character: &str,
        emotions: &[Emotion],
    ) -> Vec<(Emotion, Option<(PathBuf, DynamicImage)>)> {
        let mut updates = Vec::new();
        for emotion in emotions {
            if let Some(Some(path)) = self.resolved.get(emotion)
                && path.is_file()
            {
                continue;
            }
            let usable = load_expression_image(pack, character, *emotion);
            let path = usable.as_ref().map(|(path, _)| path.clone());
            if self.resolved.get(emotion) != Some(&path) {
                updates.push((*emotion, usable));
            }
            self.resolved.insert(*emotion, path);
        }
        updates
    }

    /// Drop every resolution. Lockstep with `App::reset_expression_images`.
    pub fn clear(&mut self) {
        self.resolved.clear();
    }
}

#[cfg(test)]
pub(in crate::host) mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use story_model::{
        CharacterDef, CharacterId, Emotion, LocationDef, LocationId, PackContentPolicy,
        PackInitialState, PackMetadata, PackRating, PromptFragments, RelationshipStage, StoryRules,
        TimeSlot,
    };

    /// A minimal in-memory pack whose `pack_root` is `root`.
    pub(in crate::host) fn pack_at(root: &Path) -> StoryPack {
        let metadata = PackMetadata {
            schema_version: semver::Version::new(1, 0, 0),
            story_id: "test".to_string(),
            story_version: semver::Version::new(0, 1, 0),
            min_engine_version: semver::Version::new(0, 1, 0),
            title: "t".to_string(),
            language: "ko-KR".to_string(),
            genres: vec![],
            rating: PackRating::Teen,
            default_choice_count: 3,
            initial_state: PackInitialState {
                chapter: "ch".to_string(),
                scene: "sc".to_string(),
                location: LocationId("cafe".to_string()),
                time_slot: TimeSlot::Evening,
            },
            content: PackContentPolicy {
                violence: "mild".to_string(),
            },
        };
        let mut locations = BTreeMap::new();
        locations.insert(
            LocationId("cafe".to_string()),
            LocationDef {
                id: LocationId("cafe".to_string()),
                display_name: "카페".to_string(),
                adjacent: Default::default(),
            },
        );
        let mut rules = StoryRules::minimal();
        rules.locations = locations;
        let mut characters = BTreeMap::new();
        characters.insert(
            CharacterId("yuna".to_string()),
            CharacterDef {
                id: CharacterId("yuna".to_string()),
                name: "한유나".to_string(),
                age: None,
                pronouns: None,
                description_md: None,
                dialogue_examples: Vec::new(),
                personality: vec![],
                appearance: vec![],
                values: vec![],
                likes: vec![],
                dislikes: vec![],
                secrets: vec![],
                known_facts: vec![],
                unknown_facts: vec![],
                current_goal: String::new(),
                long_term_desire: String::new(),
                initial_player_relationship: RelationshipStage::Acquaintance,
                speech_style: String::new(),
                honorific_rule: String::new(),
                frequent_phrases: vec![],
                avoid_phrases: vec![],
            },
        );
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
            pack_root: root.to_path_buf(),
        }
    }

    pub(in crate::host) fn write_png(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create image dir");
        }
        image::DynamicImage::new_rgb8(4, 4)
            .save(path)
            .expect("write test png");
    }

    #[test]
    fn location_image_wins_then_character_falls_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pack = pack_at(dir.path());

        // No images at all.
        assert!(load_location_image(&pack, "cafe", Some("yuna")).is_none());

        // Character portrait appears as the fallback.
        let portrait = dir.path().join("images/characters/yuna.png");
        write_png(&portrait);
        assert_eq!(
            load_location_image(&pack, "cafe", Some("yuna")).map(|(path, _)| path),
            Some(portrait.clone())
        );

        // A location image takes precedence.
        let scene = dir.path().join("images/locations/cafe.png");
        write_png(&scene);
        assert_eq!(
            load_location_image(&pack, "cafe", Some("yuna")).map(|(path, _)| path),
            Some(scene)
        );
    }

    #[test]
    fn extension_order_is_png_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pack = pack_at(dir.path());
        let jpg = dir.path().join("images/locations/cafe.jpg");
        write_png(&jpg);
        let png = dir.path().join("images/locations/cafe.png");
        write_png(&png);
        assert_eq!(
            load_location_image(&pack, "cafe", None).map(|(path, _)| path),
            Some(png),
            "png must win over jpg when both exist"
        );
    }

    #[test]
    fn unsafe_location_id_is_skipped_not_crashed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pack = pack_at(dir.path());
        assert!(load_location_image(&pack, "../evil", None).is_none());
    }

    #[test]
    fn broken_location_image_falls_through_to_character_portrait() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pack = pack_at(dir.path());
        // A corrupt location asset must not permanently shadow a valid
        // portrait: decode failure falls through to the next candidate.
        let broken = dir.path().join("images/locations/cafe.png");
        std::fs::create_dir_all(broken.parent().expect("parent")).expect("mkdir");
        std::fs::write(&broken, b"not a png").expect("write junk");
        let portrait = dir.path().join("images/characters/yuna.png");
        write_png(&portrait);

        assert_eq!(
            load_location_image(&pack, "cafe", Some("yuna")).map(|(path, _)| path),
            Some(portrait)
        );
    }

    #[test]
    fn resolved_locations_are_not_redecoded_until_the_file_vanishes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pack = pack_at(dir.path());
        let scene = dir.path().join("images/locations/cafe.png");
        write_png(&scene);

        let mut store = LocationImageStore::default();
        let updates = store.load_missing(&pack, &["cafe".to_string()], None);
        assert_eq!(updates.len(), 1, "fresh store resolves and reports");
        assert!(updates[0].1.is_some());

        // Corrupt the bytes behind the SAME path: the cache trusts the
        // existing winner while the path exists, so nothing is re-reported.
        std::fs::write(&scene, b"corrupted bytes").expect("corrupt");
        assert!(
            store
                .load_missing(&pack, &["cafe".to_string()], None)
                .is_empty()
        );

        // Once the path disappears, the store re-scans and reports the
        // changed outcome (now absent).
        std::fs::remove_file(&scene).expect("remove");
        let updates = store.load_missing(&pack, &["cafe".to_string()], None);
        assert_eq!(updates.len(), 1);
        assert!(updates[0].1.is_none());
    }

    #[test]
    fn cleared_store_re_resolves_and_reports_absence() {
        // A pack switch resets the store. The fresh store must be
        // authoritative: a pack without images reports its absence (so the
        // paired app-side clear is followed by a real resolution), never
        // silence that would keep the previous pack's image alive.
        let dir = tempfile::tempdir().expect("tempdir");
        let pack = pack_at(dir.path());

        let mut store = LocationImageStore::default();
        assert!(
            store.load_missing(&pack, &["cafe".to_string()], None)[0]
                .1
                .is_none()
        );
        store.clear();
        let updates = store.load_missing(&pack, &["cafe".to_string()], None);
        assert_eq!(updates.len(), 1, "fresh store must re-resolve");
        assert!(updates[0].1.is_none());
    }

    #[test]
    fn safe_stem_rejects_traversal() {
        assert!(is_safe_stem("cafe-namu"));
        assert!(!is_safe_stem("../etc"));
        assert!(!is_safe_stem("a/b"));
        assert!(!is_safe_stem(""));
        assert!(!is_safe_stem(".."));
        assert!(!is_safe_stem("."));
    }

    #[test]
    fn emotion_stem_matches_the_sprite_convention() {
        // All variants are single CamelCase words, so lowercase IS snake_case
        // — the documented file naming (`Affectionate` → `affectionate.png`).
        assert_eq!(emotion_stem(Emotion::Neutral), "neutral");
        assert_eq!(emotion_stem(Emotion::Affectionate), "affectionate");
        assert_eq!(emotion_stem(Emotion::Embarrassed), "embarrassed");
    }

    #[test]
    fn expression_sprite_wins_then_portrait_falls_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pack = pack_at(dir.path());

        // No art at all.
        assert!(load_expression_image(&pack, "yuna", Emotion::Happy).is_none());

        // Portrait appears as the fallback.
        let portrait = dir.path().join("images/characters/yuna.png");
        write_png(&portrait);
        assert_eq!(
            load_expression_image(&pack, "yuna", Emotion::Happy).map(|(path, _)| path),
            Some(portrait.clone())
        );

        // The emotion sprite takes precedence.
        let sprite = dir.path().join("images/expressions/yuna/happy.png");
        write_png(&sprite);
        assert_eq!(
            load_expression_image(&pack, "yuna", Emotion::Happy).map(|(path, _)| path),
            Some(sprite)
        );
    }

    #[test]
    fn broken_expression_sprite_falls_through_to_portrait() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pack = pack_at(dir.path());
        let broken = dir.path().join("images/expressions/yuna/sad.png");
        std::fs::create_dir_all(broken.parent().expect("parent")).expect("mkdir");
        std::fs::write(&broken, b"not a png").expect("write junk");
        let portrait = dir.path().join("images/characters/yuna.png");
        write_png(&portrait);

        assert_eq!(
            load_expression_image(&pack, "yuna", Emotion::Sad).map(|(path, _)| path),
            Some(portrait)
        );
    }

    #[test]
    fn unsafe_character_id_yields_no_expression_candidates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pack = pack_at(dir.path());
        assert!(load_expression_image(&pack, "../evil", Emotion::Happy).is_none());
    }

    #[test]
    fn sprite_store_caches_until_the_file_vanishes_and_clear_re_resolves() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pack = pack_at(dir.path());
        let sprite = dir.path().join("images/expressions/yuna/happy.png");
        write_png(&sprite);

        let mut store = ExpressionImageStore::default();
        let updates = store.load_missing(&pack, "yuna", &[Emotion::Happy]);
        assert_eq!(updates.len(), 1, "fresh store resolves and reports");
        assert!(updates[0].1.is_some());

        // Cached winner is trusted while the path exists.
        assert!(
            store
                .load_missing(&pack, "yuna", &[Emotion::Happy])
                .is_empty()
        );

        // Once the file vanishes the store re-resolves and reports absence.
        std::fs::remove_file(&sprite).expect("remove");
        let updates = store.load_missing(&pack, "yuna", &[Emotion::Happy]);
        assert_eq!(updates.len(), 1);
        assert!(updates[0].1.is_none());

        // A cleared store re-resolves authoritatively (pack-switch lockstep).
        store.clear();
        let updates = store.load_missing(&pack, "yuna", &[Emotion::Happy]);
        assert_eq!(updates.len(), 1, "fresh store must re-resolve");
        assert!(updates[0].1.is_none());
    }
}
