//! Grapheme-cluster free-text input buffer (ST-022).
//!
//! [`InputBuffer`] is the edit buffer behind the free-input line: it stores the
//! player's text as a sequence of **grapheme clusters** (not bytes, not scalar
//! values), so that cursor movement, backspace, and delete always land on
//! user-perceived character boundaries. This matters acutely for Korean
//! (Hangul syllables such as `가`/`각` are one grapheme each, but composed of
//! multiple scalar values) and for emoji (a ZWJ family `👨‍👩‍👧` is one grapheme
//! built from five scalars).
//!
//! # Dependencies (ADR-003)
//!
//! `unicode-segmentation` + `unicode-width` live in `story-tui` (UI layer), so
//! `story-core` stays free of width/segmentation concerns. The engine consumes
//! only the final `text()` String and a [`story_model::ParsedAction`] produced
//! by the rule-based parser — never the buffer itself.
//!
//! # `max_chars` counts graphemes
//!
//! The character counter shown to the player (`char_count`) and the truncation
//! limit (`max_chars`) both count **grapheme clusters**, never bytes or scalar
//! values. Pasting a 10-grapheme string into a buffer with `max_chars = 5`
//! keeps the first 5 graphemes and drops the rest cleanly — never mid-syllable.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// A grapheme-cluster-oriented text edit buffer.
///
/// All cursor offsets and lengths are in grapheme clusters. The buffer is
/// single-line for the MVP: control characters (including newlines) are dropped
/// on paste.
#[derive(Clone, Debug)]
pub struct InputBuffer {
    /// The stored graphemes, in insertion order. Each entry is exactly one
    /// user-perceived character (a grapheme cluster).
    graphemes: Vec<String>,
    /// Cursor position in `[0..=graphemes.len()]` (grapheme units). An insert
    /// places the new grapheme *before* the cursor.
    cursor: usize,
    /// Maximum number of graphemes the buffer will hold. Inserts that would
    /// exceed this are truncated to fit (grapheme-aligned, never mid-syllable).
    max_chars: usize,
}

impl InputBuffer {
    /// Create an empty buffer that accepts up to `max_chars` graphemes.
    pub fn new(max_chars: usize) -> Self {
        Self {
            graphemes: Vec::new(),
            cursor: 0,
            max_chars,
        }
    }

    /// Insert a pasted string, splitting it into graphemes via
    /// `unicode-segmentation`, dropping control characters, and truncating to
    /// `max_chars` (grapheme-aligned).
    pub fn insert_str(&mut self, s: &str) {
        for g in s.graphemes(true) {
            if !is_retained_grapheme(g) {
                continue;
            }
            if self.graphemes.len() >= self.max_chars {
                break;
            }
            self.graphemes.insert(self.cursor, g.to_string());
            self.cursor += 1;
        }
    }

    /// Insert a single grapheme cluster at the cursor. Caller is expected to
    /// pass one user-perceived character; if the argument is empty it is
    /// ignored, and if it contains multiple graphemes only the first is taken.
    /// Respects `max_chars`.
    pub fn insert_grapheme(&mut self, g: &str) {
        let Some(first) = g.graphemes(true).next() else {
            return;
        };
        if !is_retained_grapheme(first) {
            return;
        }
        if self.graphemes.len() >= self.max_chars {
            return;
        }
        self.graphemes.insert(self.cursor, first.to_string());
        self.cursor += 1;
    }

    /// Delete the grapheme immediately before the cursor (Backspace). No-op at
    /// the start of the buffer.
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.graphemes.remove(self.cursor - 1);
        self.cursor -= 1;
    }

    /// Delete the grapheme at the cursor (Delete). No-op at the end of the
    /// buffer.
    pub fn delete(&mut self) {
        if self.cursor >= self.graphemes.len() {
            return;
        }
        self.graphemes.remove(self.cursor);
    }

    /// Move the cursor one grapheme left, clamped at the start.
    pub fn cursor_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Move the cursor one grapheme right, clamped at the end.
    pub fn cursor_right(&mut self) {
        if self.cursor < self.graphemes.len() {
            self.cursor += 1;
        }
    }

    /// Move the cursor to the start (before the first grapheme).
    pub fn cursor_start(&mut self) {
        self.cursor = 0;
    }

    /// Move the cursor to the end (after the last grapheme).
    pub fn cursor_end(&mut self) {
        self.cursor = self.graphemes.len();
    }

    /// Current cursor position in grapheme units (`0..=len`).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Join the stored graphemes back into a `String`.
    pub fn text(&self) -> String {
        self.graphemes.concat()
    }

    /// Text before and after the grapheme cursor, for cursor-aware rendering.
    pub fn split_at_cursor(&self) -> (String, String) {
        (
            self.graphemes[..self.cursor].concat(),
            self.graphemes[self.cursor..].concat(),
        )
    }

    /// Grapheme count (what the on-screen `n/max_chars` counter shows).
    pub fn char_count(&self) -> usize {
        self.graphemes.len()
    }

    /// Sum of `unicode-width` per grapheme — the layout cell width the text
    /// occupies in a monospace terminal. Wide CJK syllables and emoji
    /// contribute 2; combining marks and zero-width joiners contribute 0.
    pub fn display_width(&self) -> usize {
        self.graphemes.iter().map(|g| g.width()).sum()
    }

    /// Clear all graphemes and reset the cursor to the start.
    pub fn clear(&mut self) {
        self.graphemes.clear();
        self.cursor = 0;
    }
}

/// Decide whether a pasted grapheme is kept. Control characters (C0 controls
/// `U+0000..=U+001F`, plus `U+007F` DEL, plus C1 controls
/// `U+0080..=U+009F`) are dropped so a paste cannot inject newlines, tabs,
/// bells, or other non-text noise into the single-line buffer.
fn is_retained_grapheme(g: &str) -> bool {
    if g.is_empty() {
        return false;
    }
    !g.chars().all(|c| c.is_control())
}

#[cfg(test)]
mod tests {
    //! ST-022 grapheme fixtures: Korean composition, emoji (with ZWJ sequences
    //! and skin-tone modifiers), combining marks, and mixed CJK. Cursor
    //! movement + backspace/delete are grapheme-aligned, never mid-syllable.

    use super::*;

    // --- Korean composition ------------------------------------------------

    #[test]
    fn korean_syllable_is_one_grapheme() {
        // 가 (U+AC00) is a single precomposed Hangul syllable → 1 grapheme.
        let mut b = InputBuffer::new(50);
        b.insert_str("가");
        assert_eq!(b.char_count(), 1);
        assert_eq!(b.text(), "가");
        assert_eq!(b.display_width(), 2, "Hangul syllable is wide");
    }

    #[test]
    fn split_at_cursor_preserves_grapheme_boundaries() {
        let mut b = InputBuffer::new(50);
        b.insert_str("안녕👨‍👩‍👧");
        b.cursor_left();
        assert_eq!(b.split_at_cursor(), ("안녕".to_string(), "👨‍👩‍👧".to_string()));
    }

    #[test]
    fn korean_composed_syllable_with_composing_jamo_is_one_grapheme() {
        // 각 = ᄀ (U+1100) + ᅡ (U+1161) + ᆨ (U+11A8): three scalars, one
        // grapheme cluster. Backspace removes the whole syllable.
        let mut b = InputBuffer::new(50);
        b.insert_str("각");
        assert_eq!(b.char_count(), 1, "각 is one grapheme cluster");
        b.backspace();
        assert_eq!(b.char_count(), 0);
        assert!(b.text().is_empty());
    }

    #[test]
    fn korean_composed_then_standalone_jamo_are_two_graphemes() {
        // 안 (composed syllable, 1 grapheme) + ㅇ (standalone jamo U+3147,
        // 1 grapheme) = 2 graphemes. Cursor movement must land on the boundary
        // between them, never inside 안.
        let mut b = InputBuffer::new(50);
        b.insert_str("안ㅇ");
        assert_eq!(b.char_count(), 2);
        // Cursor is at end (2). Move left once → between 안 and ㅇ.
        b.cursor_left();
        assert_eq!(b.cursor(), 1);
        // Backspace removes 안 (the full grapheme before the cursor), not a
        // scalar inside it.
        b.backspace();
        assert_eq!(b.text(), "ㅇ");
        assert_eq!(b.char_count(), 1);
    }

    // --- emoji --------------------------------------------------------------

    #[test]
    fn emoji_with_variation_selector_is_one_grapheme() {
        // 👁️ = eye (U+1F441) + variation selector-16 (U+FE0F): 1 grapheme.
        let mut b = InputBuffer::new(50);
        b.insert_str("👁️");
        assert_eq!(b.char_count(), 1, "👁️ is one grapheme cluster");
        assert_eq!(b.text(), "👁️");
    }

    #[test]
    fn emoji_zwj_family_sequence_is_one_grapheme() {
        // 👨‍👩‍👧 = man + ZWJ + woman + ZWJ + girl → 1 grapheme.
        let mut b = InputBuffer::new(50);
        b.insert_str("👨‍👩‍👧");
        assert_eq!(b.char_count(), 1, "ZWJ family sequence is one grapheme");
        // Backspace deletes the entire family in one keystroke.
        b.backspace();
        assert_eq!(b.char_count(), 0);
    }

    #[test]
    fn emoji_with_skin_tone_modifier_is_one_grapheme() {
        // 👋🏽 = waving hand (U+1F44B) + skin tone (U+1F3FD) → 1 grapheme.
        let mut b = InputBuffer::new(50);
        b.insert_str("👋🏽");
        assert_eq!(b.char_count(), 1);
        b.cursor_start();
        // Delete at cursor removes the whole emoji+modifier grapheme.
        b.delete();
        assert_eq!(b.char_count(), 0);
    }

    // --- combining marks ----------------------------------------------------

    #[test]
    fn combining_mark_attaches_to_base_grapheme() {
        // "é" as e (U+0065) + combining acute (U+0301): 1 grapheme, NOT 2.
        let decomposed = "e\u{0301}";
        let mut b = InputBuffer::new(50);
        b.insert_str(decomposed);
        assert_eq!(b.char_count(), 1, "decomposed é is one grapheme");
        assert_eq!(b.text(), decomposed);
        // Cursor movement lands on grapheme boundaries: one move left puts the
        // cursor before the whole é, not between e and the accent.
        b.cursor_left();
        assert_eq!(b.cursor(), 0);
    }

    #[test]
    fn precomposed_and_decomposed_produce_same_grapheme_count() {
        let precomposed = "é"; // U+00E9
        let decomposed = "e\u{0301}"; // U+0065 + U+0301
        let mut a = InputBuffer::new(50);
        let mut b = InputBuffer::new(50);
        a.insert_str(precomposed);
        b.insert_str(decomposed);
        assert_eq!(a.char_count(), b.char_count());
        assert_eq!(a.char_count(), 1);
    }

    // --- mixed CJK ----------------------------------------------------------

    #[test]
    fn mixed_cjk_emoji_ascii_counts_graphemes_not_scalars() {
        // a + 가 + 👁️ + 다 = 4 graphemes (but 1 + 1 + 2 + 2 + ... scalars).
        let mut b = InputBuffer::new(50);
        b.insert_str("a가👁️다");
        assert_eq!(b.char_count(), 4, "count is grapheme-based");
        assert_eq!(b.display_width(), 1 + 2 + 2 + 2, "a=1, 가=2, 👁️=2, 다=2");
        assert_eq!(b.text(), "a가👁️다");
    }

    // --- cursor movement lands on grapheme boundaries -----------------------

    #[test]
    fn cursor_right_left_never_splits_a_grapheme() {
        let mut b = InputBuffer::new(50);
        b.insert_str("각다👁️");
        // graphemes: [각, 다, 👁️], len = 3, cursor at end (3).
        assert_eq!(b.cursor(), 3);
        b.cursor_left(); // 2: before 👁️
        b.cursor_left(); // 1: before 다
        b.cursor_left(); // 0: before 각
        b.cursor_left(); // clamped at 0
        assert_eq!(b.cursor(), 0);
        b.cursor_right(); // 1
        b.cursor_right(); // 2
        b.cursor_right(); // 3
        b.cursor_right(); // clamped at 3
        assert_eq!(b.cursor(), 3);
    }

    #[test]
    fn cursor_start_and_end_jump_over_graphemes() {
        let mut b = InputBuffer::new(50);
        b.insert_str("가나다");
        b.cursor_start();
        assert_eq!(b.cursor(), 0);
        // Insert at start pushes existing text right.
        b.insert_grapheme("A");
        assert_eq!(b.text(), "A가나다");
        b.cursor_end();
        assert_eq!(b.cursor(), 4);
        b.insert_grapheme("Z");
        assert_eq!(b.text(), "A가나다Z");
    }

    // --- backspace / delete alignment ---------------------------------------

    #[test]
    fn backspace_at_start_is_noop() {
        let mut b = InputBuffer::new(50);
        b.insert_str("가");
        b.cursor_start();
        b.backspace();
        assert_eq!(b.text(), "가");
    }

    #[test]
    fn delete_at_end_is_noop() {
        let mut b = InputBuffer::new(50);
        b.insert_str("가");
        // cursor at end already
        b.delete();
        assert_eq!(b.text(), "가");
    }

    #[test]
    fn backspace_removes_full_zwj_sequence() {
        let mut b = InputBuffer::new(50);
        b.insert_str("x👨‍👩‍👧y");
        // [x, 👨‍👩‍👧, y], cursor at 3
        b.cursor_left(); // before y
        b.cursor_left(); // before 👨‍👩‍👧
        b.backspace(); // removes x (grapheme before cursor)
        assert_eq!(b.text(), "👨‍👩‍👧y");
    }

    // --- paste truncation ---------------------------------------------------

    #[test]
    fn paste_over_max_chars_truncates_cleanly_at_grapheme_boundary() {
        // 5 Hangul syllables into a 3-grapheme buffer keeps 가나다, drops the
        // rest WITHOUT splitting a syllable.
        let mut b = InputBuffer::new(3);
        b.insert_str("가나다라마");
        assert_eq!(b.char_count(), 3);
        assert_eq!(b.text(), "가나다");
        // cursor clamps to the (full) end.
        assert_eq!(b.cursor(), 3);
    }

    #[test]
    fn insert_grapheme_after_full_buffer_is_dropped() {
        let mut b = InputBuffer::new(2);
        b.insert_str("ab");
        b.insert_grapheme("c");
        assert_eq!(b.char_count(), 2);
        assert_eq!(b.text(), "ab");
    }

    #[test]
    fn zero_max_chars_keeps_nothing() {
        let mut b = InputBuffer::new(0);
        b.insert_str("가나다");
        assert_eq!(b.char_count(), 0);
        assert!(b.text().is_empty());
    }

    // --- control-char filtering --------------------------------------------

    #[test]
    fn control_chars_are_dropped_on_paste() {
        let mut b = InputBuffer::new(50);
        // newlines, tabs, bell, DEL between letters are all dropped.
        b.insert_str("a\nb\tc\u{0007}d\u{007F}e");
        assert_eq!(b.text(), "abcde");
        assert_eq!(b.char_count(), 5);
    }

    #[test]
    fn insert_grapheme_drops_lone_control_char() {
        let mut b = InputBuffer::new(50);
        b.insert_grapheme("\n");
        b.insert_grapheme("\t");
        assert_eq!(b.char_count(), 0);
    }

    #[test]
    fn insert_grapheme_takes_only_first_grapheme() {
        // Caller passes two graphemes; only the first is inserted.
        let mut b = InputBuffer::new(50);
        b.insert_grapheme("가나");
        assert_eq!(b.char_count(), 1);
        assert_eq!(b.text(), "가");
    }

    // --- display width ------------------------------------------------------

    #[test]
    fn display_width_sums_per_grapheme_widths() {
        let mut b = InputBuffer::new(50);
        b.insert_str("AB");
        assert_eq!(b.display_width(), 2);
        b.clear();
        b.insert_str("가나");
        assert_eq!(b.display_width(), 4);
        b.clear();
        // 👁️ = eye (wide=2) + VS16 (width 0) → 2.
        b.insert_str("👁️");
        assert_eq!(b.display_width(), 2);
    }

    #[test]
    fn clear_resets_buffer_and_cursor() {
        let mut b = InputBuffer::new(50);
        b.insert_str("가나다");
        b.cursor_left();
        b.clear();
        assert_eq!(b.char_count(), 0);
        assert_eq!(b.cursor(), 0);
        assert!(b.text().is_empty());
        // Buffer is usable after clear.
        b.insert_str("다시");
        assert_eq!(b.text(), "다시");
    }
}
