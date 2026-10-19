//! Initial home screen for choosing how to enter a StoryTUI session.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

/// Render the launcher menu. `selected` is an index into the six fixed rows.
/// `status` is a one-line guidance/notice rendered between the rows and the
/// footer: `attention` rows are yellow, the rest gray.
pub fn render_home(
    frame: &mut Frame,
    area: Rect,
    selected: usize,
    can_continue: bool,
    status: Option<(&str, bool)>,
) {
    // Destructive exit sits last so scanning top-to-bottom ends with the
    // content rows, not the exit.
    let rows = [
        ("새 게임", "새로운 이야기로 시작합니다.", true),
        ("이어하기", "자동 저장된 이야기를 이어갑니다.", can_continue),
        ("설정", "모델과 생성 설정을 확인합니다.", true),
        (
            "카드 가져오기",
            "캐릭터 카드를 이 팩 목록에 임포트합니다.",
            true,
        ),
        ("팩 선택", "이 목록의 다른 스토리 팩을 불러옵니다.", true),
        ("종료", "StoryTUI를 종료합니다.", true),
    ];
    let content = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(18),
        Constraint::Fill(1),
    ])
    .split(area)[1];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(crate::theme::BORDER_TYPE)
        .border_style(crate::theme::Styles::border_primary())
        .title(Span::styled(
            " ✦ 시작 메뉴 ✦ ",
            crate::theme::Styles::title(),
        ));
    let mut lines = vec![Line::from(Span::styled(
        "✦  S T O R Y   T U I  ✦",
        crate::theme::Styles::title(),
    ))];
    for (index, (label, hint, enabled)) in rows.iter().enumerate() {
        let active = index == selected && *enabled;
        let style = if active {
            Style::default()
                .fg(crate::theme::Palette::SELECTED)
                .add_modifier(Modifier::BOLD)
        } else if *enabled {
            Style::default().fg(crate::theme::Palette::PROSE)
        } else {
            Style::default().fg(crate::theme::Palette::MUTED)
        };
        let item_text = if active {
            format!("▶ [ {label} ]")
        } else {
            format!("    {label}  ")
        };
        lines.push(Line::from(Span::styled(item_text, style)));
        lines.push(Line::from(Span::styled(
            format!("    {hint}"),
            Style::default().fg(crate::theme::Palette::MUTED),
        )));
    }
    lines.push(Line::from(""));
    if let Some((line, attention)) = status {
        lines.push(Line::from(Span::styled(
            line,
            if attention {
                Style::default()
                    .fg(crate::theme::Palette::CAUTION)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(crate::theme::Palette::MUTED)
            },
        )));
    }
    lines.push(Line::from(Span::styled(
        "💡 마우스 클릭 · ↑↓ 이동 · Enter 선택 · q 종료",
        Style::default().fg(crate::theme::Palette::MUTED),
    )));
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .alignment(Alignment::Center),
        content,
    );
}

/// Resolve a left-click row inside the centered home menu to a menu item.
/// Both a label and its explanatory line are clickable.
pub fn menu_selection_at(area: Rect, row: u16) -> Option<usize> {
    let content = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(18),
        Constraint::Fill(1),
    ])
    .split(area)[1];
    // Internal layout: title (0), then two lines per menu row (1..=12),
    // blank + status + footer after that. The first line of each pair is the
    // label, the second its hint — both select the same row.
    let offset = row.checked_sub(content.y + 1)?;
    if offset == 0 || offset > 12 {
        return None;
    }
    Some(((offset - 1) / 2) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use unicode_width::UnicodeWidthStr;

    /// Draw the home menu and collect the buffer as one string, skipping the
    /// spacer cell after double-width symbols so Hangul stays contiguous.
    fn render_to_string(
        selected: usize,
        can_continue: bool,
        status: Option<(&str, bool)>,
    ) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
        terminal
            .draw(|frame| render_home(frame, frame.area(), selected, can_continue, status))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            let mut previous_was_wide = false;
            for x in 0..buffer.area.width {
                let symbol = buffer[(x, y)].symbol();
                if previous_was_wide && symbol == " " {
                    previous_was_wide = false;
                    continue;
                }
                previous_was_wide = symbol.width() == 2;
                out.push_str(symbol);
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn click_rows_resolve_to_menu_indices_including_the_sixth_row() {
        let area = Rect::new(0, 0, 80, 24);
        let content = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(18),
            Constraint::Fill(1),
        ])
        .split(area)[1];
        let base = content.y + 1;
        // Label line and hint line of each row resolve to the same index.
        for (index, offset) in [1u16, 3, 5, 7, 9, 11].iter().enumerate() {
            assert_eq!(
                menu_selection_at(area, base + *offset),
                Some(index),
                "label line of row {index}"
            );
            assert_eq!(
                menu_selection_at(area, base + offset + 1),
                Some(index),
                "hint line of row {index}"
            );
        }
        // Title, blank/status, and footer lines select nothing.
        assert_eq!(menu_selection_at(area, base), None);
        assert_eq!(menu_selection_at(area, base + 13), None);
        assert_eq!(menu_selection_at(area, base + 14), None);
    }

    #[test]
    fn home_renders_six_rows_and_the_status_line() {
        let out = render_to_string(
            0,
            false,
            Some((
                "모델 서버를 찾지 못했어요 — 서버를 켠 뒤 설정을 열면 자동으로 연결됩니다.",
                true,
            )),
        );
        assert!(out.contains("시작 메뉴"), "{out}");
        assert!(out.contains("새 게임"), "{out}");
        assert!(out.contains("팩 선택"), "{out}");
        assert!(out.contains("모델 서버를 찾지 못했어요"), "{out}");
        assert!(out.contains("Enter 선택"), "{out}");
    }

    #[test]
    fn home_without_status_still_renders_all_rows() {
        let out = render_to_string(5, false, None);
        assert!(
            out.contains("이 목록의 다른 스토리 팩을 불러옵니다"),
            "{out}"
        );
        assert!(out.contains("StoryTUI를 종료합니다"), "{out}");
    }
}
