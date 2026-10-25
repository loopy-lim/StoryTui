//! `storytui export` — write a save slot out as a portable transcript.
//!
//! Mirrors `import_cmd`: it never enters the TUI. Reads the slot through the
//! same checksummed [`SaveStore`] the game uses and writes `txt` or `jsonl`
//! (see `story_persistence::export`).

use std::path::PathBuf;

use story_persistence::{SaveStore, envelope_to_jsonl, envelope_to_txt};

pub const USAGE: &str =
    "사용법: storytui export <슬롯> [--format txt|jsonl] [--out 경로] [--save-dir 경로]";

/// Parse `args` (everything after the `export` subcommand) and write the
/// transcript. The default output is `<slot>.<format>` in the current
/// directory; the default save directory is `saves` like the base config.
pub fn run(args: &[String]) -> Result<(), String> {
    let mut slot: Option<String> = None;
    let mut format = "txt".to_string();
    let mut out: Option<PathBuf> = None;
    let mut save_dir = PathBuf::from("saves");
    let mut values = args.iter();
    while let Some(arg) = values.next() {
        match arg.as_str() {
            "--format" => {
                format = values
                    .next()
                    .ok_or_else(|| format!("--format 뒤에 txt 또는 jsonl이 필요합니다\n{USAGE}"))?
                    .clone();
            }
            "--out" => {
                out = Some(PathBuf::from(
                    values.next().ok_or("--out 뒤에 경로가 필요합니다")?,
                ));
            }
            "--save-dir" => {
                save_dir = PathBuf::from(values.next().ok_or("--save-dir 뒤에 경로가 필요합니다")?);
            }
            other if other.starts_with("--") => {
                return Err(format!("알 수 없는 옵션: {other}\n{USAGE}"));
            }
            other => {
                if slot.is_some() {
                    return Err(format!("슬롯은 하나만 지정할 수 있습니다\n{USAGE}"));
                }
                slot = Some(other.to_string());
            }
        }
    }
    let slot = slot.ok_or_else(|| format!("내보낼 슬롯을 지정하세요\n{USAGE}"))?;
    if format != "txt" && format != "jsonl" {
        return Err(format!(
            "--format은 txt 또는 jsonl만 지원합니다: {format}\n{USAGE}"
        ));
    }

    let store = SaveStore::open(&save_dir)
        .map_err(|e| format!("세이브 디렉터리를 열지 못했습니다: {e}"))?;
    let envelope = store
        .load(&slot)
        .map_err(|e| format!("슬롯 '{slot}'을 읽지 못했습니다: {e}"))?;
    let (body, extension) = if format == "jsonl" {
        (
            envelope_to_jsonl(&envelope)
                .map_err(|e| format!("내보내기를 직렬화하지 못했습니다: {e}"))?,
            "jsonl",
        )
    } else {
        (envelope_to_txt(&envelope), "txt")
    };
    let path = out.unwrap_or_else(|| PathBuf::from(format!("{slot}.{extension}")));
    std::fs::write(&path, body)
        .map_err(|e| format!("파일을 쓰지 못했습니다 ({}): {e}", path.display()))?;
    println!(
        "{}턴을 {}로 내보냈습니다: {}",
        envelope.payload.history.len(),
        format,
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::run;
    use story_core::tests_support::{sample_game_state, sample_history_entry};
    use story_model::{SaveEnvelope, SavePayload};
    use story_persistence::{CHECKSUM_ALGORITHM, SaveStore};

    fn sample_envelope(slot: &str) -> SaveEnvelope {
        let mut state = sample_game_state();
        state.history = vec![sample_history_entry(1, "첫 장면이 시작된다.")];
        let payload = SavePayload {
            memories: state.memories.clone(),
            history: state.history.clone(),
            stable_state: state,
            audit_tail: Vec::new(),
            undo_stack: Vec::new(),
        };
        SaveEnvelope {
            save_schema_version: 1,
            engine_version: "test".to_string(),
            story_id: "rainy-reunion".to_string(),
            story_version: semver::Version::new(0, 1, 0),
            pack_schema_version: semver::Version::new(1, 0, 0),
            slot_id: slot.to_string(),
            description: None,
            saved_at: story_model::chrono::Utc::now(),
            chapter_label: "1장".to_string(),
            scene_label: "장면".to_string(),
            checksum_algorithm: CHECKSUM_ALGORITHM.to_string(),
            payload_checksum: String::new(),
            payload,
        }
    }

    #[test]
    fn export_writes_txt_by_default_and_jsonl_by_flag() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out_dir = tempfile::tempdir().expect("tempdir");
        let store = SaveStore::open(dir.path()).expect("store");
        store.save(sample_envelope("slot-1")).expect("save");

        // Default output lands next to the cwd, so pin it to the temp out dir.
        let out = out_dir.path().join("from-default.txt");
        run(&[
            "slot-1".to_string(),
            "--save-dir".to_string(),
            dir.path().display().to_string(),
            "--out".to_string(),
            out.display().to_string(),
        ])
        .expect("txt export");
        let text = std::fs::read_to_string(&out).expect("read txt");
        assert!(text.contains("## 턴 1"));
        assert!(text.contains("첫 장면이 시작된다."));

        let jsonl = out_dir.path().join("turns.jsonl");
        run(&[
            "slot-1".to_string(),
            "--format".to_string(),
            "jsonl".to_string(),
            "--save-dir".to_string(),
            dir.path().display().to_string(),
            "--out".to_string(),
            jsonl.display().to_string(),
        ])
        .expect("jsonl export");
        let text = std::fs::read_to_string(&jsonl).expect("read jsonl");
        assert_eq!(text.lines().count(), 2, "header + 1 turn");

        // Bad slots and formats fail with a readable message.
        let error = run(&[
            "missing".to_string(),
            "--save-dir".to_string(),
            dir.path().display().to_string(),
        ])
        .expect_err("missing slot");
        assert!(error.contains("missing"));
        let error = run(&[
            "slot-1".to_string(),
            "--format".to_string(),
            "xml".to_string(),
        ])
        .expect_err("bad format");
        assert!(error.contains("txt 또는 jsonl"));
    }
}
