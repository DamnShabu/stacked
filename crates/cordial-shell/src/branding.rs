//! The name and icon the window wears.
//!
//! **Stacked**: three tilted tiles in brown and amber on a dark tile. The
//! palette lives in [`crate::theme::palette`], so the icon and the window's
//! colours cannot drift apart.
//!
//! This used to choose between two faces by date -- Cordial, and an inverted
//! "Frostbite" on two days a year. The joke was an arithmetic inversion of
//! Cordial's gradient and has no equivalent for this palette, so it went with
//! the rename rather than being kept pointing at an icon that no longer exists.

/// The application id, in the freedesktop sense: the desktop entry's file
/// name, the icon's name, the metainfo id and the Flatpak ref all use it, and
/// they must agree or the icon silently fails to resolve.
///
/// A new id rather than Cordial's, because this is a fork that can be
/// installed beside Cordial, and two applications exporting one id overwrite
/// each other's desktop entry and icon.
pub const APP_ID: &str = "io.github.damnshabu.Stacked";

/// The name the window, the task switcher and the launcher's output use.
pub const NAME: &str = "Stacked";

/// The freedesktop icon name. The same as [`APP_ID`], which is what Flatpak
/// requires of an exported icon.
pub const ICON: &str = APP_ID;

#[cfg(test)]
mod tests {
    use super::*;

    fn icon_path() -> String {
        format!(
            "{}/../../packaging/icons/hicolor/scalable/apps/{ICON}.svg",
            env!("CARGO_MANIFEST_DIR"),
        )
    }

    #[test]
    fn the_icon_is_actually_installed() {
        // The failure this prevents is a blank window in the task switcher.
        let path = icon_path();
        assert!(std::path::Path::new(&path).exists(), "{ICON} is named but not installed at {path}");
    }

    /// **The icon must be square, or the Flatpak will not export.**
    ///
    /// `flatpak build-export` refuses a non-square icon outright -- "Expected a
    /// square icon but got: 680x480" -- and it does so at the very last step,
    /// after the whole Rust build has succeeded. A test costing microseconds
    /// should catch that instead of an eight-minute CI run.
    #[test]
    fn the_icon_is_square_or_the_flatpak_export_refuses_it() {
        let svg = std::fs::read_to_string(icon_path()).expect("icon is readable");
        let head = &svg[..svg.len().min(600)];
        let attr = |name: &str| -> Option<f64> {
            let at = head.find(&format!("{name}=\""))? + name.len() + 2;
            head[at..].split('"').next()?.trim().parse().ok()
        };
        let (w, h) = (attr("width"), attr("height"));
        assert!(w.is_some(), "width must be set");
        assert_eq!(w, h, "width and height must be equal, got {w:?}x{h:?}");

        // The viewBox has to be square too: a square width/height over a
        // wide box still renders letterboxed, and it is the box
        // `build-export` measures.
        let vb: Vec<f64> = head
            .split("viewBox=\"")
            .nth(1)
            .expect("a viewBox")
            .split('"')
            .next()
            .unwrap()
            .split_whitespace()
            .filter_map(|n| n.parse().ok())
            .collect();
        assert_eq!(vb.len(), 4, "viewBox needs four numbers");
        assert_eq!(vb[2], vb[3], "viewBox must be square, got {}x{}", vb[2], vb[3]);
    }

    #[test]
    fn the_icon_is_drawn_in_the_theme_palette() {
        // One palette, two consumers. If the window's colours are retuned and
        // the icon is not, or the other way round, this says so.
        let svg = std::fs::read_to_string(icon_path()).expect("icon is readable");
        for colour in [
            crate::theme::palette::BASE,
            crate::theme::palette::MUTED,
            crate::theme::palette::MID,
            crate::theme::palette::ACCENT,
        ] {
            assert!(svg.contains(colour), "icon does not use {colour}");
        }
    }
}
