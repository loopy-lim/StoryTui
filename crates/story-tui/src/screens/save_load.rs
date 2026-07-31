//! Save/Load slot screen renderer (W8, ST-028).
//!
//! Renders the list of [`SlotMeta`] entries from a [`SaveStore`](../../story_persistence/struct.SaveStore.html)
//! into a ratatui `List`. The screen is intentionally light: the gate is the
//! persistence round-trip + fault-injection tests, not UI polish. Navigation
//! keys (up/down/enter) are handled by the host app's key dispatch; this
//! module only renders.
//!
//! The view model is the slice of `SlotMeta` plus a title and a selected
//! index — the host assembles them from `SaveStore::list_slots()`.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListState, Paragraph, Wrap};
use story_persistence::SlotMeta;

/// Minimum supported terminal size for the SaveLoad screen.
pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 12;

/// Render the SaveLoad screen.
///
/// - `title`: page heading (e.g. `"저장 / 불러오기"`).
/// - `slots`: the slot list (in display order — the host pre-sorts).
/// - `selected`: index of the highlighted slot (clamped to the slot count).
/// - `footer`: small hint line (e.g. `"↑↓ 이동 · Enter 불러오기 · Del 삭제 · Esc 닫기"`).
pub fn render_save_load(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    slots: &[SlotMeta],
    selected: usize,
    footer: &str,
) {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_min_size_warning(frame, area);
        return;
    }

    let chunks = Layout::vertical([
        Constraint::Length(2), // title
        Constraint::Min(3),    // list
        Constraint::Length(2), // footer
    ])
    .split(area);

    // Title bar.
    let title_line = Line::from(vec![Span::styled(
        title.to_string(),
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::Cyan),
    )]);
    let title_block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray));
    let title_para = Paragraph::new(title_line)
        .block(title_block)
        .alignment(Alignment::Center);
    frame.render_widget(title_para, chunks[0]);

    // Slot list.
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            "슬롯",
            Style::default().add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(chunks[1]);
    frame.render_widget(block, chunks[1]);

    let items: Vec<Line> = if slots.is_empty() {
        vec![Line::from(Span::styled(
            "저장된 슬롯이 없습니다.".to_string(),
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        slots
            .iter()
            .map(|s| {
                let desc = s
                    .description
                    .clone()
                    .unwrap_or_else(|| "(설명 없음)".to_string());
                let when = s.saved_at.format("%Y-%m-%d %H:%M").to_string();
                Line::from(vec![
                    Span::raw(format!("[{}] ", s.slot_id)),
                    Span::raw(s.story_id.clone()),
                    Span::raw(" · "),
                    Span::raw(desc),
                    Span::raw(" · "),
                    Span::styled(when, Style::default().fg(Color::DarkGray)),
                ])
            })
            .collect()
    };

    let list = List::new(items)
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Yellow)
                .add_modifier(Modifier::REVERSED),
        )
        .highlight_symbol("▶ ");

    let mut state = ListState::default();
    if !slots.is_empty() {
        state.select(Some(selected.min(slots.len() - 1)));
    }
    frame.render_stateful_widget(list, inner, &mut state);

    // Footer hint.
    let footer_para = Paragraph::new(Line::from(Span::styled(
        footer.to_string(),
        Style::default().fg(Color::DarkGray),
    )))
    .wrap(Wrap { trim: false });
    frame.render_widget(footer_para, chunks[2]);
}

fn render_min_size_warning(frame: &mut Frame, area: Rect) {
    let msg = format!(
        "터미널이 너무 작습니다 (최소 {MIN_WIDTH}x{MIN_HEIGHT}, 현재 {}x{})",
        area.width, area.height
    );
    let para = Paragraph::new(Line::from(msg))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Yellow));
    frame.render_widget(para, area);
}
