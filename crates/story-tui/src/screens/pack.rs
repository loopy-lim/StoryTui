//! Pack picker: the start menu's "팩 선택" row opens this modal listing the
//! story packs discovered next to the active one. `Enter` asks the host to
//! swap packs (engine reset), `Esc` returns to the menu untouched.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::popup::{POPUP_WIDTH_MEDIUM, centered_rect};

/// Draw the modal listing `labels` with `selected` highlighted.
pub fn render_pack_picker(frame: &mut Frame, area: Rect, labels: &[String], selected: usize) {
    let popup = centered_rect(area, POPUP_WIDTH_MEDIUM, labels.len() as u16 + 6);
    frame.render_widget(Clear, popup);

    let mut lines = Vec::new();
    if labels.is_empty() {
        lines.push(Line::from(Span::styled(
            "불러올 수 있는 팩이 없습니다.",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let active = selected.min(labels.len().saturating_sub(1));
        for (index, label) in labels.iter().enumerate() {
            let is_active = index == active;
            let prefix = if is_active { "▶ " } else { "  " };
            let style = if is_active {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::REVERSED)
            } else {
                Style::default().fg(Color::Gray)
            };
            lines.push(Line::from(Span::styled(format!("{prefix}{label}"), style)));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑↓ 이동 · Enter 이 팩 불러오기 · d 팩 삭제 · Esc 취소",
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightCyan))
        .title("팩 선택");
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        popup,
    );
}

/// Draw the pack deletion confirmation over the picker.
pub fn render_pack_delete_confirm(frame: &mut Frame, area: Rect, title: &str) {
    let popup = centered_rect(area, 56, 10);
    frame.render_widget(Clear, popup);

    let lines = vec![
        Line::from(format!("「{title}」 팩 디렉터리를 삭제합니다.")),
        Line::from(""),
        Line::from("이 작업은 되돌릴 수 없습니다. 세이브는 유지됩니다."),
        Line::from(""),
        Line::from(Span::styled(
            "Enter 삭제 · Esc 취소",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .title("팩 삭제");
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use unicode_width::UnicodeWidthStr;

    fn draw_to_string(labels: &[String], selected: usize) -> String {
        draw_with(|frame, area| render_pack_picker(frame, area, labels, selected))
    }

    fn draw_with(render: impl FnOnce(&mut Frame, Rect)) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
        terminal
            .draw(|frame| render(frame, frame.area()))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            // Skip the spacer cell after double-width symbols so Hangul stays
            // contiguous.
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
    fn picker_lists_pack_labels_and_marks_selection() {
        let labels = vec![
            "비가 그친 뒤 · ko-KR (현재)".to_string(),
            "Starlight Station · en-US".to_string(),
        ];
        let out = draw_to_string(&labels, 0);
        assert!(out.contains("팩 선택"), "{out}");
        assert!(out.contains("비가 그친 뒤 · ko-KR (현재)"), "{out}");
        assert!(out.contains("Starlight Station · en-US"), "{out}");
        assert!(out.contains("Enter 이 팩 불러오기"), "{out}");
    }

    #[test]
    fn empty_scan_explains_there_is_nothing_to_load() {
        let out = draw_to_string(&[], 0);
        assert!(out.contains("불러올 수 있는 팩이 없습니다."), "{out}");
        assert!(out.contains("d 팩 삭제"), "{out}");
    }

    #[test]
    fn delete_confirm_shows_title_and_irreversibility() {
        let out =
            draw_with(|frame, area| super::render_pack_delete_confirm(frame, area, "비가 그친 뒤"));
        assert!(out.contains("팩 삭제"), "{out}");
        assert!(out.contains("비가 그친 뒤"), "{out}");
        assert!(out.contains("되돌릴 수 없습니다"), "{out}");
        assert!(out.contains("세이브는 유지"), "{out}");
        assert!(out.contains("Enter 삭제"), "{out}");
    }
}
