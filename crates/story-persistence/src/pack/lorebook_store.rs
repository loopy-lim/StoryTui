//! Pack lorebook persistence: atomic save of an edited `lorebook.toml`.
//!
//! Editing policy (see docs/plans/2026-10-23-frontend-parity.md): the TUI
//! only persists lorebook edits to packs that ship their lorebook as a
//! `lorebook.toml` file — a pack with no such file keeps session-only edits
//! so an imported pack's layout is never rewritten behind its loader.

use std::path::{Path, PathBuf};

use story_model::Lorebook;

use crate::atomic_write::atomic_write_json;

/// The pack's lorebook file location.
pub fn lorebook_path(pack_root: &Path) -> PathBuf {
    pack_root.join("lorebook.toml")
}

/// Atomically write `book` as the pack's `lorebook.toml`, keeping one `.bak`
/// generation. Refuses packs that have no `lorebook.toml` file — callers
/// surface that as a session-only edit.
pub fn save_lorebook(pack_root: &Path, book: &Lorebook) -> Result<(), String> {
    let path = lorebook_path(pack_root);
    if !path.exists() {
        return Err(format!(
            "이 팩에는 lorebook.toml이 없습니다 — 편집은 이 세션에서만 유지됩니다 ({})",
            path.display()
        ));
    }
    let body =
        toml::to_string_pretty(book).map_err(|e| format!("로어북을 직렬화하지 못했습니다: {e}"))?;
    atomic_write_json(&path, body.as_bytes(), 1)
        .map_err(|e| format!("로어북을 저장하지 못했습니다 ({}): {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{lorebook_path, save_lorebook};
    use story_model::{Lorebook, LorebookEntry, LorebookLogic, LorebookPosition};
    use tempfile::tempdir;

    fn entry(id: &str, keys: &[&str], content: &str) -> LorebookEntry {
        LorebookEntry {
            id: id.to_string(),
            keys: keys.iter().map(|key| key.to_string()).collect(),
            secondary_keys: Vec::new(),
            logic: LorebookLogic::AndAny,
            content: content.to_string(),
            constant: false,
            enabled: true,
            insertion_order: 100,
            position: LorebookPosition::BeforeChar,
            case_sensitive: false,
            probability: 100,
            priority: None,
            comment: None,
            prevent_recursion: false,
            exclude_recursion: false,
            sticky: 0,
            cooldown: 0,
            delay: 0,
            vectorized: false,
        }
    }

    #[test]
    fn save_updates_existing_file_with_bak_and_round_trips() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let existing = Lorebook {
            name: "원본".to_string(),
            ..Lorebook::default()
        };
        std::fs::write(
            lorebook_path(root),
            toml::to_string_pretty(&existing).expect("serialize seed"),
        )
        .expect("seed file");

        let mut edited = existing.clone();
        edited.name = "편집됨".to_string();
        edited
            .entries
            .push(entry("rain", &["비"], "빗소리가 커진다."));
        save_lorebook(root, &edited).expect("save");

        let text = std::fs::read_to_string(lorebook_path(root)).expect("read");
        let reloaded: Lorebook = toml::from_str(&text).expect("parse");
        assert_eq!(reloaded, edited, "the saved file round-trips exactly");
        assert!(lorebook_path(root).with_extension("toml.bak").exists());
    }

    #[test]
    fn save_refuses_packs_without_a_lorebook_file() {
        let dir = tempdir().expect("tempdir");
        let error =
            save_lorebook(dir.path(), &Lorebook::default()).expect_err("missing file must refuse");
        assert!(
            error.contains("세션"),
            "error explains session-only: {error}"
        );
        assert!(!lorebook_path(dir.path()).exists());
    }
}
