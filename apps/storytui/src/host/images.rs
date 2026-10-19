//! Scene image discovery for packs that ship an `images/` folder.
//!
//! Convention (no pack schema change): `images/locations/<location-id>.png`
//! wins, then `images/characters/<character-id>.png` for the primary
//! character. Extensions are probed in [`IMAGE_EXTENSIONS`] order. The pack
//! loader stays text-only (`ADR-003`); the host owns all file access.
//!
//! The transcript shows one image per scene boundary, so resolution is
//! per-location and cached in a [`LocationImageStore`] — a location decodes
//! at most once per pack session.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use image::DynamicImage;
use story_model::StoryPack;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use story_model::{
        CharacterDef, CharacterId, LocationDef, LocationId, PackContentPolicy, PackInitialState,
        PackMetadata, PackRating, PromptFragments, RelationshipStage, StoryRules, TimeSlot,
    };

    /// A minimal in-memory pack whose `pack_root` is `root`.
    fn pack_at(root: &Path) -> StoryPack {
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

    fn write_png(path: &Path) {
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
}
