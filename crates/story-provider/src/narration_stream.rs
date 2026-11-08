//! Incremental narration extraction from a streaming turn response (issue #2).
//!
//! The MVP single-call path asks the model for one JSON object
//! (`narration` + `choices` + `state_patch` + `summary`), so a raw SSE delta
//! stream is JSON tokens, not prose. This extractor is a small, forgiving
//! state machine that recognizes the first `"narration": "…"` string value in
//! the stream and re-emits only its decoded text as it arrives, so the TUI
//! can render partial narration while the rest of the object is still
//! generating.
//!
//! Scope guarantees:
//!
//! - The extractor is display-only. The committed turn ALWAYS comes from the
//!   complete text through [`crate::recovery::parse_generated_turn`], so a
//!   mis-extracted partial (e.g. a doubly-encoded response, or the key token
//!   appearing inside another string) is corrected the moment the final view
//!   replaces `streaming_narration`.
//! - Feeds are `&str` (the SSE parser already buffers split UTF-8), but JSON
//!   `\uXXXX` escapes — including surrogate pairs split across feeds — are
//!   decoded here.
//! - When no `"narration"` string is found (choice-only refreshes, foreign
//!   shapes, thinking prose before the object), nothing is emitted; the
//!   loading overlay remains the only progress signal.

/// Decodes the `"narration"` string value out of a growing JSON text.
#[derive(Debug, Default)]
pub struct NarrationStreamExtractor {
    state: State,
    /// Search buffer while looking for the `"narration"` key. Only the last
    /// `KEY.len() - 1` chars are retained between feeds so a huge
    /// thinking/prelude block cannot grow it unboundedly.
    scan_buf: String,
    /// Scratch for the 4 hex digits of a `\uXXXX` escape in flight.
    hex_buf: [u8; 4],
    hex_len: usize,
    /// Buffered high surrogate waiting for its low half (emitting it eagerly
    /// would corrupt emoji whose pair lands in a later feed).
    pending_high_surrogate: Option<u16>,
}

const KEY: &str = "\"narration\"";

#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
enum State {
    /// Looking for the `"narration"` key token.
    #[default]
    Scanning,
    /// Saw the key; skipping whitespace before the `:`.
    AfterKey,
    /// Saw the `:`; skipping whitespace before the opening `"`.
    AfterColon,
    /// Inside the value string.
    InString,
    /// Saw `\` inside the string.
    PendingEscape,
    /// Collecting the 4 hex digits of a `\uXXXX` escape.
    PendingUnicode,
    /// Saw the closing `"`; done — ignore the remainder of the stream.
    Done,
}

impl NarrationStreamExtractor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one delta of the accumulating JSON text; returns the newly
    /// decoded narration text (possibly empty).
    pub fn feed(&mut self, delta: &str) -> String {
        let mut out = String::new();
        for ch in delta.chars() {
            self.step(&mut out, ch);
        }
        out
    }

    /// Process one character according to the current state.
    fn step(&mut self, out: &mut String, ch: char) {
        match self.state {
            State::Scanning => {
                self.scan_buf.push(ch);
                if let Some(at) = self.scan_buf.find(KEY) {
                    let rest: String = self.scan_buf[at + KEY.len()..].chars().collect();
                    self.scan_buf.clear();
                    self.state = State::AfterKey;
                    for c in rest.chars() {
                        self.step(out, c);
                    }
                } else {
                    self.trim_scan_buf();
                }
            }
            State::AfterKey => match ch {
                ' ' | '\t' | '\n' | '\r' => {}
                ':' => self.state = State::AfterColon,
                // Not a key/value pair after all (text that merely contains
                // the key token). Resume scanning from this char so a real
                // key starting here is not lost.
                _ => self.resume_scanning(out, Some(ch)),
            },
            State::AfterColon => match ch {
                ' ' | '\t' | '\n' | '\r' => {}
                '"' => self.state = State::InString,
                // `"narration": 123` / `null` — not a string value. Resume
                // scanning; a later genuine string occurrence still matches.
                _ => self.resume_scanning(out, Some(ch)),
            },
            State::InString => match ch {
                '"' => {
                    self.state = State::Done;
                    // A value ending on a stranded high surrogate (stream
                    // cut mid-emoji) flushes U+FFFD immediately so the
                    // partial display tracks the raw stream even when no
                    // character follows the closing quote.
                    if self.pending_high_surrogate.take().is_some() {
                        out.push(char::REPLACEMENT_CHARACTER);
                    }
                }
                '\\' => self.state = State::PendingEscape,
                _ => self.emit(out, ch),
            },
            State::PendingEscape => match ch {
                '"' => self.emit_then_string(out, '"'),
                '\\' => self.emit_then_string(out, '\\'),
                '/' => self.emit_then_string(out, '/'),
                'b' => self.emit_then_string(out, '\u{0008}'),
                'f' => self.emit_then_string(out, '\u{000C}'),
                'n' => self.emit_then_string(out, '\n'),
                'r' => self.emit_then_string(out, '\r'),
                't' => self.emit_then_string(out, '\t'),
                'u' => {
                    self.state = State::PendingUnicode;
                    self.hex_buf = [0; 4];
                    self.hex_len = 0;
                }
                // Invalid escape: emit both characters verbatim so the
                // partial display at least tracks the raw stream.
                other => {
                    self.emit(out, '\\');
                    self.emit(out, other);
                    self.state = State::InString;
                }
            },
            State::PendingUnicode => {
                if ch.is_ascii_hexdigit() && self.hex_len < 4 {
                    self.hex_buf[self.hex_len] = ch as u8;
                    self.hex_len += 1;
                    if self.hex_len == 4 {
                        self.state = State::InString;
                        let code = u16::from_str_radix(
                            std::str::from_utf8(&self.hex_buf[..4]).unwrap_or("0000"),
                            16,
                        )
                        .unwrap_or(0);
                        self.emit_unicode(out, code);
                    }
                } else {
                    // Malformed escape — replay the full prefix (including
                    // any hex digits already buffered) and treat this char
                    // as ordinary string content.
                    self.state = State::InString;
                    self.emit(out, '\\');
                    self.emit(out, 'u');
                    for i in 0..self.hex_len {
                        let digit = self.hex_buf[i] as char;
                        self.emit(out, digit);
                    }
                    self.emit(out, ch);
                }
            }
            State::Done => {}
        }
    }

    /// Emit one ordinary character, flushing a stranded high surrogate first.
    fn emit(&mut self, out: &mut String, ch: char) {
        if self.pending_high_surrogate.take().is_some() {
            out.push(char::REPLACEMENT_CHARACTER);
        }
        out.push(ch);
    }

    /// Handle a completed `\uXXXX`.
    fn emit_unicode(&mut self, out: &mut String, code: u16) {
        match code {
            0xD800..=0xDBFF => {
                // High surrogate: hold until the low half lands. If it never
                // does, the next ordinary emission flushes U+FFFD.
                if self.pending_high_surrogate.take().is_some() {
                    out.push(char::REPLACEMENT_CHARACTER);
                }
                self.pending_high_surrogate = Some(code);
            }
            0xDC00..=0xDFFF => {
                if let Some(high) = self.pending_high_surrogate.take() {
                    let c =
                        0x10000 + ((u32::from(high) - 0xD800) << 10) + (u32::from(code) - 0xDC00);
                    if let Some(emoji) = char::from_u32(c) {
                        out.push(emoji);
                        return;
                    }
                }
                out.push(char::REPLACEMENT_CHARACTER);
            }
            _ => {
                if self.pending_high_surrogate.take().is_some() {
                    out.push(char::REPLACEMENT_CHARACTER);
                }
                out.push(char::from_u32(u32::from(code)).unwrap_or(char::REPLACEMENT_CHARACTER));
            }
        }
    }

    /// Emit a simple escape's character and return to the string body.
    fn emit_then_string(&mut self, out: &mut String, ch: char) {
        self.emit(out, ch);
        self.state = State::InString;
    }

    /// Drop back to scanning, optionally re-seeding the scan buffer with the
    /// offending char (it could be the `"` of a genuine key token).
    fn resume_scanning(&mut self, out: &mut String, ch: Option<char>) {
        self.state = State::Scanning;
        self.scan_buf.clear();
        if let Some(c) = ch {
            self.step(out, c);
        }
    }

    /// Retain enough of the tail that a key token split across feeds is
    /// still found on the next match attempt. Drains in place — a long
    /// prelude must not cost a fresh allocation per character.
    fn trim_scan_buf(&mut self) {
        let keep = KEY.len() - 1;
        let excess = self.scan_buf.chars().count().saturating_sub(keep);
        if excess > 0 {
            let byte_at = self
                .scan_buf
                .char_indices()
                .nth(excess)
                .map(|(i, _)| i)
                .expect("excess < char count guarantees this index exists");
            self.scan_buf.drain(..byte_at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(feeds: &[&str]) -> String {
        let mut ex = NarrationStreamExtractor::new();
        let mut acc = String::new();
        for f in feeds {
            acc.push_str(&ex.feed(f));
        }
        acc
    }

    #[test]
    fn streams_korean_narration_incrementally() {
        let mut ex = NarrationStreamExtractor::new();
        assert_eq!(ex.feed("{\"narration\": \"비가"), "비가");
        assert_eq!(ex.feed(" 내리는 역 구석,"), " 내리는 역 구석,");
        assert_eq!(ex.feed(" 그녀가 서 있었다.\""), " 그녀가 서 있었다.");
        // Everything after the closing quote is ignored.
        assert_eq!(ex.feed(", \"choices\": [{\"id\": \"c1\"}]}"), "");
    }

    #[test]
    fn narration_after_other_fields() {
        let out = collect(&["{\"summary\": \"s\", \"state_patch\": {}, \"narration\": \"본문\"}"]);
        assert_eq!(out, "본문");
    }

    #[test]
    fn key_split_across_feeds() {
        let out = collect(&["{\"summary\": 1, \"nar", "ration\": \"이어", "짐\"}"]);
        assert_eq!(out, "이어짐");
    }

    #[test]
    fn decodes_common_escapes() {
        let out = collect(&["{\"narration\": \"첫 줄\\n둘째 \\\"대사\\\" 끝\"}"]);
        assert_eq!(out, "첫 줄\n둘째 \"대사\" 끝");
    }

    #[test]
    fn escape_split_across_feeds() {
        let out = collect(&["{\"narration\": \"줄바꿈", "\\", "n여기\"}"]);
        assert_eq!(out, "줄바꿈\n여기");
    }

    #[test]
    fn unicode_escape_surrogate_pair_split_across_feeds() {
        let mut ex = NarrationStreamExtractor::new();
        assert_eq!(ex.feed("{\"narration\": \"웃"), "웃");
        // 😀 = U+1F600 = 😀, split mid-escape and between halves.
        assert_eq!(ex.feed("\\uD8"), "");
        assert_eq!(ex.feed("3D"), "");
        assert_eq!(ex.feed("\\uDE"), "");
        let tail = ex.feed("00!\"}");
        assert_eq!(tail, "\u{1F600}!");
    }

    #[test]
    fn unpaired_high_surrogate_replaced_on_next_output() {
        let mut ex = NarrationStreamExtractor::new();
        assert_eq!(ex.feed("{\"narration\": \"x\\uD83Dy\""), "x\u{FFFD}y");
    }

    #[test]
    fn no_narration_key_emits_nothing() {
        let out = collect(&["{\"choices\": [{\"id\": \"a\"}], \"n\": 1}"]);
        assert_eq!(out, "");
    }

    #[test]
    fn prelude_text_and_think_block_skipped() {
        let out = collect(&["<think>잠깐 생각...</think>{\"narration\": \"실제 본문\"}"]);
        assert_eq!(out, "실제 본문");
    }

    #[test]
    fn non_string_value_resumes_scanning() {
        // A numeric/null "narration" is skipped; a later real key still wins.
        let out = collect(&["{\"a\": {\"narration\": null}, \"narration\": \"값\"}"]);
        assert_eq!(out, "값");
    }

    #[test]
    fn huge_prelude_does_not_grow_the_scan_buffer() {
        let mut ex = NarrationStreamExtractor::new();
        for _ in 0..10_000 {
            ex.feed("생각 중인 텍스트가 흐른다. ");
        }
        assert!(ex.scan_buf.chars().count() <= KEY.len());
        assert_eq!(ex.feed("\"narration\": \"도착\""), "도착");
    }

    #[test]
    fn empty_narration_and_immediate_close() {
        let out = collect(&["{\"narration\": \"\"}"]);
        assert_eq!(out, "");
    }

    /// Combined pipeline: a raw byte stream fractured mid-multibyte (as a
    /// local server may deliver it) goes through the SSE parser first; the
    /// extractor must then see only complete chars and still produce the
    /// narration. This pins the byte-boundary guarantee the wiremock
    /// contract tests cannot force (single-piece bodies).
    #[test]
    fn combined_with_sse_parser_across_byte_splits() {
        use crate::sse::{SseEvent, SseParser};
        let narration = "비가 그쳤다.";
        let mut sse = String::new();
        sse.push_str("data: {\"choices\":[{\"delta\":{\"content\":\"{\\\"narration\\\": \\\"");
        sse.push_str(narration);
        sse.push_str("\\\"}\"}}]}\n\ndata: [DONE]\n\n");
        // Feed the byte stream one byte at a time.
        let mut parser = SseParser::new();
        let mut extractor = NarrationStreamExtractor::new();
        let mut streamed = String::new();
        for byte in sse.as_bytes() {
            for ev in parser.feed(std::slice::from_ref(byte)) {
                if let SseEvent::Data(payload) = ev
                    && let Ok(v) = story_model::serde_json::from_str::<serde_json::Value>(&payload)
                    && let Some(text) = v["choices"][0]["delta"]["content"].as_str()
                {
                    streamed.push_str(&extractor.feed(text));
                }
            }
        }
        assert_eq!(streamed, narration);
    }

    #[test]
    fn trailing_stranded_surrogate_flushes_on_value_close() {
        let mut ex = NarrationStreamExtractor::new();
        // The value ends right after an unpaired high surrogate escape and
        // the closing quote is the LAST character of the feed — the flush
        // must happen at the InString→Done transition itself.
        let out = ex.feed("{\"narration\": \"끝\\uD83D\"");
        assert_eq!(out, "끝\u{FFFD}");
    }

    #[test]
    fn malformed_unicode_escape_replays_buffered_hex_digits() {
        // `\uD8X`: D8 is buffered before X arrives; the replay must emit
        // the whole prefix, not just `\uX` (display-only, self-corrects on
        // commit — this pins the raw-stream-tracking property).
        let out = collect(&["{\"narration\": \"a\\uD8", "Xb\"}"]);
        assert_eq!(out, "a\\uD8Xb");
    }
}
