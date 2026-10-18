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
}
