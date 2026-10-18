//! Story-pack discovery for the start-menu pack picker: scan the sibling
//! directories of the active pack and surface loadable ones.

use std::path::Path;

/// One selectable pack: directory (identity for the swap request) plus the
/// display label parts parsed from its manifest.
pub struct PackEntry {
    pub dir: String,
    pub title: String,
    pub language: String,
}

/// The handful of `story.toml` fields the picker needs. A full pack load is
/// deliberately skipped here — a broken pack must degrade to "skipped", not
/// block the menu.
#[derive(serde::Deserialize)]
struct PackManifest {
    #[serde(default)]
    title: String,
    #[serde(default)]
    language: String,
}

/// Scan the siblings of `current_pack_dir` for packs. Corrupted or
/// incomplete entries are logged and skipped so one bad directory can never
/// take the menu down. If the active pack lives outside the scan root (e.g.
/// loaded via `--pack` from elsewhere), only that pack is listed.
pub fn scan_packs(current_pack_dir: &Path) -> Vec<PackEntry> {
    let Some(scan_root) = current_pack_dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    else {
        return vec![pack_entry(current_pack_dir)];
    };
    let mut entries: Vec<PackEntry> = Vec::new();
    let Ok(dirs) = std::fs::read_dir(scan_root) else {
        return vec![pack_entry(current_pack_dir)];
    };
    for dir in dirs.flatten() {
        let path = dir.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = dir.file_name().to_string_lossy().to_string();
        if dir_name.starts_with('.') {
            continue;
        }
        let manifest_path = path.join("story.toml");
        if !manifest_path.is_file() {
            continue;
        }
        let manifest = match std::fs::read_to_string(&manifest_path)
            .map_err(|e| e.to_string())
            .and_then(|text| toml::from_str::<PackManifest>(&text).map_err(|e| e.to_string()))
        {
            Ok(manifest) => manifest,
            Err(error) => {
                tracing::warn!(pack = %dir_name, %error, "pack scan skipped a broken pack");
                continue;
            }
        };
        let title = if manifest.title.trim().is_empty() {
            dir_name.clone()
        } else {
            manifest.title
        };
        entries.push(PackEntry {
            dir: path.display().to_string(),
            title,
            language: manifest.language,
        });
        if entries.len() >= 32 {
            break;
        }
    }
    if !entries
        .iter()
        .any(|entry| entry.dir == current_pack_dir.display().to_string())
    {
        entries.push(pack_entry(current_pack_dir));
    }
    entries.sort_by(|a, b| a.title.cmp(&b.title));
    entries
}

/// A fallback entry for the active pack, in case it is not a sibling of the
/// scan root or the root is unreadable.
fn pack_entry(current_pack_dir: &Path) -> PackEntry {
    let manifest = std::fs::read_to_string(current_pack_dir.join("story.toml"))
        .ok()
        .and_then(|text| toml::from_str::<PackManifest>(&text).ok());
    let (title, language) = manifest.map(|m| (m.title, m.language)).unwrap_or_default();
    PackEntry {
        dir: current_pack_dir.display().to_string(),
        title: if title.trim().is_empty() {
            current_pack_dir
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| current_pack_dir.display().to_string())
        } else {
            title
        },
        language,
    }
}

/// One row of the import file browser: a directory, or a card file whose
/// extension the importer accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Extensions the card importer accepts (case-insensitive on the file name).
const CARD_EXTENSIONS: &[&str] = &["png", "json", "charx"];

/// List `dir` for the import file browser: directories first (sorted), then
/// card files. Hidden (dot-prefixed) entries and files the importer cannot
/// parse are skipped. Symlinks do not count as directories — the browser
/// never descends through one.
pub fn list_dir_for_browse(dir: &Path) -> Result<Vec<BrowseEntry>, String> {
    let mut dirs: Vec<String> = Vec::new();
    let mut cards: Vec<String> = Vec::new();
    let entries =
        std::fs::read_dir(dir).map_err(|error| format!("하위를 읽지 못했습니다: {error}"))?;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if file_type.is_dir() {
            dirs.push(name);
        } else if file_type.is_file() {
            let extension = name
                .rsplit('.')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if CARD_EXTENSIONS.contains(&extension.as_str()) {
                cards.push(name);
            }
        }
    }
    dirs.sort();
    cards.sort();
    Ok(dirs
        .into_iter()
        .map(|name| BrowseEntry { name, is_dir: true })
        .chain(cards.into_iter().map(|name| BrowseEntry {
            name,
            is_dir: false,
        }))
        .collect())
}

/// Delete a pack directory. Guarded: the target must be a direct child of
/// the scan root, hold a `story.toml`, and not be the active pack — any
/// guard violation returns the Korean error text without touching the file
/// system. Saves are not touched (they live in the save store).
pub fn delete_pack(target: &Path, scan_root: &Path, active_pack_dir: &Path) -> Result<(), String> {
    if target == active_pack_dir {
        return Err("현재 플레이 중인 팩은 삭제할 수 없습니다.".to_string());
    }
    let Some(target_parent) = target.parent() else {
        return Err("팩 경로가 올바르지 않습니다.".to_string());
    };
    if target_parent != scan_root {
        return Err("팩 목록의 디렉터리만 삭제할 수 있습니다.".to_string());
    }
    if !target.join("story.toml").is_file() {
        return Err("story.toml이 없는 디렉터리는 팩으로 삭제할 수 없습니다.".to_string());
    }
    std::fs::remove_dir_all(target)
        .map_err(|error| format!("팩 디렉터리를 삭제하지 못했습니다: {error}"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    fn test_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "storytui-packscan-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create test directory");
        dir
    }

    fn write_pack(root: &Path, name: &str, title: &str, language: &str) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).expect("pack dir");
        fs::write(
            dir.join("story.toml"),
            format!("title = \"{title}\"\nlanguage = \"{language}\"\n"),
        )
        .expect("story.toml");
    }

    #[test]
    fn scan_lists_sibling_packs_with_title_and_language() {
        let root = test_dir("scan-basic");
        write_pack(&root, "alpha", "비가 그친 뒤", "ko-KR");
        write_pack(&root, "beta", "Starlight Station", "en-US");

        let entries = super::scan_packs(&root.join("alpha"));

        assert_eq!(entries.len(), 2);
        // Entries are sorted by title: ASCII titles come before Hangul ones.
        assert_eq!(entries[0].title, "Starlight Station");
        assert_eq!(
            entries
                .iter()
                .find(|e| e.title == "비가 그친 뒤")
                .map(|e| e.dir.as_str()),
            Some(root.join("alpha").display().to_string().as_str())
        );
        fs::remove_dir_all(root).expect("clean test directory");
    }

    #[test]
    fn scan_skips_broken_manifests_and_non_pack_dirs() {
        let root = test_dir("scan-broken");
        write_pack(&root, "good", "좋은 팩", "ko-KR");
        let broken = root.join("broken");
        fs::create_dir_all(&broken).expect("dir");
        fs::write(broken.join("story.toml"), "title = [unclosed").expect("bad toml");
        fs::create_dir_all(root.join("not-a-pack")).expect("dir");

        let entries = super::scan_packs(&root.join("good"));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "좋은 팩");
        fs::remove_dir_all(root).expect("clean test directory");
    }

    #[test]
    fn scan_without_sibling_root_returns_only_current_pack() {
        let dir = test_dir("scan-lonely");
        write_pack(&dir, "", "외톨이 팩", "ko-KR");

        let entries = super::scan_packs(&dir);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "외톨이 팩");
        assert_eq!(entries[0].dir, dir.display().to_string());
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn browse_lists_dirs_first_and_filters_files() {
        let root = test_dir("browse");
        fs::create_dir_all(root.join("zeta-dir")).expect("dir");
        fs::create_dir_all(root.join(".hidden-dir")).expect("hidden dir");
        fs::write(root.join("card.CHARX"), b"x").expect("charx");
        fs::write(root.join("notes.txt"), b"x").expect("txt");
        fs::write(root.join(".dot.png"), b"x").expect("hidden card");
        // A symlink must not count as a browsable directory.
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("zeta-dir"), root.join("link-dir")).expect("symlink");

        let entries = super::list_dir_for_browse(&root).expect("list");

        #[cfg(unix)]
        {
            assert!(
                !entries.iter().any(|e| e.name == "link-dir"),
                "symlinks are not browsable dirs: {entries:?}"
            );
        }
        assert_eq!(
            entries,
            vec![
                super::BrowseEntry {
                    name: "zeta-dir".to_string(),
                    is_dir: true,
                },
                super::BrowseEntry {
                    name: "card.CHARX".to_string(),
                    is_dir: false,
                },
            ]
        );
        fs::remove_dir_all(root).expect("clean test directory");
    }

    #[test]
    fn browse_unreadable_dir_reports_instead_of_panicking() {
        let missing = test_dir("browse-missing").join("nope");
        let error = super::list_dir_for_browse(&missing).unwrap_err();
        assert!(error.contains("읽지 못했습니다"), "{error}");
    }

    #[test]
    fn delete_pack_guards_active_root_and_manifest() {
        let root = test_dir("delete");
        write_pack(&root, "alpha", "A", "ko-KR");
        write_pack(&root, "beta", "B", "ko-KR");
        let plain = root.join("plain");
        fs::create_dir_all(&plain).expect("plain dir");

        let error =
            super::delete_pack(&root.join("alpha"), &root, &root.join("alpha")).unwrap_err();
        assert!(error.contains("현재 플레이 중"), "{error}");
        let error =
            super::delete_pack(&root, &root.join("alpha"), &root.join("alpha")).unwrap_err();
        assert!(error.contains("팩 목록의"), "{error}");
        let error = super::delete_pack(&plain, &root, &root.join("alpha")).unwrap_err();
        assert!(error.contains("story.toml"), "{error}");

        super::delete_pack(&root.join("beta"), &root, &root.join("alpha")).expect("delete");
        assert!(!root.join("beta").exists(), "pack removed");
        assert!(root.join("alpha").exists(), "active pack untouched");
        fs::remove_dir_all(root).expect("clean test directory");
    }
}
