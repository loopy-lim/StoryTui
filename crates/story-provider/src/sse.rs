//! UTF-8 boundary-safe Server-Sent Events byte parser (ST-034, W10 Part C).
//!
//! [`SseParser`] consumes an OpenAI-style streaming chat-completion body one
//! byte chunk at a time and yields complete SSE events as soon as a blank-line
//! separator is observed. It is **UTF-8 boundary-safe**: a chunk that ends
//! mid-multi-byte-character is buffered until the rest arrives, so a Korean
//! lead byte never surfaces as a replacement char or a panic.
//!
//! # Events
//!
//! Each SSE event block (separated by a blank line — `\n\n`, `\r\n\r\n`, or
//! `\r\r`) is decoded into one [`SseEvent::Data`] per `data:` line's payload.
//! The literal `[DONE]` sentinel (OpenAI's stream terminator) yields
//! [`SseEvent::Done`]. [`SseEvent::Incomplete`] is emitted at most once per
//! [`SseParser::feed`] call when bytes remain buffered but no complete event
//! could be parsed yet — the consumer ignores it.
//!
//! # Multi-byte safety
//!
//! The parser never splits a UTF-8 character. After each `feed()`, the buffer
//! is split into a UTF-8-valid prefix (consumed as far as complete events go)
//! and a trailing run of incomplete UTF-8 bytes (retained). The standard
//! library's `std::str::from_utf8` gives us the valid-prefix length directly.
//!
//! ```
//! use story_provider::sse::{SseEvent, SseParser};
//! let mut p = SseParser::new();
//! // '비' is UTF-8 0xEB 0xB9 0x84. Split across two chunks:
//! let ev1 = p.feed(b"data: \xEB\xB9");
//! let ev2 = p.feed(b"\x84\n\n");
//! // ev1 produces no Data event (the char is mid-stream).
//! let texts: Vec<String> = ev1.into_iter().filter_map(|e| match e {
//!     SseEvent::Data(s) => Some(s), _ => None }).collect();
//! assert!(texts.is_empty(), "no Data yet: {texts:?}");
//! // ev2 completes the event and yields the full payload, intact.
//! let got: Vec<String> = ev2.into_iter().filter_map(|e| match e {
//!     SseEvent::Data(s) => Some(s), _ => None }).collect();
//! assert_eq!(got, vec!["비".to_string()]);
//! ```

/// A parsed Server-Sent Event from an OpenAI streaming body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SseEvent {
    /// A decoded SSE `data:` payload (UTF-8 whole; never a partial multi-byte
    /// split). Multi-line data blocks are joined with `\n`.
    Data(String),
    /// The `[DONE]` sentinel — the stream terminator. The parser stops after
    /// emitting this; further `feed()` calls are no-ops.
    Done,
    /// Bytes are buffered but no complete event could be parsed yet. Consumers
    /// ignore this variant; it exists for symmetry / diagnostics.
    Incomplete,
}

/// A streaming SSE byte parser. Owns a buffer of incomplete bytes; call
/// [`SseParser::feed`] for every chunk the HTTP body yields.
#[derive(Debug, Default)]
pub struct SseParser {
    buffer: Vec<u8>,
    done: bool,
}

impl SseParser {
    /// New empty parser.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one chunk of bytes. Returns the events completed by this chunk in
    /// order. UTF-8 boundary-safe: an incomplete trailing multi-byte sequence
    /// is buffered internally and surfaced once the rest arrives.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        if self.done {
            return Vec::new();
        }
        if chunk.is_empty() {
            return Vec::new();
        }
        self.buffer.extend_from_slice(chunk);
        self.parse_complete_events()
    }

    /// `true` once a `[DONE]` sentinel has been parsed (further feeds no-op).
    pub fn is_done(&self) -> bool {
        self.done
    }

    fn parse_complete_events(&mut self) -> Vec<SseEvent> {
        // Step 1: UTF-8 boundary — find the longest valid-UTF-8 prefix. Bytes
        // after `valid_len` are an incomplete multi-byte sequence (held back).
        let valid_len = match std::str::from_utf8(&self.buffer) {
            Ok(s) => s.len(),
            Err(e) => {
                let valid = e.valid_up_to();
                match e.error_len() {
                    None => valid, // incomplete trailing bytes — hold back
                    Some(_) => {
                        // Actually invalid UTF-8 from the server. Mark done to
                        // stop the stream; the host surfaces an error.
                        self.done = true;
                        self.buffer.clear();
                        return Vec::new();
                    }
                }
            }
        };

        // SAFETY: from_utf8 confirmed the first valid_len bytes are valid.
        let valid: &str = unsafe { std::str::from_utf8_unchecked(&self.buffer[..valid_len]) };

        let mut events = Vec::new();
        let mut consumed = 0usize; // byte offset into `valid` already processed
        let mut cursor = 0usize; // scanning cursor

        while cursor < valid.len() {
            // Find the next blank-line separator at or after `cursor`.
            let Some((sep_at, sep_len)) = find_blank_line(valid, cursor) else {
                break; // no complete event left
            };
            let block = &valid[consumed..sep_at];
            cursor = sep_at + sep_len;
            consumed = cursor;
            parse_block(block, &mut events, &mut self.done);
            if self.done {
                break;
            }
        }

        // Retain unconsumed complete-event tail + any held-back UTF-8 bytes.
        let trailing_valid = &self.buffer[consumed..valid_len];
        let trailing_invalid = &self.buffer[valid_len..];
        let mut remaining = Vec::with_capacity(trailing_valid.len() + trailing_invalid.len());
        remaining.extend_from_slice(trailing_valid);
        remaining.extend_from_slice(trailing_invalid);
        self.buffer = remaining;

        // Signal incompleteness once if we have buffered bytes but no events.
        if events.is_empty() && !self.buffer.is_empty() {
            events.push(SseEvent::Incomplete);
        }

        events
    }
}

/// Locate the next blank-line separator in `s` at or after `from`. Returns
/// `(byte_offset_of_separator, separator_len)` where the separator is `\n\n`,
/// `\r\n\r\n`, or `\r\r`. The byte offset points at the first byte of the
/// separator run.
fn find_blank_line(s: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = s.as_bytes();
    let mut i = from;
    while i + 1 < bytes.len() {
        let pair = (bytes[i], bytes[i + 1]);
        if pair == (b'\n', b'\n') {
            return Some((i, 2));
        }
        if pair == (b'\r', b'\r') {
            return Some((i, 2));
        }
        // `\r\n\r\n`: detect at the first `\r` (4-byte separator).
        if i + 3 < bytes.len()
            && bytes[i] == b'\r'
            && bytes[i + 1] == b'\n'
            && bytes[i + 2] == b'\r'
            && bytes[i + 3] == b'\n'
        {
            return Some((i, 4));
        }
        i += 1;
    }
    None
}

/// Parse one SSE event block (the text between two blank-line separators) into
/// zero or more [`SseEvent`]s. Sets `done = true` when the `[DONE]` sentinel
/// appears.
fn parse_block(block: &str, events: &mut Vec<SseEvent>, done: &mut bool) {
    // An event block consists of one or more `field: value` lines; the payload
    // is the concatenation of all `data:` lines joined with `\n`. Per the SSE
    // spec we ignore unknown fields and comments (`:`).
    let mut data_parts: Vec<&str> = Vec::new();
    for line in block.split(['\n', '\r']) {
        if line.is_empty() {
            continue;
        }
        // Some servers pad a single leading space after the colon; the spec
        // strips exactly one.
        if let Some(rest) = line.strip_prefix("data:") {
            let value = rest.strip_prefix(' ').unwrap_or(rest);
            data_parts.push(value);
        }
        // Other fields (event:, id:, retry:) and `:` comments are ignored.
    }
    if data_parts.is_empty() {
        return;
    }
    let joined = data_parts.join("\n");
    if joined == "[DONE]" {
        events.push(SseEvent::Done);
        *done = true;
    } else {
        events.push(SseEvent::Data(joined));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_events(events: Vec<SseEvent>) -> Vec<String> {
        events
            .into_iter()
            .filter_map(|e| match e {
                SseEvent::Data(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn single_data_event_in_one_chunk() {
        let mut p = SseParser::new();
        let ev = p.feed(b"data: hello\n\n");
        assert_eq!(data_events(ev), vec!["hello".to_string()]);
        assert!(p.buffer.is_empty());
    }

    #[test]
    fn done_sentinel_terminates_and_blocks_further_feeds() {
        let mut p = SseParser::new();
        let ev = p.feed(b"data: [DONE]\n\n");
        assert!(ev.contains(&SseEvent::Done));
        assert!(p.is_done());
        // Further feed is a no-op.
        let after = p.feed(b"data: more\n\n");
        assert!(after.is_empty(), "{after:?}");
    }

    #[test]
    fn split_korean_multibyte_char_across_chunks_does_not_panic_or_split_char() {
        // '비' is UTF-8 EB B9 84. Split after the first two bytes.
        let mut p = SseParser::new();
        let ev1 = p.feed(b"data: \xEB\xB9");
        // No Data event yet — the char is incomplete.
        assert!(data_events(ev1).is_empty(), "must not surface partial char");
        let ev2 = p.feed(b"\x84\n\n");
        assert_eq!(data_events(ev2), vec!["비".to_string()]);
    }

    #[test]
    fn split_between_data_and_newline_is_held() {
        let mut p = SseParser::new();
        let ev1 = p.feed(b"data: hello");
        assert!(data_events(ev1).is_empty());
        let ev2 = p.feed(b"\n\n");
        assert_eq!(data_events(ev2), vec!["hello".to_string()]);
    }

    #[test]
    fn multiple_events_in_one_chunk() {
        let mut p = SseParser::new();
        let ev = p.feed(b"data: a\n\ndata: b\n\ndata: c\n\n");
        assert_eq!(
            data_events(ev),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn multiline_data_block_is_joined_with_newline() {
        let mut p = SseParser::new();
        // Per SSE spec, two `data:` lines form one event with `\n`.
        let ev = p.feed(b"data: line1\ndata: line2\n\n");
        assert_eq!(data_events(ev), vec!["line1\nline2".to_string()]);
    }

    #[test]
    fn blank_line_variants_all_recognized() {
        let mut p = SseParser::new();
        // \r\n\r\n separator.
        let ev = p.feed(b"data: x\r\n\r\n");
        assert_eq!(data_events(ev), vec!["x".to_string()]);
        let mut p = SseParser::new();
        // \r\r separator.
        let ev = p.feed(b"data: y\r\r");
        assert_eq!(data_events(ev), vec!["y".to_string()]);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let mut p = SseParser::new();
        let ev = p.feed(b"id: 1\nevent: chunk\ndata: z\n\n");
        assert_eq!(data_events(ev), vec!["z".to_string()]);
    }

    #[test]
    fn empty_chunk_yields_nothing() {
        let mut p = SseParser::new();
        assert!(p.feed(b"").is_empty());
    }

    #[test]
    fn invalid_utf8_marks_done_and_clears() {
        // Lone 0xFF is invalid UTF-8 (not just incomplete).
        let mut p = SseParser::new();
        let _ = p.feed(b"data: ok\n\n");
        let ev = p.feed(b"\xFF");
        assert!(p.is_done());
        // The valid prior event was already consumed; the invalid feed yields
        // nothing and stops the parser.
        assert!(ev.is_empty(), "{ev:?}");
    }

    #[test]
    fn real_openai_shaped_stream_parses_correctly() {
        let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n\
                    data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n\
                    data: [DONE]\n\n";
        let mut p = SseParser::new();
        let evs = p.feed(body);
        let texts = data_events(evs.clone());
        assert_eq!(
            texts,
            vec![
                "{\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}".to_string(),
                "{\"choices\":[{\"delta\":{\"content\":\" world\"}}]}".to_string(),
            ]
        );
        assert!(evs.iter().any(|e| matches!(e, SseEvent::Done)));
    }
}
