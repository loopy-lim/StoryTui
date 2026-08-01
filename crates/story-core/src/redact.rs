//! Redaction policy and the no-leak audit (ST-035, Acceptance §20).
//!
//! Pure string operations on diagnostic / log text. No IO, no UI, no HTTP —
//! safe to live in [`crate`] (story-core) under ADR-003: depends only on the
//! std `alloc` string primitives.
//!
//! # The no-leak contract
//!
//! Acceptance §20: "default logs/diagnostics contain NONE of prompt / body /
//! free-input / secret." [`redact`] is the single chokepoint the diagnostic
//! bundle and any log line pass through before being written or printed. It
//! applies three passes:
//!
//! 1. **Known-secret masking** — each literal in `known_secrets` is replaced
//!    with `***`. This is how an API key read from the environment at runtime
//!    is masked even if a downstream error echoes it back.
//! 2. **Secret-marker masking** — for each key in
//!    [`RedactionPolicy::secret_markers`] (e.g. `api_key`, `authorization`,
//!    `bearer`), the *value* following the marker is replaced with `***`.
//!    Recognized value shapes: `key=value`, `key: value`, `"key":"value"`,
//!    and the HTTP header form `Marker value` (e.g. `Bearer xyz`).
//! 3. **Section stripping** — for each name in
//!    [`RedactionPolicy::strip_sections`] (e.g. `system_prompt`, `narration`,
//!    `free_input`), the *content* between `<<<name>>>` and `<<</name>>>` is
//!    replaced with `<redacted>`. The delimiters themselves are preserved so
//!    a reader can see content was elided. An unclosed opening tag is treated
//!    as if the rest of the document were the section body (defensive).
//!
//! The default policy is [`default_policy`]; the no-leak audit lives in
//! `crates/story-core/tests/redact.rs`.

/// The fixed mask token substituted for secret values.
pub const MASK: &str = "***";
/// The fixed placeholder substituted for stripped section content.
pub const SECTION_PLACEHOLDER: &str = "<redacted>";

/// A redaction policy. Use [`default_policy`] for Acceptance §20.
#[derive(Clone, Debug, PartialEq)]
pub struct RedactionPolicy {
    /// Marker keys whose *value* must be masked wherever they appear as
    /// `key=value`, `key: value`, `"key":"value"`, or `Marker value`
    /// (space-separated, e.g. `Bearer xyz`). Case-insensitive on the marker
    /// word; values are masked up to the next whitespace, comma, semicolon,
    /// closing bracket, or end of input.
    pub secret_markers: Vec<String>,
    /// Section names whose *content* must be stripped. Sections are delimited
    /// as `<<<name>...content...<<</name>>>`; the inner content is replaced
    /// with [`SECTION_PLACEHOLDER`] so a diagnostic excerpt never echoes
    /// prompt/body/free-input. Section name matching is case-sensitive — the
    /// diagnostic serializer and the policy share a single source of truth.
    pub strip_sections: Vec<&'static str>,
}

/// Acceptance §20 default policy: default logs/diagnostics contain NONE of
/// prompt / body / free-input / secret. Secret markers cover the common key
/// names (snake_case and HTTP-header variants); strip sections cover the four
/// sensitive content classes.
pub fn default_policy() -> RedactionPolicy {
    RedactionPolicy {
        secret_markers: vec![
            "api_key".to_string(),
            "apikey".to_string(),
            "authorization".to_string(),
            "bearer".to_string(),
            "token".to_string(),
            "secret".to_string(),
            "password".to_string(),
            "passwd".to_string(),
        ],
        strip_sections: vec!["system_prompt", "narration", "free_input", "user_input"],
    }
}

/// Redact `text` per `policy`. `known_secrets` are additional literal strings
/// (e.g. a real API key read from the environment) that must be masked
/// wherever they appear, in addition to the policy's marker-based masking.
///
/// Pure: same inputs ⇒ same output. Allocates only the return string.
pub fn redact(text: &str, policy: &RedactionPolicy, known_secrets: &[&str]) -> String {
    let mut out = text.to_string();
    // 1. Known-secret literals.
    for s in known_secrets {
        if !s.is_empty() {
            out = out.replace(s, MASK);
        }
    }
    // 2. Secret-marker values.
    for marker in &policy.secret_markers {
        out = mask_marker_values(&out, marker);
    }
    // 3. Section content.
    for section in &policy.strip_sections {
        out = strip_section(&out, section);
    }
    out
}

// ---------------------------------------------------------------------------
// secret-marker value masking
// ---------------------------------------------------------------------------

/// Mask the value following each case-insensitive occurrence of `marker` as a
/// word. Recognized value shapes (in order):
/// - `marker=value`, `marker:value` (with optional surrounding whitespace).
/// - `"marker":"value"` / `"marker" : "value"` (JSON).
/// - `marker value` (HTTP header form, e.g. `Bearer xyz`).
fn mask_marker_values(text: &str, marker: &str) -> String {
    if marker.is_empty() {
        return text.to_string();
    }
    let cv: Vec<char> = text.chars().collect();
    let lc: Vec<char> = text.to_lowercase().chars().collect();
    let mv: Vec<char> = marker.to_lowercase().chars().collect();
    let n = cv.len();
    let m = mv.len();
    if m == 0 || m > n {
        return text.to_string();
    }
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i + m <= n {
        if lc[i..i + m] == mv[..]
            && (i == 0 || !is_word_char(lc[i - 1]))
            && (i + m == n || !is_word_char(lc[i + m]))
            && let Some((vs, ve)) = value_span(&cv, i + m)
        {
            spans.push((vs, ve));
            i = ve.max(i + m);
            continue;
        }
        i += 1;
    }
    apply_spans(&cv, &spans)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Given the char position immediately AFTER a marker word, locate the value
/// span to be masked. Returns `None` when no recognizable value pattern
/// follows.
fn value_span(cv: &[char], after_marker: usize) -> Option<(usize, usize)> {
    let n = cv.len();
    let mut j = after_marker;
    let mut saw_ws = false;
    while j < n && cv[j].is_whitespace() {
        j += 1;
        saw_ws = true;
    }
    if j >= n {
        return None;
    }
    // Separator form: `=value` or `:value`.
    if cv[j] == '=' || cv[j] == ':' {
        j += 1;
        while j < n && cv[j].is_whitespace() {
            j += 1;
        }
        if j >= n {
            return None;
        }
        return Some(value_token_span(cv, j));
    }
    // HTTP-header / space-separated form (e.g. `Bearer xyz`). Only fire when
    // whitespace actually separated the marker from the value — otherwise
    // `api_key` running directly into other text would over-mask.
    if saw_ws {
        return Some(value_token_span(cv, j));
    }
    None
}

/// Locate a single value token starting at `start`:
/// - Quoted (`"..."` or `'...'`) → the inner content (quotes preserved
///   outside the masked span).
/// - Bare → up to the next whitespace, `,`, `;`, `}`, `]`, `)`, or end.
fn value_token_span(cv: &[char], start: usize) -> (usize, usize) {
    let n = cv.len();
    if start >= n {
        return (start, start);
    }
    let c = cv[start];
    if c == '"' || c == '\'' {
        let quote = c;
        let mut j = start + 1;
        while j < n && cv[j] != quote {
            j += 1;
        }
        return (start + 1, j.min(n));
    }
    let mut j = start;
    while j < n {
        let cc = cv[j];
        if cc.is_whitespace() || cc == ',' || cc == ';' || cc == '}' || cc == ']' || cc == ')' {
            break;
        }
        j += 1;
    }
    (start, j)
}

/// Rebuild the string from `cv`, replacing each `(start, end)` char-index
/// span with [`MASK`]. Spans must be sorted ascending by `start` and
/// non-overlapping; this is guaranteed by `mask_marker_values`'s left-to-right
/// scan.
fn apply_spans(cv: &[char], spans: &[(usize, usize)]) -> String {
    if spans.is_empty() {
        return cv.iter().collect();
    }
    let mut out = String::with_capacity(cv.len());
    let mut i = 0;
    let mut s = 0;
    while i < cv.len() {
        if s < spans.len() && i == spans[s].0 {
            out.push_str(MASK);
            i = spans[s].1.max(i);
            s += 1;
        } else {
            out.push(cv[i]);
            i += 1;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// section stripping
// ---------------------------------------------------------------------------

/// Replace the inner content of every `<<<section>>>...<<</section>>>` block
/// with [`SECTION_PLACEHOLDER`]. Delimiters are preserved. An unclosed opening
/// tag consumes the rest of the document (defensive: never leak partial
/// content).
fn strip_section(text: &str, section: &str) -> String {
    let open = format!("<<<{section}>>>");
    let close = format!("<<</{section}>>>");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(o) = rest.find(&open) {
        out.push_str(&rest[..o]);
        out.push_str(&open);
        let after_open = &rest[o + open.len()..];
        match after_open.find(&close) {
            Some(c) => {
                out.push_str(SECTION_PLACEHOLDER);
                out.push_str(&close);
                rest = &after_open[c + close.len()..];
            }
            None => {
                // Unclosed — elide everything that follows.
                out.push_str(SECTION_PLACEHOLDER);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_simple_eq_value() {
        assert_eq!(
            mask_marker_values("api_key=hello", "api_key"),
            "api_key=***"
        );
    }

    #[test]
    fn mask_colon_with_space() {
        assert_eq!(
            mask_marker_values("api_key: hello", "api_key"),
            "api_key: ***"
        );
    }

    #[test]
    fn mask_json_quoted_value() {
        assert_eq!(
            mask_marker_values("\"api_key\":\"hello\"", "api_key"),
            "\"api_key\":\"***\""
        );
    }

    #[test]
    fn mask_bearer_token() {
        assert_eq!(
            mask_marker_values("Authorization: Bearer abc123", "authorization"),
            "Authorization: ***"
        );
        assert_eq!(mask_marker_values("Bearer abc123", "bearer"), "Bearer ***");
    }

    #[test]
    fn mask_does_not_match_substring_of_word() {
        // "api_key" should not fire inside "my_api_keyboard".
        let out = mask_marker_values("my_api_keyboard=xyz", "api_key");
        assert_eq!(out, "my_api_keyboard=xyz");
    }

    #[test]
    fn strip_section_basic() {
        let t = "<<<narration>>>hello world<<</narration>>>";
        assert_eq!(
            strip_section(t, "narration"),
            "<<<narration>>><redacted><<</narration>>>"
        );
    }
}
