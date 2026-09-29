//! The game window's colours.
//!
//! **Stacked** is the default: a dark header bar in the brand's brown with the
//! amber of the icon's front tile as its accent, and libadwaita held in dark
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

/// The brand palette, taken from the icon.
///
/// Named by role rather than by colour so that a sheet written against them
/// still reads correctly if the palette is ever retuned.
pub mod palette {
    /// The icon's background tile. The window's darkest surface.
    pub const BASE: &str = "#2a1f0c";
    /// Somewhere between the base and the back tile, for the header bar, so it
    /// reads as a surface rather than as a hole in the window.
    pub const SURFACE: &str = "#3a2c14";
    /// The back tile.
    pub const MUTED: &str = "#6b5634";
    /// The middle tile.
    pub const MID: &str = "#b08c4c";
    /// The front tile. Accent, focus rings and highlighted text.
    pub const ACCENT: &str = "#e9bd6a";
    /// Text on the dark surfaces. Warm rather than pure white, which on this
    /// brown reads as blue by contrast.
    pub const TEXT: &str = "#f5ead3";
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
                 @define-color accent_bg_color {MID}; \
                 @define-color accent_fg_color {BASE}; \
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
                     box-shadow: inset 0 -1px {MUTED}; \
                 }} \
                 .cordial-engine-host headerbar:backdrop {{ background-color: {BASE}; }} \
                 .cordial-engine-host headerbar .title {{ font-weight: 700; }} \
                 .cordial-engine-host headerbar windowcontrols button:hover {{ \
                     background-color: alpha({ACCENT}, 0.18); \
                 }} \
                 .cordial-engine-host .cordial-text-fallback {{ \
                     background-color: alpha({BASE}, 0.96); \
                     color: {TEXT}; \
                     border: 1px solid {MID}; \
                 }}"
            ),
        }
    }

    /// Whether libadwaita should be held in dark mode. The Stacked palette is
    /// dark only; letting the desktop flip it to light would leave light
    /// window controls on a brown bar.
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
