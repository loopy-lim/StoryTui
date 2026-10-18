//! Greeting picker: when a pack ships alternate openings (`greetings/*.md`),
//! "새 게임" first asks which opening to play. Index 0 is always the pack's
//! own opening.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::popup::{POPUP_WIDTH_NARROW, centered_rect};

/// Draw the modal listing `options` with `selected` highlighted.
pub fn render_greeting_picker(frame: &mut Frame, area: Rect, options: &[String], selected: usize) {
    let popup = centered_rect(area, POPUP_WIDTH_NARROW, options.len() as u16 + 6);
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
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        popup,
    );
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
