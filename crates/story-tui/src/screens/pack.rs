//! Pack picker: the start menu's "팩 선택" row opens this modal listing the
//! story packs discovered next to the active one. `Enter` asks the host to
//! swap packs (engine reset), `Esc` returns to the menu untouched.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

/// Draw the modal listing `labels` with `selected` highlighted.
pub fn render_pack_picker(frame: &mut Frame, area: Rect, labels: &[String], selected: usize) {
    let popup = centered_rect(area, 70, labels.len() as u16 + 6);
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
        "↑↓ 이동 · Enter 이 팩 불러오기 · Esc 취소",
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightCyan))
        .title("팩 선택");
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

/// A centered rect of at most `width`×`height`, clamped to `area`.
fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let horizontal = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(width),
        Constraint::Fill(1),
    ])
    .split(area);
    let vertical = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height),
        Constraint::Fill(1),
    ])
    .split(area);
    Rect {
        x: horizontal[1].x,
        y: vertical[1].y,
        width: horizontal[1].width,
        height: vertical[1].height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use unicode_width::UnicodeWidthStr;

    fn draw_to_string(labels: &[String], selected: usize) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
        terminal
            .draw(|frame| render_pack_picker(frame, frame.area(), labels, selected))
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
    }
}
