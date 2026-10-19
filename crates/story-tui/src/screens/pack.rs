//! Pack picker: the start menu's "팩 선택" row opens this modal listing the
//! story packs discovered next to the active one. `Enter` asks the host to
//! swap packs (engine reset), `Esc` returns to the menu untouched.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::popup::{POPUP_WIDTH_MEDIUM, centered_rect};

/// Resolve a click row inside the pack picker modal to a pack index.
pub fn pack_selection_at(
    area: Rect,
    row: u16,
    pack_count: usize,
    has_notice: bool,
) -> Option<usize> {
    if pack_count == 0 {
        return None;
    }
    let notice_rows = u16::from(has_notice);
    let popup = centered_rect(
        area,
        POPUP_WIDTH_MEDIUM,
        pack_count as u16 + 6 + notice_rows,
    );
    let start_row = popup.y.saturating_add(1 + if has_notice { 2 } else { 0 });
    let end_row = start_row.saturating_add(pack_count as u16);
    if row >= start_row && row < end_row {
        Some((row - start_row) as usize)
    } else {
        None
    }
}

/// Draw the modal listing `labels` with `selected` highlighted. `notice`
/// (e.g. a failed deletion) renders inside the modal so it cannot be hidden
/// behind the popup itself.
pub fn render_pack_picker(
    frame: &mut Frame,
    area: Rect,
    labels: &[String],
    selected: usize,
    notice: Option<&str>,
) {
    let notice_rows = u16::from(notice.is_some());
    let popup = centered_rect(
        area,
        POPUP_WIDTH_MEDIUM,
        labels.len() as u16 + 6 + notice_rows,
    );
    frame.render_widget(Clear, popup);

    let mut lines = Vec::new();
    if let Some(notice) = notice {
        lines.push(Line::from(Span::styled(
            format!("! {notice}"),
            Style::default().fg(Color::Red),
        )));
        lines.push(Line::from(""));
    }
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
                    .fg(crate::theme::Palette::SELECTED)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(crate::theme::Palette::PROSE)
            };
            lines.push(Line::from(Span::styled(format!("{prefix}{label}"), style)));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "💡 마우스 클릭 · ↑↓ 이동 · Enter 이 팩 불러오기 · d 팩 삭제 · Esc 취소",
        Style::default().fg(crate::theme::Palette::MUTED),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(crate::theme::BORDER_TYPE)
        .border_style(crate::theme::Styles::border_primary())
        .title(Span::styled(" ✦ 팩 선택 ✦ ", crate::theme::Styles::title()));
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
            Style::default().fg(crate::theme::Palette::MUTED),
        )),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(crate::theme::BORDER_TYPE)
        .border_style(Style::default().fg(crate::theme::Palette::DANGER))
        .title(Span::styled(
            " 팩 삭제 ",
            Style::default()
                .fg(crate::theme::Palette::DANGER)
                .add_modifier(Modifier::BOLD),
        ));
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use unicode_width::UnicodeWidthStr;

    fn draw_to_string(labels: &[String], selected: usize) -> String {
        draw_with(|frame, area| render_pack_picker(frame, area, labels, selected, None))
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

    #[test]
    fn picker_notice_renders_inside_the_modal() {
        let labels = vec!["비가 그친 뒤 · ko-KR (현재)".to_string()];
        let out = draw_with(|frame, area| {
            super::render_pack_picker(
                frame,
                area,
                &labels,
                0,
                Some("현재 플레이 중인 팩은 삭제할 수 없습니다."),
            )
        });
        assert!(
            out.contains("현재 플레이 중인 팩은 삭제할 수 없습니다."),
            "{out}"
        );
        assert!(out.contains("팩 선택"), "{out}");
    }
}
