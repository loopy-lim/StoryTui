//! Keymap reference table (ST-035, Technical Spec §14.1).
//!
//! Pure data: a static list of `(key, action_ko)` pairs the Settings screen
//! renders. Kept in its own module so a future help overlay can re-use it
//! without pulling settings.rs.

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
/// Order is the on-screen order; do not reorder without bumping the snapshot
/// tests.
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
        key: "c",
        action_ko: "생성 취소 (로딩 중)",
    },
    KeymapRow {
        key: "o",
        action_ko: "게임 중 설정 열기",
    },
    KeymapRow {
        key: "v",
        action_ko: "내레이션 낭독 켜기/끄기",
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
        key: "Home",
        action_ko: "시작 메뉴 (팩 선택·카드 가져오기 포함)",
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

/// Build the keymap as styled [`Line`]s for the Settings screen's keymap block.
/// Each line is `<key>  ·  <action_ko>` with the key in bold cyan.
pub fn keymap_lines() -> Vec<Line<'static>> {
    KEYMAP
        .iter()
        .map(|row| {
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
        })
        .collect()
}
