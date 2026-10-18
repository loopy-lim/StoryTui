//! Shared modal geometry and min-size warnings.
//!
//! Every centered popup (greeting/pack/import pickers, error overlay) uses
//! [`centered_rect`] so margins and clamping behave identically across
//! screens, and every screen shows the same Korean warning below its minimum
//! supported size.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

/// Popup widths, in terminal columns. One shared scale keeps the modals from
/// drifting apart (they used to be 60/70/72 ad-hoc).
pub const POPUP_WIDTH_NARROW: u16 = 60;
pub const POPUP_WIDTH_MEDIUM: u16 = 70;
pub const POPUP_WIDTH_WIDE: u16 = 76;

/// A centered rect of at most `width`×`height`, clamped to `area`.
pub fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
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

/// The one Korean "terminal too small" warning every screen renders below its
/// supported size, with the minimum and the current size spelled out.
pub fn render_min_size_warning(frame: &mut Frame, area: Rect, min_width: u16, min_height: u16) {
    let msg = format!(
        "터미널이 너무 작습니다 (최소 {min_width}x{min_height}, 현재 {}x{})",
        area.width, area.height
    );
    let para = Paragraph::new(Line::from(msg))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Yellow));
    frame.render_widget(para, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_rect_clamps_to_area() {
        let area = Rect::new(0, 0, 80, 24);
        let popup = centered_rect(area, 100, 40);
        assert_eq!(popup.width, 80);
        assert_eq!(popup.height, 24);
        assert_eq!(popup.x, 0);
        assert_eq!(popup.y, 0);

        let popup = centered_rect(area, 60, 8);
        assert_eq!(popup.width, 60);
        assert_eq!(popup.height, 8);
        assert_eq!(popup.x, 10);
        assert_eq!(popup.y, 8);
    }
}
