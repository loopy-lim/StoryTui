//! Transparent-first theme and UI styling helpers for StoryTUI.
//!
//! Designed specifically for terminals with transparency/blur enabled:
//! no styles set an opaque background (`bg`), ensuring the underlying
//! desktop or wallpaper remains visible through the terminal.

use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::BorderType;
use story_model::RiskLevel;

/// Semantic color palette for transparent terminals.
pub struct Palette;

impl Palette {
    /// Primary highlight color (soft sky blue).
    pub const PRIMARY: Color = Color::Rgb(110, 200, 245);
    /// Secondary accent color (warm amber/gold).
    pub const ACCENT: Color = Color::Rgb(250, 200, 90);
    /// Narrative prose body text (clear light gray).
    pub const PROSE: Color = Color::Rgb(215, 220, 230);
    /// Spoken dialogue text (warm soft ivory).
    pub const DIALOGUE: Color = Color::Rgb(255, 245, 210);
    /// Speaker name label (gentle mint/cyan).
    pub const SPEAKER: Color = Color::Rgb(130, 225, 215);
    /// Subtle muted guide/hint color (dim gray).
    pub const MUTED: Color = Color::DarkGray;
    /// Secondary muted text.
    pub const SUBTLE: Color = Color::Gray;
    /// Safe action (emerald green).
    pub const SAFE: Color = Color::Rgb(100, 220, 140);
    /// Normal risk action (warm amber).
    pub const CAUTION: Color = Color::Rgb(245, 185, 75);
    /// High danger / critical risk action (soft coral red).
    pub const DANGER: Color = Color::Rgb(250, 115, 115);
    /// Active selection outline/accent.
    pub const SELECTED: Color = Color::Rgb(255, 215, 75);
}

/// Global UI border type preference: rounded corners look modern and sleek on transparent terminals.
pub const BORDER_TYPE: BorderType = BorderType::Rounded;

/// Styles that never set a solid background, preserving terminal transparency.
pub struct Styles;

impl Styles {
    /// Screen title style.
    #[inline]
    pub fn title() -> Style {
        Style::default()
            .fg(Palette::PRIMARY)
            .add_modifier(Modifier::BOLD)
    }

    /// Primary border style.
    #[inline]
    pub fn border_primary() -> Style {
        Style::default().fg(Palette::PRIMARY)
    }

    /// Subtle/muted border style.
    #[inline]
    pub fn border_muted() -> Style {
        Style::default().fg(Palette::MUTED)
    }

    /// Active/focused border style.
    #[inline]
    pub fn border_selected() -> Style {
        Style::default()
            .fg(Palette::SELECTED)
            .add_modifier(Modifier::BOLD)
    }

    /// Selected item text style (using subtle bold + accent rather than harsh invert).
    #[inline]
    pub fn item_selected() -> Style {
        Style::default()
            .fg(Palette::SELECTED)
            .add_modifier(Modifier::BOLD)
    }

    /// Unselected item text style.
    #[inline]
    pub fn item_normal() -> Style {
        Style::default().fg(Palette::PROSE)
    }

    /// Button keycap badge (e.g. `[ Enter ]`, `[ S ]`).
    #[inline]
    pub fn keycap() -> Style {
        Style::default()
            .fg(Palette::PRIMARY)
            .add_modifier(Modifier::BOLD)
    }

    /// Button label text.
    #[inline]
    pub fn button_label() -> Style {
        Style::default().fg(Palette::SUBTLE)
    }

    /// Risk level indicator style.
    #[inline]
    pub fn risk_style(risk: RiskLevel) -> Style {
        let color = match risk {
            RiskLevel::Safe => Palette::SAFE,
            RiskLevel::Low => Palette::PRIMARY,
            RiskLevel::Medium => Palette::CAUTION,
            RiskLevel::High | RiskLevel::Severe => Palette::DANGER,
        };
        Style::default().fg(color)
    }
}

/// Helper to render a compact visual gauge bar (e.g. `[■■■□□] 6/10`).
pub fn render_gauge_bar(value: i16, max: i16, bar_width: usize) -> String {
    if max <= 0 || bar_width == 0 {
        return String::new();
    }
    let clamped_val = value.clamp(0, max) as usize;
    let max_val = max as usize;
    // Round to nearest filled block
    let filled_count = ((clamped_val * bar_width * 2 + max_val) / (max_val * 2)).min(bar_width);
    let empty_count = bar_width.saturating_sub(filled_count);
    format!("{}{}", "■".repeat(filled_count), "□".repeat(empty_count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauge_bar_bounds() {
        assert_eq!(render_gauge_bar(0, 10, 5), "□□□□□");
        assert_eq!(render_gauge_bar(5, 10, 5), "■■■□□");
        assert_eq!(render_gauge_bar(10, 10, 5), "■■■■■");
        assert_eq!(render_gauge_bar(12, 10, 5), "■■■■■"); // clamped
    }
}
