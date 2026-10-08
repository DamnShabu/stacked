//! A launcher's choice of graphics quality, as a FastFlag layer.
//!
//! A launcher running several clients at once -- the Roblox manager's
//! performance levels are the caller this was written for -- needs one client
//! drawn cheaply and the next at its best, and each profile's `flags.json` is
//! the user's own file, which a launcher should not be writing into. So the
//! choice is passed the way `CORDIAL_FPS_CAP` is: as the environment of the
//! one process it applies to, read before the settings document is built,
//! layered above plugins and below the user's file.
//!
//! **`INFERRED` that these flags change what a game costs.** Every name is one
//! the Windows client's community presets have used for years, but
//! `docs/fastflags.md` records that `DebugFRMQualityLevelOverride` and the MSAA
//! overrides changed nothing measurable on the logged-out landing page, which
//! is 2D; nobody has measured them in a 3D experience on this engine. A name
//! the engine does not know is ignored, so the worst a wrong one does is
//! nothing.

use std::collections::BTreeMap;

use crate::flags::{Layer, Source};

/// The environment variable a launcher sets: `low`, `medium`, `high` or
/// `max`. Absent, or `high`, sets nothing.
pub const QUALITY_ENV: &str = "CORDIAL_QUALITY";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    /// The cheapest frame the engine will draw: lowest render quality, no
    /// anti-aliasing, smallest textures, no shadows, post-processing or grass.
    Low,
    /// A middle render quality with no anti-aliasing and reduced textures.
    Medium,
    /// The engine's own choice. Sets nothing.
    #[default]
    High,
    /// The top render quality, 4x anti-aliasing and full textures.
    Max,
}

impl Quality {
    pub fn parse(text: &str) -> Option<Quality> {
        match text.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Quality::Low),
            "medium" => Some(Quality::Medium),
            "" | "high" => Some(Quality::High),
            "max" => Some(Quality::Max),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::Low => "low",
            Quality::Medium => "medium",
            Quality::High => "high",
            Quality::Max => "max",
        }
    }
}

/// The flags a quality asks for. Pure, so the table can be tested.
pub fn quality_flags(quality: Quality) -> Vec<(&'static str, &'static str)> {
    match quality {
        Quality::Low => vec![
            ("DFIntDebugFRMQualityLevelOverride", "1"),
            ("FIntDebugForceMSAASamples", "0"),
            ("DFFlagTextureQualityOverrideEnabled", "True"),
            ("DFIntTextureQualityOverride", "0"),
            ("FIntRenderShadowIntensity", "0"),
            ("FFlagDisablePostFx", "True"),
            ("FIntFRMMinGrassDistance", "0"),
            ("FIntFRMMaxGrassDistance", "0"),
            ("FIntRenderGrassDetailStrands", "0"),
        ],
        Quality::Medium => vec![
            ("DFIntDebugFRMQualityLevelOverride", "8"),
            ("FIntDebugForceMSAASamples", "0"),
            ("DFFlagTextureQualityOverrideEnabled", "True"),
            ("DFIntTextureQualityOverride", "1"),
            ("FFlagDisablePostFx", "True"),
        ],
        Quality::High => Vec::new(),
        Quality::Max => vec![
            ("DFIntDebugFRMQualityLevelOverride", "21"),
            ("FIntDebugForceMSAASamples", "4"),
            ("DFFlagTextureQualityOverrideEnabled", "True"),
            ("DFIntTextureQualityOverride", "3"),
        ],
    }
}

/// Which quality the launcher asked for. An unparseable value is said and
/// treated as `high`, like an unknown performance mode, rather than ignored
/// in silence.
pub fn quality() -> Quality {
    let Ok(text) = std::env::var(QUALITY_ENV) else {
        return Quality::High;
    };
    Quality::parse(&text).unwrap_or_else(|| {
        println!(
            "  flags: {QUALITY_ENV}={text:?} is not a graphics quality; leaving the engine's \
             own. Known: low, medium, high, max"
        );
        Quality::High
    })
}

/// The layer [`quality`] contributes, empty on `high`.
pub fn layer() -> Layer {
    let q = quality();
    let values: BTreeMap<String, String> = quality_flags(q)
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    if !values.is_empty() {
        println!(
            "  flags: graphics quality {} sets {} flag(s)",
            q.label(),
            values.len()
        );
    }
    Layer {
        source: Source::Launcher,
        values,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_and_absent_leave_the_engine_alone() {
        assert!(quality_flags(Quality::High).is_empty());
        assert_eq!(Quality::parse(""), Some(Quality::High));
        assert_eq!(Quality::default(), Quality::High);
    }

    #[test]
    fn every_level_parses_by_its_own_name_and_nothing_else_does() {
        for q in [Quality::Low, Quality::Medium, Quality::High, Quality::Max] {
            assert_eq!(Quality::parse(&q.label().to_uppercase()), Some(q));
        }
        assert_eq!(Quality::parse("ultra"), None);
    }

    #[test]
    fn the_levels_order_the_render_quality() {
        let level = |q| {
            quality_flags(q)
                .into_iter()
                .find(|(k, _)| *k == "DFIntDebugFRMQualityLevelOverride")
                .map(|(_, v)| v.parse::<u32>().unwrap())
        };
        assert!(level(Quality::Low) < level(Quality::Medium));
        assert!(level(Quality::Medium) < level(Quality::Max));
    }

    #[test]
    fn every_value_has_the_shape_its_prefix_wants() {
        for q in [Quality::Low, Quality::Medium, Quality::Max] {
            for (k, v) in quality_flags(q) {
                if k.starts_with("FFlag") || k.starts_with("DFFlag") {
                    assert!(v == "True" || v == "False", "{k}={v}");
                } else {
                    assert!(v.parse::<i64>().is_ok(), "{k}={v}");
                }
            }
        }
    }
}
