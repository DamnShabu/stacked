//! What Cordial tells the engine about the display's refresh.
//!
//! The engine takes two things:
//!
//! ```text
//! nativePassSupportedRefreshRates([F)V     every rate it may be asked to run at
//! nativePassCurrentDisplayRefreshRate(F)V  the one in force now
//! ```
//!
//! **Cordial had never called either, and now calls both.** For most of this
//! module's life the engine ran with whatever it assumes when the application
//! says nothing, and AGENTS.md's note that the frame rate is a hard FIFO vsync
//! lock to the output's refresh — 60 Hz gives 60, a 50 Hz monitor gives 49.4
//! even in fullscreen — was written against that silence. `bin/load.rs`'s
//! `wire_refresh_rate` now resolves both natives and calls them: once at
//! startup and again whenever GDK's monitor list changes (a hotplug, or the
//! window landing on a different output), using the policy this module
//! decides and `crate::android::wayland::current()` for which output the
//! engine's own window is mostly on. **Whether telling it changes the frame
//! rate is still untested** — `wire_refresh_rate`'s own doc comment says so in
//! full and marks the two-monitor path `UNVERIFIED` pending a machine that has
//! one.
//!
//! ## A window can be on two outputs at once, and Wayland means it
//!
//! `wl_surface.enter` fires once per output a surface overlaps, so "the
//! monitor" is not a well-defined thing to ask for. Dragging a window across a
//! boundary genuinely puts it on both, sometimes for as long as the user leaves
//! it there.
//!
//! The engine's interface does not admit that: `nativePassCurrentDisplayRefreshRate`
//! takes one float. So a choice has to be made, and it is made here rather than
//! left to whichever call site happens to run:
//!
//! * **Supported** is the union across every output the display has, not just the
//!   ones the window is on. The window can be moved, and a list that shrank when
//!   it crossed a boundary would make the engine renegotiate for no reason.
//! * **Current** is the rate of the output the window is *most* on, which is what
//!   GDK's `monitor_at_surface` already answers and, on the compositors that
//!   matter, is the one driving the frame callbacks.
//!
//! **Untested, and stated so it is not mistaken for measured:** whether a window
//! spanning a 60 Hz and a 144 Hz output actually presents at the rate GDK names.
//! It is the reasonable reading of how compositors schedule, and this project has
//! been wrong before about exactly this class of thing. If it turns out a
//! spanning window is paced by the slower output, `current_for` is the one
//! function that has to change.

/// One output, as much of it as this decision needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Output {
    /// Refresh in hertz. GDK reports millihertz; convert before constructing.
    pub hz: f32,
    /// Whether the window is mostly on this one.
    pub current: bool,
}

/// Rates below this are a bug in the source rather than a slow monitor.
///
/// GDK returns 0 for an output whose mode it does not know, and a zero told to
/// the engine is not "unknown", it is "no frames". Dropped rather than passed on.
const MIN_PLAUSIBLE_HZ: f32 = 20.0;

/// Rates above this are not a display mode.
const MAX_PLAUSIBLE_HZ: f32 = 1000.0;

fn plausible(hz: f32) -> bool {
    hz.is_finite() && (MIN_PLAUSIBLE_HZ..=MAX_PLAUSIBLE_HZ).contains(&hz)
}

/// Every rate worth offering, ascending and without duplicates.
///
/// Deduplicated because two identical monitors are the common desk setup and
/// handing the engine `[60.0, 60.0]` invites it to treat them as choices.
/// Compared at a hundredth of a hertz: GDK's millihertz for a 59.94 mode comes
/// back as 59940, and rounding to whole numbers would merge 59.94 with 60.0,
/// which are genuinely different modes.
pub fn supported_from(outputs: &[Output]) -> Vec<f32> {
    let mut rates: Vec<f32> = outputs.iter().map(|o| o.hz).filter(|hz| plausible(*hz)).collect();
    rates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    rates.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    rates
}

/// The rate to report as in force.
///
/// `None` when nothing plausible is known, which the caller must treat as "say
/// nothing" rather than as a number. Telling the engine 0 would be worse than
/// the silence this module exists to end.
pub fn current_for(outputs: &[Output]) -> Option<f32> {
    outputs
        .iter()
        .find(|o| o.current && plausible(o.hz))
        .or_else(|| outputs.iter().find(|o| plausible(o.hz)))
        .map(|o| o.hz)
}

/// GDK reports refresh in millihertz; the engine wants hertz.
///
/// Its own "unknown" is 0, which [`plausible`] then rejects — so an output whose
/// mode GDK cannot read drops out rather than arriving as a stopped display.
pub fn hz_from_millihertz(mhz: i32) -> f32 {
    mhz as f32 / 1000.0
}

/// Whether a change is worth telling the engine about.
///
/// Monitor notifications fire on far more than a rate change — a scale factor,
/// a geometry nudge, a hotplug elsewhere — and re-announcing an unchanged rate
/// makes the engine renegotiate for nothing. The comparison is the same
/// hundredth-hertz one `supported_from` dedupes with, so a 59.94 output does not
/// oscillate against 60.0.
pub fn worth_announcing(previous: Option<f32>, now: Option<f32>) -> bool {
    match (previous, now) {
        (Some(a), Some(b)) => (a - b).abs() >= 0.01,
        (None, Some(_)) => true,
        // Losing the rate is not news: the engine keeps running at whatever it
        // last had, and there is no float that means "I no longer know".
        (_, None) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(hz: f32, current: bool) -> Output {
        Output { hz, current }
    }

    #[test]
    fn supported_is_the_union_across_every_output() {
        let outs = [out(60.0, true), out(144.0, false), out(120.0, false)];
        assert_eq!(supported_from(&outs), vec![60.0, 120.0, 144.0]);
    }

    #[test]
    fn two_identical_monitors_offer_one_rate() {
        assert_eq!(supported_from(&[out(60.0, true), out(60.0, false)]), vec![60.0]);
    }

    /// 59.94 and 60.0 are different modes and must not be merged. This is why
    /// the comparison is a hundredth of a hertz rather than a whole one.
    #[test]
    fn a_ntsc_rate_is_not_merged_with_sixty() {
        let r = supported_from(&[out(59.94, true), out(60.0, false)]);
        assert_eq!(r.len(), 2, "{r:?}");
    }

    /// The case that prompted this: a window straddling two outputs. The engine
    /// takes one float, so the one it is told is the output the window is most
    /// on.
    #[test]
    fn a_window_spanning_two_outputs_reports_the_one_it_is_mostly_on() {
        let outs = [out(60.0, false), out(144.0, true)];
        assert_eq!(current_for(&outs), Some(144.0));
        // ...and both rates stay on offer, because the window can be dragged
        // wholly onto either.
        assert_eq!(supported_from(&outs), vec![60.0, 144.0]);
    }

    /// GDK says 0 for an output whose mode it cannot read. Zero is not a slow
    /// display, and passing it on would tell the engine to stop.
    #[test]
    fn an_unknown_mode_is_dropped_rather_than_reported_as_zero() {
        assert_eq!(supported_from(&[out(0.0, true), out(60.0, false)]), vec![60.0]);
        assert_eq!(current_for(&[out(0.0, true), out(60.0, false)]), Some(60.0));
        assert_eq!(current_for(&[out(0.0, true)]), None);
    }

    #[test]
    fn nonsense_rates_do_not_reach_the_engine() {
        assert!(supported_from(&[out(f32::NAN, true), out(-60.0, false), out(5000.0, false)])
            .is_empty());
    }

    #[test]
    fn millihertz_converts_and_an_unknown_mode_stays_unknown() {
        assert_eq!(hz_from_millihertz(60000), 60.0);
        assert_eq!(hz_from_millihertz(59940), 59.94);
        assert!(!plausible(hz_from_millihertz(0)));
    }

    #[test]
    fn only_a_real_change_is_announced() {
        assert!(worth_announcing(None, Some(60.0)));
        assert!(worth_announcing(Some(60.0), Some(144.0)));
        assert!(!worth_announcing(Some(60.0), Some(60.0)));
        // A scale-factor notification that leaves 59.94 alone must not fire.
        assert!(!worth_announcing(Some(59.94), Some(59.94)));
        // 59.94 against 60.0 is a real change and must fire.
        assert!(worth_announcing(Some(59.94), Some(60.0)));
        // Losing the rate tells the engine nothing it can use.
        assert!(!worth_announcing(Some(60.0), None));
    }
}
