//! Save-slot export: turn a saved envelope into portable transcripts.
//!
//! The export only reads what a save already records — it never re-derives
//! story content. `txt` is a readable transcript; `jsonl` keeps the full
//! per-turn [`HistoryEntry`] payloads, one JSON object per line, with a
//! `storytui-export` metadata header line first (line-delimited JSON in the
//! SillyTavern chat-export spirit).

use story_model::SaveEnvelope;

/// Plain-text transcript: a title line, then one `## 턴 N` block per
/// history entry (player action line when the turn recorded one).
pub fn envelope_to_txt(envelope: &SaveEnvelope) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# StoryTUI 내보내기 — {} {} (슬롯: {})\n",
        envelope.story_id, envelope.story_version, envelope.slot_id
    ));
    out.push_str(&format!("저장 시각: {}\n", envelope.saved_at.to_rfc3339()));
    for entry in &envelope.payload.history {
        out.push_str(&format!("\n## 턴 {} · {}\n", entry.turn, entry.scene_id));
        if let Some(action) = &entry.player_action {
            let text = action.original_text.trim();
            if !text.is_empty() {
                out.push_str(&format!("[플레이어] {text}\n"));
            }
        }
        out.push_str(entry.narration.trim_end());
        out.push('\n');
    }
    out
}

/// JSONL transcript: metadata header line, then one `turn` line per history
/// entry carrying the full entry payload.
pub fn envelope_to_jsonl(envelope: &SaveEnvelope) -> Result<String, serde_json::Error> {
    let header = serde_json::json!({
        "type": "storytui-export",
        "story_id": envelope.story_id,
        "story_version": envelope.story_version.to_string(),
        "slot_id": envelope.slot_id,
        "saved_at": envelope.saved_at.to_rfc3339(),
        "turns": envelope.payload.history.len(),
    });
    let mut lines = vec![header.to_string()];
    for entry in &envelope.payload.history {
        let line = serde_json::json!({ "type": "turn", "turn": entry });
        lines.push(line.to_string());
    }
    let mut out = lines.join("\n");
    out.push('\n');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{envelope_to_jsonl, envelope_to_txt};
    use crate::CHECKSUM_ALGORITHM;
    use story_core::tests_support::{sample_game_state, sample_history_entry};
    use story_model::{ActionType, ParsedAction, SaveEnvelope, SavePayload};

    fn sample_envelope() -> SaveEnvelope {
        let mut state = sample_game_state();
        let mut first = sample_history_entry(1, "첫 장면이 시작된다.");
        first.player_action = Some(ParsedAction {
            action_type: ActionType::Action,
            target: None,
            intent: None,
            tone: None,
            content: "조심스럽게 문을 연다.".to_string(),
            original_text: " 조심스럽게 문을 연다. ".to_string(),
            confidence: None,
        });
        state.history = vec![first, sample_history_entry(2, "두 번째 장면.")];
        let payload = SavePayload {
            memories: state.memories.clone(),
            history: state.history.clone(),
            stable_state: state,
            undo_stack: Vec::new(),
            audit_tail: Vec::new(),
        };
        SaveEnvelope {
            save_schema_version: 1,
            engine_version: "test".to_string(),
            story_id: "rainy-reunion".to_string(),
            story_version: semver::Version::new(0, 1, 0),
            pack_schema_version: semver::Version::new(1, 0, 0),
            slot_id: "slot-1".to_string(),
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
    fn txt_export_lists_turns_with_player_actions() {
        let text = envelope_to_txt(&sample_envelope());
        assert!(text.starts_with("# StoryTUI 내보내기 — rainy-reunion"));
        assert!(text.contains("## 턴 1 · scene-1"));
        assert!(
            text.contains("[플레이어] 조심스럽게 문을 연다."),
            "raw text is trimmed"
        );
        assert!(text.contains("첫 장면이 시작된다."));
        assert!(text.contains("## 턴 2 · scene-1"));
        assert!(text.contains("두 번째 장면."));
    }

    #[test]
    fn jsonl_export_has_header_and_full_turn_payloads() {
        let text = envelope_to_jsonl(&sample_envelope()).expect("serialize");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "header + 2 turns");

        let header: serde_json::Value = serde_json::from_str(lines[0]).expect("header json");
        assert_eq!(header["type"], "storytui-export");
        assert_eq!(header["story_id"], "rainy-reunion");
        assert_eq!(header["turns"], 2);

        let turn: serde_json::Value = serde_json::from_str(lines[1]).expect("turn json");
        assert_eq!(turn["type"], "turn");
        assert_eq!(turn["turn"]["narration"], "첫 장면이 시작된다.");
        assert_eq!(
            turn["turn"]["player_action"]["original_text"],
            " 조심스럽게 문을 연다. "
        );
    }
}
