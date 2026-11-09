//! Keymap reference table (ST-035, Technical Spec §14.1).
//!
//! Pure data: a static list of `(key, action_ko)` pairs the Settings screen
//! renders (two columns) and the `?` help overlay re-uses. Kept in its own
//! module so the help overlay doesn't pull settings.rs.
//!
//! The table must stay in sync with what `app.rs`/`event.rs` actually bind —
//! it documents the whole game keymap, not just the story shortcuts.
//!
//! # Display policy (2026-10-22 UX simplification)
//!
//! Rows are grouped by screen (`KeymapGroup`) so settings-only keys stop
//! reading as game keys, descriptions are one line each, and the Korean
//! 2-beolsik alias note is a single footer line instead of per-row noise.
//! `docs/UI-NAVIGATION.md` keeps the full `u/ㅕ`-style alias table.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::{Palette, Styles};

/// Which screen a row belongs to — the help overlay groups by this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeymapGroup {
    /// Keys bound while playing (story screen).
    Game,
    /// Keys that only work inside the settings screen.
    Settings,
}

/// One row of the keymap reference.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeymapRow {
    /// The key (or short chord) shown in the "key" column.
    pub key: &'static str,
    /// Korean description of what the key does (one line, no alias noise).
    pub action_ko: &'static str,
    /// Screen the key belongs to.
    pub group: KeymapGroup,
}

/// The canonical StoryTUI keymap (Technical Spec §14.1).
///
/// Korean 2-beolsik layouts hit the same physical keys (q=ㅂ, j=ㅓ, …); the
/// table shows the Latin keycaps and the note line explains the rest. Order
/// is the on-screen order within each group.
pub const KEYMAP: &[KeymapRow] = &[
    // --- 게임 화면 ---
    KeymapRow {
        key: "↑ / k",
        action_ko: "이전 선택",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "↓ / j",
        action_ko: "다음 선택",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "1..9",
        action_ko: "n번째 선택 바로 확정",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "Alt+1-9",
        action_ko: "퀵 입력",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "Enter",
        action_ko: "현재 선택 확정",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "I",
        action_ko: "직접 입력",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "R",
        action_ko: "다시 만들기 (생성 중엔 턴)",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "w",
        action_ko: "이어쓰기",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "z",
        action_ko: "대필",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "g",
        action_ko: "인물 대화",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "p",
        action_ko: "인물 목록",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "u",
        action_ko: "직전 턴 되돌리기",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "[ / ]",
        action_ko: "변형 순환 (영문 모드)",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "c",
        action_ko: "생성 취소 (로딩 중)",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "h",
        action_ko: "이전 기록 · 편집/삭제",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "m",
        action_ko: "기억 목록",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "s",
        action_ko: "저장",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "l",
        action_ko: "불러오기",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "v",
        action_ko: "낭독 켜기/끄기",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "n",
        action_ko: "새 게임 확인",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "Home",
        action_ko: "시작 메뉴",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "o",
        action_ko: "설정 열기",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "?",
        action_ko: "이 도움말",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "Esc",
        action_ko: "오류·모달 닫기",
        group: KeymapGroup::Game,
    },
    KeymapRow {
        key: "q / Ctrl+C",
        action_ko: "종료",
        group: KeymapGroup::Game,
    },
    // --- 설정 화면에서 ---
    KeymapRow {
        key: "e",
        action_ko: "프로바이더 전환",
        group: KeymapGroup::Settings,
    },
    KeymapRow {
        key: "i",
        action_ko: "장면 이미지 켜기/끄기",
        group: KeymapGroup::Settings,
    },
    KeymapRow {
        key: "b",
        action_ko: "이미지 렌더 방식 전환",
        group: KeymapGroup::Settings,
    },
    KeymapRow {
        key: "g",
        action_ko: "생성 방식 전환",
        group: KeymapGroup::Settings,
    },
    KeymapRow {
        key: "f",
        action_ko: "표정 스프라이트 켜기/끄기",
        group: KeymapGroup::Settings,
    },
    KeymapRow {
        key: "x",
        action_ko: "스타일 프리셋 순환",
        group: KeymapGroup::Settings,
    },
    KeymapRow {
        key: "a",
        action_ko: "페르소나 편집",
        group: KeymapGroup::Settings,
    },
    KeymapRow {
        key: "w",
        action_ko: "로어북",
        group: KeymapGroup::Settings,
    },
    KeymapRow {
        key: "t",
        action_ko: "노트",
        group: KeymapGroup::Settings,
    },
    KeymapRow {
        key: "r",
        action_ko: "프롬프트",
        group: KeymapGroup::Settings,
    },
];

const GAME_HEADER: &str = "게임 화면";
const SETTINGS_HEADER: &str = "설정 화면에서";
const ALIAS_NOTE: &str = "한글 2벌식도 같은 물리 키 (q=ㅂ, j=ㅓ, s=ㄴ …)";

/// One keymap row as a styled line: `<key>  ·  <action_ko>` with the key in
/// bold primary accent.
fn keymap_line(row: &KeymapRow) -> Line<'static> {
    Line::from(vec![
        Span::styled(row.key.to_string(), Styles::keycap()),
        Span::styled("  ·  ", Styles::muted()),
        Span::styled(row.action_ko.to_string(), Styles::item_normal()),
    ])
}

/// A bold group header line.
fn group_header(title: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        title.to_string(),
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Palette::ACCENT),
    ))
}

fn group_rows(group: KeymapGroup) -> impl Iterator<Item = &'static KeymapRow> {
    KEYMAP.iter().filter(move |row| row.group == group)
}

/// Build the keymap as styled lines for a single-column rendering: group
/// headers, rows, and the one-line Korean-alias note.
pub fn keymap_lines() -> Vec<Line<'static>> {
    let mut lines = vec![group_header(GAME_HEADER)];
    lines.extend(group_rows(KeymapGroup::Game).map(keymap_line));
    lines.push(Line::from(""));
    lines.push(group_header(SETTINGS_HEADER));
    lines.extend(group_rows(KeymapGroup::Settings).map(keymap_line));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        ALIAS_NOTE.to_string(),
        Styles::muted(),
    )));
    lines
}

/// Build the keymap as two balanced columns so the table fits mid-size
/// terminals without scrolling. The split never strands a group header at
/// the bottom of a column: the settings group always shares the right column
/// (its header must sit at a column top), so the game rows are what
/// rebalances — the split point keeps both columns inside the `?` help
/// overlay's 20-row body budget (22-line popup minus borders) while staying
/// as even as that budget allows.
pub fn keymap_lines_two_columns() -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    /// Rows the `?` overlay body offers per column (`render_help_overlay`'s
    /// 22-line popup minus its borders).
    const HELP_BODY_BUDGET: usize = 20;

    let game: Vec<&KeymapRow> = group_rows(KeymapGroup::Game).collect();
    let settings: Vec<&KeymapRow> = group_rows(KeymapGroup::Settings).collect();

    // Everything after the game tail in the right column: blank + settings
    // header + settings rows + blank + alias note.
    let right_fixed = 4 + settings.len();
    // The split must keep right ≤ budget, then go as even as possible.
    let min_split = game
        .len()
        .saturating_sub(HELP_BODY_BUDGET.saturating_sub(right_fixed));
    let balanced = (game.len() + right_fixed).div_ceil(2);
    // The left column carries the game header plus its rows, so it also has
    // its own budget: once the game group outgrows the even split, the clamp
    // moves overflow to the right column (which then must still fit — the
    // budget test below fails loudly when the whole table no longer does).
    let max_split = HELP_BODY_BUDGET.saturating_sub(1);
    let split_at = balanced.max(min_split).min(max_split).min(game.len());

    let mut left = vec![group_header(GAME_HEADER)];
    left.extend(game[..split_at].iter().map(|row| keymap_line(row)));

    let mut right: Vec<Line<'static>> = game[split_at..]
        .iter()
        .map(|row| keymap_line(row))
        .collect();
    right.push(Line::from(""));
    right.push(group_header(SETTINGS_HEADER));
    right.extend(settings.iter().map(|row| keymap_line(row)));
    right.push(Line::from(""));
    right.push(Line::from(Span::styled(
        ALIAS_NOTE.to_string(),
        Styles::muted(),
    )));

    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn keymap_documents_every_story_shortcut() {
        let keys: Vec<_> = KEYMAP.iter().map(|row| row.key).collect();
        // Rows added after the first audit: these were bound in app.rs/event.rs
        // but missing from the table.
        for expected in ["I", "b", "R", "u", "h", "m", "s", "l", "n", "?"] {
            assert!(keys.contains(&expected), "keymap is missing {expected}");
        }
    }

    #[test]
    fn every_row_is_grouped_and_one_line_short() {
        for row in KEYMAP {
            assert!(
                row.action_ko.width() <= 30,
                "row '{}: {}' is too long for one help column: {}",
                row.key,
                row.action_ko,
                row.action_ko.width()
            );
        }
    }

    #[test]
    fn two_columns_cover_every_row_without_orphan_headers() {
        let (left, right) = keymap_lines_two_columns();
        let flat: Vec<String> = left
            .iter()
            .chain(right.iter())
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.to_string())
                    .collect()
            })
            .collect();
        // Every bound row appears exactly once. Match on the full row (key +
        // description): the same keycap may be documented in both groups
        // (story `w` 이어쓰기 vs settings `w` 로어북) by design.
        for row in KEYMAP {
            let expected = format!("{}  ·  {}", row.key, row.action_ko);
            let hits = flat.iter().filter(|line| **line == expected).count();
            assert_eq!(hits, 1, "row '{expected}' appears {hits} times");
        }
        // Group headers sit at a column top, never as the last line.
        assert_eq!(flat[0], GAME_HEADER);
        let settings_pos = flat
            .iter()
            .position(|line| line == SETTINGS_HEADER)
            .expect("settings header present");
        assert!(
            settings_pos + 1 < flat.len(),
            "settings header must not be the last line of a column"
        );
        assert!(flat.last().is_some_and(|line| line.contains("2벌식")));
    }

    #[test]
    fn both_columns_fit_the_help_overlay_body_budget() {
        // The `?` overlay body is 20 rows tall (22-line popup minus borders).
        // A column past the budget would silently clip its tail rows.
        let (left, right) = keymap_lines_two_columns();
        assert!(
            left.len() <= 20 && right.len() <= 20,
            "help columns exceed the overlay budget: left={}, right={}",
            left.len(),
            right.len()
        );
    }
}
