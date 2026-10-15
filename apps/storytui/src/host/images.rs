//! Scene image discovery for packs that ship an `images/` folder.
//!
//! Convention (no pack schema change): `images/locations/<location-id>.png`
//! wins, then `images/characters/<character-id>.png` for the primary
//! character. Extensions are probed in [`IMAGE_EXTENSIONS`] order. The pack
//! loader stays text-only (`ADR-003`); the host owns all file access.

use std::path::{Path, PathBuf};

use story_core::engine::EngineState;
use story_model::StoryPack;

pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];

/// Candidate image paths for the current scene, most specific first:
/// location art, then the primary character's portrait.
pub fn scene_image_candidates(pack: &StoryPack, engine: &EngineState) -> Vec<PathBuf> {
    let images = pack.pack_root.join("images");
    let mut candidates: Vec<PathBuf> = Vec::with_capacity(IMAGE_EXTENSIONS.len() * 2);
    if is_safe_stem(engine.game.location.0.as_str()) {
        for ext in IMAGE_EXTENSIONS {
            candidates.push(
                images
                    .join("locations")
                    .join(format!("{}.{}", engine.game.location.0, ext)),
            );
        }
    }
    // The view's primary character is the first key of the state map; a
    // portrait is the fallback when the location has no art.
    if let Some(character_id) = engine.game.characters.keys().next()
        && is_safe_stem(character_id.0.as_str())
    {
        for ext in IMAGE_EXTENSIONS {
            candidates.push(
                images
                    .join("characters")
                    .join(format!("{}.{}", character_id.0, ext)),
            );
        }
    }
    candidates
}

/// First candidate that actually DECODES. A corrupt location image must not
/// permanently shadow a valid character portrait, so decode failures fall
/// through to the next candidate instead of ending the search.
pub fn load_scene_image(
    pack: &StoryPack,
    engine: &EngineState,
) -> Option<(PathBuf, image::DynamicImage)> {
    scene_image_candidates(pack, engine)
        .into_iter()
        .filter(|path| path.is_file())
        .find_map(|path| decode_scene_image(&path).map(|decoded| (path, decoded)))
}

/// Decode the scene image at `path`. Failures are logged and become "no
/// image" — a broken asset must never block play.
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

/// What the host should do with the scene-image pane after a refresh.
#[derive(Debug, PartialEq)]
pub enum SceneImageUpdate {
    /// The scene's image did not change — keep the current pane.
    Unchanged,
    /// A new image exists for the (new) scene.
    Set(image::DynamicImage),
    /// The new scene has no image (or the previous one vanished).
    Clear,
}

/// Remembers which path the current scene image came from so an unchanged
/// scene never re-decodes across turns.
#[derive(Default)]
pub struct SceneImageCache {
    last_path: Option<PathBuf>,
}

impl SceneImageCache {
    /// Resolve + (on change) decode the scene image for `engine`. The cache
    /// key is the winning candidate's path, so a scene whose image is decoded
    /// once is not re-read every turn.
    pub fn refresh(&mut self, pack: &StoryPack, engine: &EngineState) -> SceneImageUpdate {
        // Fast path: `last_path` only ever stores a decode-success path, so
        // its continued existence means the same usable image is still there.
        // This keeps every-turn PublishView refreshes decode-free.
        if self.last_path.as_ref().is_some_and(|path| path.is_file()) {
            return SceneImageUpdate::Unchanged;
        }
        let usable = load_scene_image(pack, engine);
        let path = usable.as_ref().map(|(path, _)| path.clone());
        if path == self.last_path {
            return SceneImageUpdate::Unchanged;
        }
        self.last_path = path;
        match usable {
            Some((_, image)) => SceneImageUpdate::Set(image),
            None => SceneImageUpdate::Clear,
        }
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
                initial_player_relationship: RelationshipStage::Acquaintance,
                speech_style: String::new(),
                honorific_rule: String::new(),
                frequent_phrases: vec![],
                avoid_phrases: vec![],
                known_facts: vec![],
                unknown_facts: vec![],
                current_goal: String::new(),
                long_term_desire: String::new(),
            },
        );
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
        let engine = EngineState::initial(&pack);

        // No images at all.
        assert!(load_scene_image(&pack, &engine).is_none());

        // Character portrait appears as the fallback.
        let portrait = dir.path().join("images/characters/yuna.png");
        write_png(&portrait);
        assert_eq!(
            load_scene_image(&pack, &engine).map(|(path, _)| path),
            Some(portrait.clone())
        );

        // A location image takes precedence.
        let scene = dir.path().join("images/locations/cafe.png");
        write_png(&scene);
        assert_eq!(
            load_scene_image(&pack, &engine).map(|(path, _)| path),
            Some(scene)
        );
    }

    #[test]
    fn extension_order_is_png_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pack = pack_at(dir.path());
        let engine = EngineState::initial(&pack);
        let jpg = dir.path().join("images/locations/cafe.jpg");
        write_png(&jpg);
        let png = dir.path().join("images/locations/cafe.png");
        write_png(&png);
        assert_eq!(
            load_scene_image(&pack, &engine).map(|(path, _)| path),
            Some(png),
            "png must win over jpg when both exist"
        );
    }

    #[test]
    fn unsafe_location_id_is_skipped_not_crashed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pack = pack_at(dir.path());
        let mut engine = EngineState::initial(&pack);
        engine.game.location = LocationId("../evil".to_string());
        assert!(load_scene_image(&pack, &engine).is_none());
    }

    #[test]
    fn broken_location_image_falls_through_to_character_portrait() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pack = pack_at(dir.path());
        let engine = EngineState::initial(&pack);
        // A corrupt location asset must not permanently shadow a valid
        // portrait: decode failure falls through to the next candidate.
        let broken = dir.path().join("images/locations/cafe.png");
        std::fs::create_dir_all(broken.parent().expect("parent")).expect("mkdir");
        std::fs::write(&broken, b"not a png").expect("write junk");
        let portrait = dir.path().join("images/characters/yuna.png");
        write_png(&portrait);

        assert_eq!(
            load_scene_image(&pack, &engine).map(|(path, _)| path),
            Some(portrait)
        );
    }

    #[test]
    fn unchanged_scene_does_not_redecode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pack = pack_at(dir.path());
        let engine = EngineState::initial(&pack);
        let scene = dir.path().join("images/locations/cafe.png");
        write_png(&scene);

        let mut cache = SceneImageCache::default();
        assert!(matches!(
            cache.refresh(&pack, &engine),
            SceneImageUpdate::Set(_)
        ));

        // Corrupt the bytes behind the SAME path. The cache trusts the
        // existing winner while the path exists: re-decoding here would fall
        // through and report Clear instead.
        std::fs::write(&scene, b"corrupted bytes").expect("corrupt");
        assert_eq!(
            cache.refresh(&pack, &engine),
            SceneImageUpdate::Unchanged,
            "an existing winner must never be re-decoded on every turn"
        );

        // Once the path disappears, the cache re-scans and clears.
        std::fs::remove_file(&scene).expect("remove");
        assert_eq!(cache.refresh(&pack, &engine), SceneImageUpdate::Clear);
    }

    #[test]
    fn decode_failure_with_no_fallback_clears() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pack = pack_at(dir.path());
        let engine = EngineState::initial(&pack);
        let broken = dir.path().join("images/locations/cafe.png");
        std::fs::create_dir_all(broken.parent().expect("parent")).expect("mkdir");
        std::fs::write(&broken, b"not a png").expect("write junk");
        assert!(load_scene_image(&pack, &engine).is_none());
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
