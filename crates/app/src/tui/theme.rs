// palette is wired into pages in M3
#![allow(dead_code)]

use ratatui::style::Color;
use wntui_core::model::OnlineStatus;

pub const PRIMARY: Color = Color::Rgb(178, 132, 255);
pub const ACCENT: Color = Color::Rgb(147, 112, 219);
pub const DIM: Color = Color::Rgb(120, 90, 180);
pub const SUBTLE_BACKGROUND: Color = Color::Rgb(30, 22, 44);
pub const MUTED: Color = Color::Rgb(130, 125, 150);

// Detail-field affordances (review F6): blue = editable, purple = has a
// graph behind it. White text on both for contrast.
pub const EDITABLE_BG: Color = Color::Rgb(40, 62, 150);
pub const GRAPH_BG: Color = Color::Rgb(96, 64, 168);
pub const AFFORDANCE_FG: Color = Color::White;

pub const STATUS_ONLINE: Color = Color::Rgb(80, 200, 120);
pub const STATUS_UNCERTAIN: Color = Color::Rgb(230, 180, 60);
pub const STATUS_OFFLINE: Color = Color::Rgb(220, 70, 70);

pub fn status_color(status: OnlineStatus) -> Color {
    match status {
        OnlineStatus::Online => STATUS_ONLINE,
        OnlineStatus::Uncertain => STATUS_UNCERTAIN,
        OnlineStatus::Offline => STATUS_OFFLINE,
    }
}

pub fn status_glyph(status: OnlineStatus) -> &'static str {
    match status {
        OnlineStatus::Online => "●",
        OnlineStatus::Uncertain => "◐",
        OnlineStatus::Offline => "○",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_colors_match_official_ui_semantics() {
        assert_eq!(status_color(OnlineStatus::Online), STATUS_ONLINE);
        assert_eq!(status_color(OnlineStatus::Uncertain), STATUS_UNCERTAIN);
        assert_eq!(status_color(OnlineStatus::Offline), STATUS_OFFLINE);
    }

    #[test]
    fn glyphs_cover_every_status() {
        for status in [
            OnlineStatus::Online,
            OnlineStatus::Uncertain,
            OnlineStatus::Offline,
        ] {
            assert_eq!(status_glyph(status).chars().count(), 1);
        }
    }
}
