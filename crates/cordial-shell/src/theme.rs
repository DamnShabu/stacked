//! The game window's colours.
//!
//! **Stacked** is the default: Headcount's near-black neutrals with its red as
//! the accent, so the launcher and the game window it starts read as one
//! family, and libadwaita held in dark
//! mode so the area behind the canvas never flashes white while a resize
//! catches up. **System** is what this window did before the fork had a brand
//! -- the desktop's own colours, following its light and dark setting, with
//! nothing of Stacked's own applied.
//!
//! Chosen by `CORDIAL_THEME`, which the `stacked` launcher sets from its
//! config. An environment variable rather than a config read here because the
//! game window lives in `cordial-run`, which is handed its settings by the
//! launcher and reads no launcher config of its own, the same as
//! `CORDIAL_TITLE_BAR`.

use serde::{Deserialize, Serialize};

/// The environment variable the launcher sets and the window reads.
pub const THEME_ENV: &str = "CORDIAL_THEME";

/// The brand palette, taken from the icon and shared with Headcount.
///
/// Named by role rather than by colour so that a sheet written against them
/// still reads correctly if the palette is ever retuned. The values are
/// Headcount's own (its `DESIGN.md`), because Headcount is what installs and
/// starts this client and the two windows sit side by side on one desktop.
pub mod palette {
    /// The icon's background tile, and the window's darkest surface.
    pub const BASE: &str = "#121317";
    /// A step up from the base, for the header bar, so it reads as a surface
    /// rather than as a hole in the window. Headcount's sidebar.
    pub const SURFACE: &str = "#16171c";
    /// Hairlines: the bar's lower edge, the splash's outline.
    pub const MUTED: &str = "#30333b";
    /// A raised control's fill, and a hovered window control.
    pub const MID: &str = "#2a2d35";
    /// Accent: focus rings, the progress bar, highlighted text. The icon's
    /// one red block.
    pub const ACCENT: &str = "#ff3d2e";
    /// Text drawn on the accent.
    pub const ON_ACCENT: &str = "#140806";
    /// The icon's white blocks.
    pub const TILE: &str = "#ffffff";
    /// Text on the dark surfaces. Slightly cool, matching the neutrals, rather
    /// than pure white.
    pub const TEXT: &str = "#eceef2";
    /// Secondary text: the version beside the title, a step label.
    pub const TEXT_DIM: &str = "#7d818c";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Stacked,
    System,
}

impl Theme {
    /// Parse the value the launcher writes, or a user typed. Anything not
    /// recognised is the default rather than a refusal, because a typo in a
    /// colour preference is not a reason to not start a game.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "system" | "adwaita" | "desktop" => Self::System,
            _ => Self::Stacked,
        }
    }

    pub fn from_env() -> Self {
        std::env::var(THEME_ENV).map(|v| Self::parse(&v)).unwrap_or_default()
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stacked => "stacked",
            Self::System => "system",
        }
    }

    /// The stylesheet this theme adds over libadwaita's own, scoped to the
    /// game window's class so it can never restyle a web-view dialog that
    /// happens to share the display.
    ///
    /// libadwaita's named colours are redefined as well as the header bar being
    /// styled directly: the window controls, the text-entry fallback and any
    /// focus ring take their colours from those names, and styling only the bar
    /// left the close button's hover in Adwaita blue.
    pub fn css(self) -> String {
        use palette::*;
        match self {
            Self::System => String::new(),
            Self::Stacked => format!(
                "@define-color accent_color {ACCENT}; \
                 @define-color accent_bg_color {ACCENT}; \
                 @define-color accent_fg_color {ON_ACCENT}; \
                 @define-color window_bg_color {BASE}; \
                 @define-color window_fg_color {TEXT}; \
                 @define-color view_bg_color {BASE}; \
                 @define-color view_fg_color {TEXT}; \
                 @define-color headerbar_bg_color {SURFACE}; \
                 @define-color headerbar_fg_color {TEXT}; \
                 @define-color headerbar_backdrop_color {BASE}; \
                 .cordial-engine-host {{ background-color: {BASE}; color: {TEXT}; }} \
                 .cordial-engine-host headerbar {{ \
                     background-color: {SURFACE}; \
                     color: {TEXT}; \
                     box-shadow: inset 0 -1px alpha({MUTED}, 0.55); \
                 }} \
                 .cordial-engine-host headerbar:backdrop {{ background-color: {BASE}; }} \
                 .cordial-engine-host headerbar .stacked-title {{ color: {TEXT}; }} \
                 .cordial-engine-host headerbar .stacked-title-detail {{ color: {TEXT_DIM}; }} \
                 .cordial-engine-host headerbar windowcontrols button {{ \
                     background-color: transparent; color: {TEXT_DIM}; border-radius: 6px; \
                 }} \
                 .cordial-engine-host headerbar windowcontrols button > image {{ \
                     background: none; \
                 }} \
                 .cordial-engine-host headerbar windowcontrols button:hover {{ \
                     background-color: {MID}; color: {TEXT}; \
                 }} \
                 .cordial-engine-host headerbar windowcontrols button.close:hover {{ \
                     background-color: {ACCENT}; color: {ON_ACCENT}; \
                 }} \
                 .cordial-engine-host .cordial-text-fallback {{ \
                     background-color: alpha({BASE}, 0.96); \
                     color: {TEXT}; \
                     border: 1px solid {MUTED}; \
                 }}"
            ),
        }
    }

    /// Whether libadwaita should be held in dark mode. The Stacked palette is
    /// dark only; letting the desktop flip it to light would leave light
    /// window controls on a near-black bar.
    pub const fn forces_dark(self) -> bool {
        matches!(self, Self::Stacked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrecognised_values_fall_back_to_the_brand_rather_than_refusing() {
        assert_eq!(Theme::parse("system"), Theme::System);
        assert_eq!(Theme::parse("  System "), Theme::System);
        assert_eq!(Theme::parse("stacked"), Theme::Stacked);
        assert_eq!(Theme::parse("sytem"), Theme::Stacked);
        assert_eq!(Theme::parse(""), Theme::Stacked);
    }

    #[test]
    fn the_system_theme_adds_nothing_of_its_own() {
        assert!(Theme::System.css().is_empty());
        assert!(!Theme::System.forces_dark());
    }

    #[test]
    fn the_brand_sheet_is_scoped_to_the_game_window() {
        let css = Theme::Stacked.css();
        assert!(css.contains(palette::ACCENT));
        // Every rule that selects a widget must be under the host class, or a
        // web-view dialog on the same display is restyled with it.
        for rule in css.split('}').filter(|r| r.contains('{')) {
            let selector = rule.split('{').next().unwrap();
            let selector = selector.rsplit(';').next().unwrap().trim();
            assert!(
                selector.starts_with(".cordial-engine-host"),
                "unscoped selector: {selector}"
            );
        }
    }

    #[test]
    fn the_value_round_trips_through_config_and_environment() {
        for theme in [Theme::Stacked, Theme::System] {
            assert_eq!(Theme::parse(theme.as_str()), theme);
            let stored = serde_json::to_string(&theme).unwrap();
            assert_eq!(stored, format!("\"{}\"", theme.as_str()));
        }
    }
}
