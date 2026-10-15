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
//! **What has been measured, on the logged-out landing page with `FLogFRM=7`,
//! and what has not.** `DFIntDebugFRMQualityLevelOverride` acts: without it
//! the engine's FRM controller logs `FastBackoff, reducing quality` and steps
//! 21 to 19 within seconds while chasing a 165 Hz frame-rate target, and with
//! it set the controller logs no status and no step at all, in alternating
//! runs from the same cleared state. Whether the MSAA and texture overrides
//! change a 3D frame has not been measured; the landing page is 2D and an
//! engine-scaled 3D scene is not on it, so neither is a picture of one. Every
//! name below is in `libroblox.so`; a name the engine does not know is
//! accepted and ignored, which is how `RenderShadowIntensity`, `DisablePostFx`
//! and `RenderGrassDetailStrands` sat in the first version of this table doing
//! nothing.
//!
//! **Max also turns dynamic resolution off, and that half is `INFERRED`.** The
//! symptom was a 3D scene drawn soft at a native 1920x1080 swapchain. The
//! engine's `DRS init:` log line reads `canUseHighDPIFullFB=true` only when
//! automatic DRS is rolled out to the client, and on the account that showed
//! the symptom Roblox rolls it out through an experiment assignment
//! (`IxpSettings.json`, `AutomaticDRSHundredthPercent` = 10000). That
//! assignment beats the settings document: `DFIntAutomaticDRSHundredthPercent`
//! = 10000 turns the line on for a client with no assignment, and = 0 does not
//! turn it off for one that has it, five runs of five. So the rollout cannot be
//! withdrawn from here, and Max sets the switches downstream of it instead.
//! None of them shows in any log line a signed-out client produces, so none is
//! measured; see `quality_flags`.

use std::collections::BTreeMap;

use crate::flags::{Layer, Source};

/// The environment variable a launcher sets: `low`, `medium`, `high` or
/// `max`. Absent, or `high`, sets nothing.
pub const QUALITY_ENV: &str = "CORDIAL_QUALITY";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    /// The cheapest frame the engine will draw: lowest render quality, no
    /// anti-aliasing, smallest textures, no grass.
    Low,
    /// A middle render quality with no anti-aliasing and reduced textures.
    Medium,
    /// The engine's own choice. Sets nothing.
    #[default]
    High,
    /// The top render quality, 4x anti-aliasing, full textures and no
    /// dynamic resolution scaling.
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
            ("FIntFRMMinGrassDistance", "0"),
            ("FIntFRMMaxGrassDistance", "0"),
        ],
        Quality::Medium => vec![
            ("DFIntDebugFRMQualityLevelOverride", "8"),
            ("FIntDebugForceMSAASamples", "0"),
            ("DFFlagTextureQualityOverrideEnabled", "True"),
            ("DFIntTextureQualityOverride", "1"),
        ],
        Quality::High => Vec::new(),
        Quality::Max => vec![
            ("DFIntDebugFRMQualityLevelOverride", "21"),
            ("FIntDebugForceMSAASamples", "4"),
            ("DFFlagTextureQualityOverrideEnabled", "True"),
            ("DFIntTextureQualityOverride", "3"),
            // The rollout itself, for a client Roblox has not enrolled by
            // experiment, so a later server-side rollout cannot start scaling
            // a client the user asked to draw at its best. `DFInt` is
            // measured: the server document's `FIntAutomaticDRSHundredthPercent`
            // is a dead name on this build and moved nothing, the `DFInt`
            // spelling did.
            ("DFIntAutomaticDRSHundredthPercent", "0"),
            // `INFERRED`, all three. `DFFlag` for `AutomaticDRS` is read off
            // the `DFFlagAutomaticDRS_IXP` key Roblox's own document carries;
            // `DebugDisableDRS` is in the binary under a prefix nothing here
            // has shown, so both spellings go in and the wrong one is ignored.
            ("DFFlagAutomaticDRS", "False"),
            ("FFlagDebugDisableDRS", "True"),
            ("DFFlagDebugDisableDRS", "True"),
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
    fn only_max_turns_dynamic_resolution_off() {
        let has = |q, key: &str, value: &str| {
            quality_flags(q).into_iter().any(|(k, v)| k == key && v == value)
        };
        assert!(has(Quality::Max, "DFFlagDebugDisableDRS", "True"));
        assert!(has(Quality::Max, "DFIntAutomaticDRSHundredthPercent", "0"));
        for q in [Quality::Low, Quality::Medium] {
            assert!(!quality_flags(q).into_iter().any(|(k, _)| k.contains("DRS")));
        }
    }

    /// Names checked against `libroblox.so` 2.742 and absent from it. The
    /// engine accepts and ignores an unknown name, so nothing else would ever
    /// say one of these had crept back in.
    #[test]
    fn no_level_carries_a_name_the_engine_does_not_have() {
        let dead = [
            "FIntRenderShadowIntensity",
            "FFlagDisablePostFx",
            "FIntRenderGrassDetailStrands",
            "FIntAutomaticDRSHundredthPercent",
        ];
        for q in [Quality::Low, Quality::Medium, Quality::Max] {
            for (k, _) in quality_flags(q) {
                assert!(!dead.contains(&k), "{k} is not in the engine");
            }
        }
    }

    #[test]
    fn no_level_sets_a_flag_twice() {
        for q in [Quality::Low, Quality::Medium, Quality::Max] {
            let flags = quality_flags(q);
            let keys: std::collections::BTreeSet<_> = flags.iter().map(|(k, _)| *k).collect();
            assert_eq!(keys.len(), flags.len(), "{q:?}");
        }
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
