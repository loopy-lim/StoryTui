//! Card import dialog: path input → running → report, rendered as a modal
//! over whatever screen is underneath (normally the home menu).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::popup::{POPUP_WIDTH_WIDE, centered_rect};

/// Per-frame dialog state as plain display data.
pub enum ImportDialogView<'a> {
    /// Path input; the cursor cell renders reversed.
    Input { before: &'a str, after: &'a str },
    /// Browsing directories; `entries` pairs a name with `is_dir`.
    Browse {
        dir: &'a str,
        entries: &'a [(String, bool)],
        selected: usize,
    },
    /// The host is running the import.
    Running { path: &'a str },
    /// Finished; `ready` marks a loadable pack.
    Report {
        path: &'a str,
        lines: &'a [String],
        ready: bool,
    },
}

/// Draw the modal centered over `area`, sized to its content so a one-line
/// report never floats in an empty box.
pub fn render_import_dialog(frame: &mut Frame, area: Rect, view: ImportDialogView<'_>) {
    let (title, mut body, footer) = match view {
        ImportDialogView::Input { before, after } => (
            "카드 가져오기",
            vec![
                Line::from("카드 파일을 경로로 입력하거나, Tab으로 파일 브라우저를 여세요."),
                Line::from("지원 형식: PNG/JSON/CHARX · 옵션은 CLI: storytui import --help"),
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
            "Enter 임포트 · Tab 파일 브라우저 · Esc 취소",
        ),
        ImportDialogView::Browse {
            dir,
            entries,
            selected,
        } => {
            // Sliding window: the modal fits ~12 rows of entries; keep the
            // selection inside it so long listings never lose the cursor.
            const VISIBLE_ROWS: usize = 12;
            let active = selected.min(entries.len().saturating_sub(1));
            let window_start = active.saturating_sub(VISIBLE_ROWS.saturating_sub(1));
            let window_end = (window_start + VISIBLE_ROWS).min(entries.len());
            let mut body = vec![
                Line::from(Span::styled(dir, Style::default().fg(Color::DarkGray))),
                Line::from(""),
            ];
            if entries.is_empty() {
                body.push(Line::from(Span::styled(
                    "표시할 항목이 없습니다 (읽기 실패했을 수 있습니다).",
                    Style::default().fg(Color::DarkGray),
                )));
            } else {
                for (index, (name, is_dir)) in entries[window_start..window_end].iter().enumerate()
                {
                    let index = index + window_start;
                    let label = if *is_dir {
                        format!("{name}/")
                    } else {
                        name.clone()
                    };
                    if index == active {
                        body.push(Line::from(Span::styled(
                            format!("▶ {label}"),
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                                .add_modifier(Modifier::REVERSED),
                        )));
                    } else {
                        body.push(Line::from(Span::styled(
                            format!("  {label}"),
                            Style::default().fg(Color::Gray),
                        )));
                    }
                }
                if entries.len() > VISIBLE_ROWS {
                    body.push(Line::from(Span::styled(
                        format!("전체 {}개 항목", entries.len()),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            (
                "카드 가져오기 — 파일 브라우저",
                body,
                "Enter 폴더 열기/카드 선택 · Backspace 상위 · Tab 경로 입력 · Esc 취소",
            )
        }
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

    let height = (body.len() as u16 + 2).clamp(7, area.height.max(7));
    let popup = centered_rect(area, POPUP_WIDTH_WIDE, height);
    frame.render_widget(Clear, popup);
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
    fn browse_view_lists_entries_with_hints() {
        let entries = vec![("sub".to_string(), true), ("card.charx".to_string(), false)];
        let out = draw(ImportDialogView::Browse {
            dir: "/tmp/cards",
            entries: &entries,
            selected: 0,
        });
        assert!(out.contains("파일 브라우저"), "{out}");
        assert!(out.contains("/tmp/cards"), "{out}");
        assert!(out.contains("sub/"), "{out}");
        assert!(out.contains("card.charx"), "{out}");
        assert!(out.contains("Tab 경로 입력"), "{out}");
    }

    #[test]
    fn browse_view_explains_empty_listings() {
        let out = draw(ImportDialogView::Browse {
            dir: "/empty",
            entries: &[],
            selected: 0,
        });
        assert!(out.contains("표시할 항목이 없습니다"), "{out}");
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
