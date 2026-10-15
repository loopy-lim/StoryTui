//! Card macro handling: `{{char}}`/`{{user}}` fixed substitution plus
//! `<START>` dialogue-example splitting.
//!
//! Policy (import contract): the two environment macros resolve to fixed
//! strings at import time — `{{char}}` to the card's character name, `{{user}}`
//! to the configured player address. Unknown macros are removed and recorded
//! in the report; the raw card in `sources/` always keeps them.

/// Replace card macros in `input`. Unknown `{{...}}` macros are dropped and
/// their names recorded once each in `ignored_macros`.
pub fn substitute(
    input: &str,
    card_name: &str,
    user_name: &str,
    ignored_macros: &mut Vec<String>,
) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            // Unterminated marker: keep the literal text.
            out.push_str("{{");
            out.push_str(after);
            return out;
        };
        let name = after[..end].trim().to_ascii_lowercase();
        match name.as_str() {
            "char" => out.push_str(card_name),
            "user" => out.push_str(user_name),
            "linebreak" => out.push('\n'),
            other => {
                if !other.is_empty() && !ignored_macros.iter().any(|seen| seen == other) {
                    ignored_macros.push(other.to_string());
                }
            }
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// Split a card `mes_example` into individual example blocks on `<START>`
/// (case-insensitive). Empty blocks are dropped.
pub fn split_dialogue_examples(mes_example: &str) -> Vec<String> {
    let mut examples = Vec::new();
    let mut rest = mes_example;
    loop {
        match rest.to_ascii_lowercase().find("<start>") {
            Some(index) => {
                push_example(&mut examples, &rest[..index]);
                rest = &rest[index + "<START>".len()..];
            }
            None => {
                push_example(&mut examples, rest);
                break;
            }
        }
    }
    examples
}

fn push_example(examples: &mut Vec<String>, block: &str) {
    let trimmed = block.trim();
    if !trimmed.is_empty() {
        examples.push(trimmed.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_and_user_macros_substitute() {
        let mut ignored = Vec::new();
        let out = substitute(
            "{{char}}는 {{user}}를 본다.",
            "유나",
            "플레이어",
            &mut ignored,
        );
        assert_eq!(out, "유나는 플레이어를 본다.");
        assert!(ignored.is_empty());
    }

    #[test]
    fn macros_are_case_insensitive() {
        let mut ignored = Vec::new();
        let out = substitute("{{CHAR}}와 {{User}}", "A", "B", &mut ignored);
        assert_eq!(out, "A와 B");
    }

    #[test]
    fn unknown_macros_are_removed_and_reported_once() {
        let mut ignored = Vec::new();
        let out = substitute("{{random:a,b}} {{time}} {{char}}", "C", "U", &mut ignored);
        assert_eq!(out, "  C", "removed macros leave the surrounding spaces");
        assert_eq!(ignored, vec!["random:a,b".to_string(), "time".to_string()]);
    }

    #[test]
    fn unterminated_marker_is_kept_literal() {
        let mut ignored = Vec::new();
        let out = substitute("안녕 {{char", "C", "U", &mut ignored);
        assert_eq!(out, "안녕 {{char");
    }

    #[test]
    fn dialogue_examples_split_on_start_marker() {
        let examples = split_dialogue_examples(
            "<START>\n{{user}}: 안녕\n{{char}}: 그래.\n<START>\n{{user}}: 또 봐\n{{char}}: 응.",
        );
        assert_eq!(examples.len(), 2);
        assert!(examples[0].starts_with("{{user}}"));
    }
}
