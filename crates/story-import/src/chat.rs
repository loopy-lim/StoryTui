//! SillyTavern chat-log parsing: a chat export (JSONL, or a JSON object with
//! a `chat` array) becomes ordered character turns with the player action
//! that preceded each one.
//!
//! Mapping contract: a character message is one story turn; user messages
//! attach to the NEXT character turn as its player action (multiple user
//! messages join with newlines). Swipes are already resolved by ST into `mes`.

use crate::report::ImportReport;

/// One character turn with the player action that preceded it.
#[derive(Clone, Debug)]
pub struct ImportedChatTurn {
    /// The character's message (one history entry's narration).
    pub narration: String,
    /// The user message(s) immediately before it, joined with newlines.
    pub player_action: Option<String>,
}

/// Parse a chat export. Report notes record skipped metadata lines and any
/// trailing user message that has no character reply.
pub fn parse_chat(text: &str, report: &mut ImportReport) -> Result<Vec<ImportedChatTurn>, String> {
    let messages = parse_messages(text)?;
    let mut pending_action: Option<String> = None;
    let mut turns = Vec::new();
    let mut skipped = 0_usize;
    for message in &messages {
        let Some(mes) = message.get("mes").and_then(serde_json::Value::as_str) else {
            // The first line of an ST export is chat metadata, not a message.
            skipped += 1;
            continue;
        };
        let mes = mes.trim();
        if mes.is_empty() {
            skipped += 1;
            continue;
        }
        let is_user = message
            .get("is_user")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if is_user {
            match pending_action.as_mut() {
                Some(existing) => {
                    existing.push('\n');
                    existing.push_str(mes);
                }
                None => pending_action = Some(mes.to_string()),
            }
            continue;
        }
        turns.push(ImportedChatTurn {
            narration: mes.to_string(),
            player_action: pending_action.take(),
        });
    }
    if pending_action.is_some() {
        report.note_approximation("마지막 플레이어 메시지는 응답이 없어 생략했습니다");
    }
    if skipped > 0 {
        report.note_approximation(&format!(
            "메시지가 아닌 줄 {skipped}개 생략 (채팅 메타데이터 등)"
        ));
    }
    if turns.is_empty() {
        return Err("채팅 파일에서 캐릭터 메시지를 찾지 못했습니다".to_string());
    }
    Ok(turns)
}

/// Accept both shapes: a JSON object with a `chat` array, or JSONL lines.
fn parse_messages(text: &str) -> Result<Vec<serde_json::Value>, String> {
    let trimmed = text.trim();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(chat) = value.get("chat") {
            return serde_json::from_value(chat.clone())
                .map_err(|e| format!("chat 배열을 해석하지 못했습니다: {e}"));
        }
        if value.get("mes").is_some() {
            return Ok(vec![value]);
        }
        return Err("이 파일은 채팅 내보내기가 아닙니다 (mes/chat 없음)".to_string());
    }
    let mut out = Vec::new();
    for (index, line) in trimmed.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(value) => out.push(value),
            Err(error) => return Err(format!("{}행을 해석하지 못했습니다: {error}", index + 1)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> ImportReport {
        ImportReport::new("테스트", "chat")
    }

    #[test]
    fn jsonl_turns_pair_user_messages_with_the_next_reply() {
        let mut report = report();
        let text = concat!(
            r#"{"user_name":"You","character_name":"미라","chat_metadata":{}}"#,
            "\n",
            r#"{"name":"You","is_user":true,"mes":"다시 왔어"}"#,
            "\n",
            r#"{"name":"You","is_user":true,"mes":"오래 됐지"}"#,
            "\n",
            r#"{"name":"미라","is_user":false,"mes":"그래, 3년이나 지났지."}"#,
            "\n",
            r#"{"name":"미라","is_user":false,"mes":"앉아요."}"#,
        );
        let turns = parse_chat(text, &mut report).expect("parse");
        assert_eq!(turns.len(), 2);
        assert_eq!(
            turns[0].player_action.as_deref(),
            Some("다시 왔어\n오래 됐지"),
            "consecutive user messages join"
        );
        assert_eq!(turns[0].narration, "그래, 3년이나 지났지.");
        assert_eq!(turns[1].player_action, None);
        assert!(turns[1].narration == "앉아요.");
    }

    #[test]
    fn wrapped_chat_object_is_accepted() {
        let mut report = report();
        let text = r#"{"user_name":"You","chat":[
            {"name":"You","is_user":true,"mes":"안녕"},
            {"name":"미라","is_user":false,"mes":"어서 와"}
        ]}"#;
        let turns = parse_chat(text, &mut report).expect("parse");
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].narration, "어서 와");
    }

    #[test]
    fn trailing_user_message_is_reported_and_dropped() {
        let mut report = report();
        let text = concat!(
            r#"{"name":"미라","is_user":false,"mes":"인사"}"#,
            "\n",
            r#"{"name":"You","is_user":true,"mes":"답장 없는 말"}"#,
        );
        let turns = parse_chat(text, &mut report).expect("parse");
        assert_eq!(turns.len(), 1);
        assert!(
            report
                .approximations
                .iter()
                .any(|note| note.contains("마지막 플레이어 메시지")),
            "{:?}",
            report.approximations
        );
    }

    #[test]
    fn a_file_without_character_messages_is_an_error() {
        let mut report = report();
        // A card-like JSON with neither `mes` nor `chat` gets guidance, not silence.
        let error = parse_chat(r#"{"user_name":"You"}"#, &mut report).unwrap_err();
        assert!(error.contains("채팅 내보내기가 아닙니다"), "{error}");

        // A JSONL file whose lines carry no character message at all.
        let error = parse_chat(
            r#"{"name":"You","is_user":true,"mes":"혼자 말만 함"}"#,
            &mut report,
        )
        .unwrap_err();
        assert!(error.contains("캐릭터 메시지"), "{error}");
    }
}
