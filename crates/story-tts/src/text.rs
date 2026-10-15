//! Narration text cleanup for speech: what reads well is not what renders
//! well. Markdown markers, headers, list/quote prefixes, links and bare URLs
//! are stripped; dialogue and prose pass through untouched.

/// Turn rendered narration into something a TTS voice reads naturally.
pub fn speakable_text(input: &str) -> String {
    input
        .lines()
        .map(strip_line_markers)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Strip the block-level markers one rendered line can carry.
fn strip_line_markers(line: &str) -> String {
    let trimmed = line.trim_start();
    let body = if let Some(rest) = trimmed
        .strip_prefix("### ")
        .or_else(|| trimmed.strip_prefix("## "))
        .or_else(|| trimmed.strip_prefix("# "))
    {
        rest
    } else if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
    {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("> ") {
        rest
    } else if trimmed == "---" {
        ""
    } else {
        trimmed
    };
    strip_inline_markers(body).trim_end().to_string()
}

/// Remove inline markdown (`**`, `__`, `~~`, `` ` ``, `*`), fold `[label](url)`
/// down to its label, and drop bare URLs (a voice reading query strings is
/// worse than a silent gap).
fn strip_inline_markers(body: &str) -> String {
    let unlinked = strip_links(body);
    let mut result = unlinked;
    for marker in ["**", "__", "~~", "`", "*"] {
        result = result.replace(marker, "");
    }
    strip_bare_urls(&result)
}

/// `[label](url)` → `label`. Unmatched brackets are left alone.
fn strip_links(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('[') {
        let after = &rest[start + 1..];
        if let Some(mid) = after.find("](")
            && let Some(end) = after[mid + 2..].find(')')
        {
            result.push_str(&rest[..start]);
            result.push_str(&after[..mid]);
            rest = &after[mid + 2 + end + 1..];
            continue;
        }
        break;
    }
    result.push_str(rest);
    result
}

/// Drop whitespace-delimited `http(s)://…` tokens.
fn strip_bare_urls(input: &str) -> String {
    input
        .split_whitespace()
        .filter(|token| !token.starts_with("http://") && !token.starts_with("https://"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_lists_and_quotes_lose_their_markers() {
        assert_eq!(speakable_text("# 장면"), "장면");
        assert_eq!(speakable_text("### 소제목"), "소제목");
        assert_eq!(speakable_text("- 항목 하나"), "항목 하나");
        assert_eq!(speakable_text("> 인용문"), "인용문");
        assert_eq!(speakable_text("---"), "");
    }

    #[test]
    fn inline_markers_are_removed_but_text_survives() {
        assert_eq!(speakable_text("**굵게** 말했다"), "굵게 말했다");
        assert_eq!(speakable_text("`코드`와 *기울임*"), "코드와 기울임");
    }

    #[test]
    fn links_become_labels_and_bare_urls_disappear() {
        assert_eq!(
            speakable_text("[카페 나무](https://map.example.com) 앞"),
            "카페 나무 앞"
        );
        assert_eq!(
            speakable_text("주소는 https://example.com?a=1 이었다"),
            "주소는 이었다"
        );
    }

    #[test]
    fn dialogue_and_prose_pass_through() {
        let line = "유나: “그건 네 해석이고.”";
        assert_eq!(speakable_text(line), line);
    }

    #[test]
    fn plain_korean_narration_is_unchanged() {
        let body = "비가 그쳤다.\n\n그녀는 창밖을 보았다.";
        assert_eq!(speakable_text(body), body);
    }
}
