//! Greeting picker: when a pack ships alternate openings (`greetings/*.md`),
//! "새 게임" first asks which opening to play. Index 0 is always the pack's
//! own opening.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

/// Draw the modal listing `options` with `selected` highlighted.
pub fn render_greeting_picker(frame: &mut Frame, area: Rect, options: &[String], selected: usize) {
    let popup = centered_rect(area, 60, options.len() as u16 + 6);
    frame.render_widget(Clear, popup);

    let active = selected.min(options.len().saturating_sub(1));
    let mut lines = Vec::new();
    for (index, option) in options.iter().enumerate() {
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
        lines.push(Line::from(Span::styled(format!("{prefix}{option}"), style)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑↓ 이동 · Enter 선택 · Esc 취소",
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightCyan))
        .title("오프닝 선택");
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

    #[test]
    fn picker_lists_options_and_marks_selection() {
        let options = vec!["기본 오프닝".to_string(), "비 오는 날의 인사".to_string()];
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
        terminal
            .draw(|frame| render_greeting_picker(frame, frame.area(), &options, 1))
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
        assert!(out.contains("오프닝 선택"), "{out}");
        assert!(out.contains("기본 오프닝"));
        assert!(out.contains("비 오는 날의 인사"));
    }
}
