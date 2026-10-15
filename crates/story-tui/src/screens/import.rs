//! Card import dialog: path input → running → report, rendered as a modal
//! over whatever screen is underneath (normally the home menu).

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

/// Per-frame dialog state as plain display data.
pub enum ImportDialogView<'a> {
    /// Path input; the cursor cell renders reversed.
    Input { before: &'a str, after: &'a str },
    /// The host is running the import.
    Running { path: &'a str },
    /// Finished; `ready` marks a loadable pack.
    Report {
        path: &'a str,
        lines: &'a [String],
        ready: bool,
    },
}

/// Draw the modal centered over `area`.
pub fn render_import_dialog(frame: &mut Frame, area: Rect, view: ImportDialogView<'_>) {
    let popup = centered_rect(area, 72, 18);
    frame.render_widget(Clear, popup);

    let (title, mut body, footer) = match view {
        ImportDialogView::Input { before, after } => (
            "카드 가져오기",
            vec![
                Line::from("캐릭터 카드(PNG/JSON) 파일 경로를 입력하세요."),
                Line::from("옵션은 CLI를 사용하세요: storytui import --help"),
                Line::from(""),
                Line::from(vec![
                    Span::raw("> "),
                    Span::raw(before),
                    Span::styled(
                        " ",
                        Style::default()
                            .add_modifier(Modifier::REVERSED)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(after),
                ]),
            ],
            "Enter 임포트 · Esc 취소",
        ),
        ImportDialogView::Running { path } => (
            "카드 가져오기",
            vec![
                Line::from("임포트 중…"),
                Line::from(""),
                Line::from(Span::styled(path, Style::default().fg(Color::DarkGray))),
            ],
            "잠시만 기다려 주세요",
        ),
        ImportDialogView::Report { path, lines, ready } => {
            let mut body = vec![
                Line::from(Span::styled(path, Style::default().fg(Color::DarkGray))),
                Line::from(""),
            ];
            body.extend(lines.iter().map(|line| {
                if line.starts_with('!') {
                    Line::from(Span::styled(
                        line.clone(),
                        Style::default().fg(Color::Yellow),
                    ))
                } else {
                    Line::from(line.clone())
                }
            }));
            (
                "임포트 리포트",
                body,
                if ready {
                    "Enter 이 팩 불러오기 · Esc 닫기"
                } else {
                    "Esc 닫기"
                },
            )
        }
    };
    body.push(Line::from(""));
    body.push(Line::from(Span::styled(
        footer,
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightCyan))
        .title(title);
    frame.render_widget(
        Paragraph::new(body)
            .block(block)
            .wrap(ratatui::widgets::Wrap { trim: false }),
        popup,
    );
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

    fn draw(view: ImportDialogView<'_>) -> String {
        let mut terminal = Terminal::new(TestBackend::new(90, 30)).expect("terminal");
        terminal
            .draw(|frame| render_import_dialog(frame, frame.area(), view))
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
    fn input_view_shows_prompt_and_cursor_line() {
        let out = draw(ImportDialogView::Input {
            before: "stories/",
            after: "",
        });
        assert!(out.contains("카드 가져오기"));
        assert!(out.contains("> stories/"));
        assert!(out.contains("Enter 임포트"));
    }

    #[test]
    fn report_view_lists_lines_and_ready_footer() {
        let lines = vec!["로어북 엔트리 2개".to_string()];
        let out = draw(ImportDialogView::Report {
            path: "stories/yuna",
            lines: &lines,
            ready: true,
        });
        assert!(out.contains("임포트 리포트"));
        assert!(out.contains("stories/yuna"));
        assert!(out.contains("로어북 엔트리 2개"));
        assert!(out.contains("Enter 이 팩 불러오기"));
    }
}
