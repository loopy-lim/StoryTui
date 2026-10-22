//! Keymap reference table (ST-035, Technical Spec §14.1).
//!
//! Pure data: a static list of `(key, action_ko)` pairs the Settings screen
//! renders (two columns) and the `?` help overlay re-uses. Kept in its own
//! module so the help overlay doesn't pull settings.rs.
//!
//! The table must stay in sync with what `app.rs`/`event.rs` actually bind —
//! it documents the whole game keymap, not just the story shortcuts.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// One row of the keymap reference.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeymapRow {
    /// The key (or short chord) shown in the "key" column.
    pub key: &'static str,
    /// Korean description of what the key does.
    pub action_ko: &'static str,
}

/// The canonical StoryTUI keymap (Technical Spec §14.1).
///
/// Korean 2-beolsik layouts hit the same physical keys (q=ㅂ, j=ㅓ, …); the
/// table shows the Latin keycaps. Order is the on-screen order; do not
/// reorder without bumping the snapshot tests.
pub const KEYMAP: &[KeymapRow] = &[
    KeymapRow {
        key: "↑ / k",
        action_ko: "이전 선택",
    },
    KeymapRow {
        key: "↓ / j",
        action_ko: "다음 선택",
    },
    KeymapRow {
        key: "1..9",
        action_ko: "n번째 선택 바로 확정",
    },
    KeymapRow {
        key: "Enter",
        action_ko: "현재 선택 확정",
    },
    KeymapRow {
        key: "I",
        action_ko: "직접 입력 (선택지가 있을 때)",
    },
    KeymapRow {
        key: "R",
        action_ko: "선택지 다시 만들기",
    },
    KeymapRow {
        key: "c",
        action_ko: "생성 취소 (로딩 중)",
    },
    KeymapRow {
        key: "h",
        action_ko: "이전 생성 기록",
    },
    KeymapRow {
        key: "m",
        action_ko: "기억 목록",
    },
    KeymapRow {
        key: "s",
        action_ko: "저장 화면 열기",
    },
    KeymapRow {
        key: "l",
        action_ko: "불러오기 화면 열기",
    },
    KeymapRow {
        key: "v",
        action_ko: "내레이션 낭독 켜기/끄기",
    },
    KeymapRow {
        key: "n",
        action_ko: "새 게임 시작",
    },
    KeymapRow {
        key: "Home",
        action_ko: "시작 메뉴 (팩 선택·카드 가져오기 포함)",
    },
    KeymapRow {
        key: "o",
        action_ko: "게임 중 설정 열기",
    },
    KeymapRow {
        key: "?",
        action_ko: "도움말 오버레이",
    },
    KeymapRow {
        key: "e",
        action_ko: "프로바이더 전환 (설정 화면)",
    },
    KeymapRow {
        key: "i",
        action_ko: "장면 이미지 켜기/끄기 (설정 화면)",
    },
    KeymapRow {
        key: "g",
        action_ko: "생성 방식 전환 — 단일 패스 ↔ 2단계 스트리밍 (설정 화면)",
    },
    KeymapRow {
        key: "f",
        action_ko: "표정 스프라이트 켜기/끄기 (설정 화면)",
    },
    KeymapRow {
        key: "Esc",
        action_ko: "오류·모달 닫기",
    },
    KeymapRow {
        key: "q / Ctrl+C",
        action_ko: "종료",
    },
];

/// One keymap row as a styled line: `<key>  ·  <action_ko>` with the key in
/// bold cyan.
fn keymap_line(row: &KeymapRow) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            row.key.to_string(),
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Cyan),
        ),
        Span::raw("  ·  "),
        Span::raw(row.action_ko.to_string()),
    ])
}

/// Build the keymap as styled lines for a single-column rendering.
pub fn keymap_lines() -> Vec<Line<'static>> {
    KEYMAP.iter().map(keymap_line).collect()
}

/// Build the keymap as two balanced columns so the table fits mid-size
/// terminals without scrolling. Each column is an independent line list; the
/// Settings screen renders them side by side.
pub fn keymap_lines_two_columns() -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let half = KEYMAP.len().div_ceil(2);
    let (left, right) = KEYMAP.split_at(half);
    (
        left.iter().map(keymap_line).collect(),
        right.iter().map(keymap_line).collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymap_documents_every_story_shortcut() {
        let keys: Vec<_> = KEYMAP.iter().map(|row| row.key).collect();
        // Rows added after the first audit: these were bound in app.rs/event.rs
        // but missing from the table.
        for expected in ["I", "R", "h", "m", "s", "l", "n", "?"] {
            assert!(keys.contains(&expected), "keymap is missing {expected}");
        }
    }

    #[test]
    fn two_columns_split_the_table_without_loss() {
        let (left, right) = keymap_lines_two_columns();
        assert_eq!(left.len() + right.len(), KEYMAP.len());
        assert!(!left.is_empty() && !right.is_empty());
    }
}
