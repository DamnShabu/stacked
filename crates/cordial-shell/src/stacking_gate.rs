//! When the engine's canvas may be lowered under the GTK window.
//!
//! Lowering the canvas is how a text editor or a web-view dialog gets drawn
//! over the game, and it is only correct if the compositor already holds a GTK
//! buffer whose canvas rectangle is transparent and which contains the thing
//! to be seen. Lowering it any earlier shows the compositor's *last* GTK
//! buffer, which was painted while the canvas was above and is opaque all the
//! way across: a flat grey screen, for as long as GTK goes on not presenting.
//!
//! **That is issue #53, and the old ordering could not tell it was happening.**
//! `repaint_now` waited up to 40 ms for a frame, logged that none had come, and
//! the restack went out anyway. On the reporter's KWin session GTK started a
//! frame after every one of those timeouts and never attached a buffer to the
//! window surface for the whole of the 2 seconds the box was focused, while the
//! canvas attached 84 to 139. The rule here is the one the log asks for: no
//! committed GTK frame, no lowering. If none arrives the canvas stays above --
//! the editor is then invisible, which is a smaller failure than a grey screen
//! -- and the attempt is repeated rather than given up on.
//!
//! This module is only the decision, so that "never lower without a frame" is
//! a test and not a comment. What counts as a committed frame, and how to ask
//! for one, is `HostWindow`'s business.

use std::time::{Duration, Instant};

/// How long one attempt waits for GTK to commit a frame.
///
/// A healthy compositor presents within a couple of refresh intervals; the old
/// blocking wait allowed 40 ms and never once ran out on the maintainer's
/// machines. This is deliberately far longer, because running out now costs a
/// visible retry rather than a lowered canvas, and a VM's first frame after the
/// engine has been busy can take a good while.
pub const CONFIRM_TIMEOUT: Duration = Duration::from_millis(750);

/// The pause between a failed attempt and the next one. Long enough that a GTK
/// that is not presenting is not poked at frame rate, short enough that a
/// window which recovers gets its editor within a second or two.
pub const RETRY_AFTER: Duration = Duration::from_millis(1000);

/// Whether a lowering has to be preceded by a committed GTK frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The rule: wait for the frame, or keep the canvas above.
    Gated,
    /// The behaviour before this module existed: lower at once. Kept, behind
    /// `CORDIAL_STACKING_GATE=off`, as the control a before-and-after
    /// measurement needs from the same binary.
    Legacy,
}

impl Mode {
    /// `CORDIAL_STACKING_GATE=off` selects [`Mode::Legacy`]; anything else, or
    /// nothing, is the gate.
    pub fn from_env() -> Mode {
        match std::env::var("CORDIAL_STACKING_GATE").as_deref() {
            Ok("off") | Ok("0") | Ok("legacy") => Mode::Legacy,
            _ => Mode::Gated,
        }
    }
}

/// What the caller should do about the canvas this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing.
    Idle,
    /// Make the window transparent over the canvas, ask GTK for a frame, and
    /// start polling for it. The canvas stays where it is.
    Arm,
    /// Restack the canvas beneath the window now.
    Lower,
    /// A frame was asked for and none came in time: undo the transparency and
    /// leave the canvas above. `attempt` counts from 1.
    GiveUp { attempt: u32 },
    /// Nobody wants the canvas lowered any more while a frame was still being
    /// waited for: undo the transparency.
    Cancel,
    /// Put the canvas back above the window.
    Raise,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Above,
    Arming { since: Instant },
    Below,
}

/// The decision, with no GTK in it.
#[derive(Debug)]
pub struct Gate {
    mode: Mode,
    state: State,
    /// Failed attempts in the current run of them, reset when one succeeds.
    attempts: u32,
    /// Not before this instant; set by a failed attempt.
    retry_at: Option<Instant>,
}

impl Gate {
    pub fn new(mode: Mode) -> Gate {
        Gate { mode, state: State::Above, attempts: 0, retry_at: None }
    }

    /// Whether a frame is currently being waited for, so the caller knows to
    /// ask GTK about it. Asking otherwise would be a wasted lookup.
    pub fn is_arming(&self) -> bool {
        matches!(self.state, State::Arming { .. })
    }

    /// Whether the canvas has been restacked below the window.
    pub fn is_below(&self) -> bool {
        self.state == State::Below
    }

    /// One tick.
    ///
    /// `want_below` is whether anything -- the editor, a dialog -- currently
    /// needs the canvas out of the way. `frame_committed` is whether GTK has
    /// committed a frame painted since the last [`Step::Arm`]; it is only
    /// consulted while arming, and the caller may pass `false` otherwise.
    pub fn poll(&mut self, want_below: bool, now: Instant, frame_committed: bool) -> Step {
        match self.state {
            State::Above if want_below => {
                if self.mode == Mode::Legacy {
                    self.state = State::Below;
                    return Step::Lower;
                }
                if self.retry_at.is_some_and(|t| now < t) {
                    return Step::Idle;
                }
                self.state = State::Arming { since: now };
                Step::Arm
            }
            State::Above => Step::Idle,
            State::Arming { .. } if !want_below => {
                self.state = State::Above;
                Step::Cancel
            }
            State::Arming { since } => {
                if frame_committed {
                    self.state = State::Below;
                    self.attempts = 0;
                    self.retry_at = None;
                    Step::Lower
                } else if now.duration_since(since) >= CONFIRM_TIMEOUT {
                    self.state = State::Above;
                    self.attempts += 1;
                    self.retry_at = Some(now + RETRY_AFTER);
                    Step::GiveUp { attempt: self.attempts }
                } else {
                    Step::Idle
                }
            }
            State::Below if want_below => Step::Idle,
            State::Below => {
                self.state = State::Above;
                self.retry_at = None;
                Step::Raise
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    /// The rule the whole module exists for. Every tick of a long wait with no
    /// committed frame, from the moment something asks for the canvas to be
    /// lowered, and not one of them may say `Lower`.
    #[test]
    fn the_canvas_is_never_lowered_while_no_gtk_frame_has_been_committed() {
        let mut gate = Gate::new(Mode::Gated);
        let start = t0();
        // Twenty seconds of ticks at 50 ms, with GTK never committing.
        for i in 0..400u64 {
            let now = start + Duration::from_millis(i * 50);
            let step = gate.poll(true, now, false);
            assert_ne!(step, Step::Lower, "lowered at tick {i} with no committed frame");
            assert!(!gate.is_below());
        }
    }

    /// The control. The behaviour this replaces lowered on the first tick, with
    /// nothing committed, which is the grey screen.
    #[test]
    fn control_the_legacy_mode_lowers_regardless_of_any_frame() {
        let mut gate = Gate::new(Mode::Legacy);
        assert_eq!(gate.poll(true, t0(), false), Step::Lower);
        assert!(gate.is_below());
    }

    #[test]
    fn a_committed_frame_lowers_it() {
        let mut gate = Gate::new(Mode::Gated);
        let now = t0();
        assert_eq!(gate.poll(true, now, false), Step::Arm);
        assert!(gate.is_arming());
        assert_eq!(gate.poll(true, now + Duration::from_millis(50), false), Step::Idle);
        assert_eq!(gate.poll(true, now + Duration::from_millis(100), true), Step::Lower);
        assert!(gate.is_below());
        assert!(!gate.is_arming());
    }

    /// A frame that lands on the tick that would also have timed out is still a
    /// frame; the timeout must not win a tie against evidence.
    #[test]
    fn a_frame_arriving_at_the_deadline_beats_the_deadline() {
        let mut gate = Gate::new(Mode::Gated);
        let now = t0();
        gate.poll(true, now, false);
        assert_eq!(gate.poll(true, now + CONFIRM_TIMEOUT, true), Step::Lower);
    }

    #[test]
    fn no_frame_in_time_gives_up_and_leaves_the_canvas_above() {
        let mut gate = Gate::new(Mode::Gated);
        let now = t0();
        assert_eq!(gate.poll(true, now, false), Step::Arm);
        assert_eq!(gate.poll(true, now + CONFIRM_TIMEOUT, false), Step::GiveUp { attempt: 1 });
        assert!(!gate.is_below());
        assert!(!gate.is_arming());
    }

    /// Not giving up for good: the point of the retry is that a GTK which
    /// starts presenting again gets its editor, without the user having to
    /// blur and refocus the box.
    #[test]
    fn it_retries_after_the_pause_and_a_late_frame_then_lowers() {
        let mut gate = Gate::new(Mode::Gated);
        let now = t0();
        gate.poll(true, now, false);
        let failed = now + CONFIRM_TIMEOUT;
        assert_eq!(gate.poll(true, failed, false), Step::GiveUp { attempt: 1 });
        // Inside the pause: nothing, in particular no second arming.
        assert_eq!(gate.poll(true, failed + Duration::from_millis(500), false), Step::Idle);
        let again = failed + RETRY_AFTER;
        assert_eq!(gate.poll(true, again, false), Step::Arm);
        assert_eq!(gate.poll(true, again + Duration::from_millis(30), true), Step::Lower);
    }

    #[test]
    fn repeated_failures_count_up_and_a_success_resets_the_count() {
        let mut gate = Gate::new(Mode::Gated);
        let mut now = t0();
        for expected in 1..=3u32 {
            assert_eq!(gate.poll(true, now, false), Step::Arm);
            now += CONFIRM_TIMEOUT;
            assert_eq!(gate.poll(true, now, false), Step::GiveUp { attempt: expected });
            now += RETRY_AFTER;
        }
        assert_eq!(gate.poll(true, now, false), Step::Arm);
        assert_eq!(gate.poll(true, now + Duration::from_millis(20), true), Step::Lower);
        // Raise, then a fresh episode starts counting from one again.
        assert_eq!(gate.poll(false, now + Duration::from_secs(1), false), Step::Raise);
        let later = now + Duration::from_secs(2);
        assert_eq!(gate.poll(true, later, false), Step::Arm);
        assert_eq!(gate.poll(true, later + CONFIRM_TIMEOUT, false), Step::GiveUp { attempt: 1 });
    }

    /// The editor blurred while a frame was still awaited. The window was made
    /// transparent for the attempt, so it has to be told to stop.
    #[test]
    fn wanting_it_back_while_arming_cancels_rather_than_raising() {
        let mut gate = Gate::new(Mode::Gated);
        let now = t0();
        gate.poll(true, now, false);
        assert_eq!(gate.poll(false, now + Duration::from_millis(20), false), Step::Cancel);
        assert!(!gate.is_arming());
        // And a refocus straight away starts over rather than being refused.
        assert_eq!(gate.poll(true, now + Duration::from_millis(40), false), Step::Arm);
    }

    #[test]
    fn a_lowered_canvas_is_raised_when_nothing_wants_it_lowered() {
        let mut gate = Gate::new(Mode::Gated);
        let now = t0();
        gate.poll(true, now, false);
        gate.poll(true, now + Duration::from_millis(10), true);
        assert!(gate.is_below());
        assert_eq!(gate.poll(true, now + Duration::from_millis(60), false), Step::Idle);
        assert_eq!(gate.poll(false, now + Duration::from_millis(70), false), Step::Raise);
        assert!(!gate.is_below());
        assert_eq!(gate.poll(false, now + Duration::from_millis(80), false), Step::Idle);
    }

    /// Raising never waits for a frame: a canvas going back on top covers the
    /// window whatever GTK has or has not painted.
    #[test]
    fn raising_is_not_gated() {
        let mut gate = Gate::new(Mode::Gated);
        let now = t0();
        gate.poll(true, now, false);
        gate.poll(true, now + Duration::from_millis(10), true);
        assert_eq!(gate.poll(false, now + Duration::from_millis(20), false), Step::Raise);
    }

    #[test]
    fn nothing_happens_when_nothing_is_wanted() {
        let mut gate = Gate::new(Mode::Gated);
        let now = t0();
        for i in 0..10u64 {
            assert_eq!(gate.poll(false, now + Duration::from_millis(i * 50), true), Step::Idle);
        }
    }
}
