//! One dropdown and one toggle, and why it is neither two dropdowns nor two
//! toggles.
//!
//! **Auto update** — update in the background, ask, or manual. **Download on
//! metered connection** — one switch.
//!
//! ## The two-dropdown shape this replaced, and why it went
//!
//! This module used to be two enums, and its header argued the case at length:
//! three controls that can each be set on their own produce combinations with no
//! defined meaning, and "update in the background, but never download" is a
//! setting that has to either lie or explain itself. That reasoning is kept here
//! rather than deleted, because it was not wrong — and because the shape that
//! followed it, a dropdown and *two* switches, did make that contradiction
//! reachable. Both connection switches off with Auto update on background could
//! never download anything, so the module named the state, the settings page
//! carried a warning row about it, and `may_download` reported it.
//!
//! ## And why the second switch went too
//!
//! All of that machinery existed because one question was being asked twice.
//! **There is no radio in this.** `org.freedesktop.NetworkManager`'s `Metered`
//! property is the only question [`crate::metered`] asks, and it is a statement
//! about who pays for the bytes rather than about the link layer. *Download on
//! Wi-Fi* governed the not-metered answer and *Download on metered connection*
//! governed the metered one — two names for one bit, with a switch labelled
//! Wi-Fi that an ordinary wired desktop was also governed by. Its own row in the
//! settings page had to explain that, which is a fair sign the label was wrong.
//!
//! Nobody turns Wi-Fi off while leaving metered on. So the question is asked
//! once, in the direction people have an opinion about: **may a download run
//! when the connection is metered?** Off holds the download until an unmetered
//! link turns up, which is what somebody on a data allowance wants; on stops the
//! connection being consulted at all.
//!
//! With one switch there is no combination that refuses forever, so the warning
//! row and [`DownloadOn::never_downloads`] are both gone. The contradiction the
//! original two-dropdown argument warned about is unreachable again — not by
//! forbidding it, but because it was an artefact of the duplicated question.
//!
//! [`UpdateSettings::plan`] is still total, which is the property every one of
//! these shapes was chosen for: every combination of the controls and every one
//! of NetworkManager's answers maps to exactly one [`Plan`], and
//! `every_setting_combination_has_exactly_one_plan` is the test that would have
//! to be told what a new control meant.
//!
//! ## A guess counts as metered
//!
//! NetworkManager answers with four values, two of which are guesses, and both
//! guesses are treated as metered. An ordinary desktop on a LAN answers
//! guess-no, so the default settings hold a download there — the switch is
//! reached far more often than "am I on a hotspot" suggests. Reading a guess as
//! cheap is how a data allowance pays for a 115 MB download nobody asked for,
//! and the cost of being wrong the other way is a button press.
//!
//! Nothing here is GTK. The shell binds an `AdwComboRow` to [`Automatic::index`],
//! the same seam `shell_config::AppearanceScheme` already uses, so the position
//! in the model is the one thing that has to agree between the two files rather
//! than a name-keyed lookup that can drift from the model's actual contents.

use crate::metered::Metered;
use serde::{Deserialize, Serialize};

/// What Cordial does about updates without being asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Automatic {
    /// Check, and fetch what it finds, subject to [`DownloadOn`].
    Background,
    /// Check, and when Cordial starts with an update waiting, open the changelog
    /// and wait to be told. A dialog on launch rather than a badge: the point of
    /// asking is that somebody is asked.
    Ask,
    /// No check of any kind without a button being pressed.
    ///
    /// **Manual, not "disabled".** It turns off every automatic behaviour and it
    /// does not turn the feature off, because the header-bar button still checks
    /// on demand — it becomes a refresh control. Turning off automatic checking
    /// is a statement about background network use, not a refusal to ever know,
    /// and "disabled" says the second thing.
    Manual,
}

impl Default for Automatic {
    /// Background, because a stale client is not a working client: Roblox
    /// refuses old builds server-side, so this is not a convenience setting and
    /// the default that leaves somebody broken on Roblox's schedule is the
    /// wrong one. It is a safe default only because [`DownloadOn`] leaves the
    /// metered switch off — the pair is the decision, not either half.
    fn default() -> Self {
        Automatic::Background
    }
}

impl Automatic {
    /// Order matches the `AdwComboRow` model in the shell.
    pub fn index(self) -> u32 {
        match self {
            Automatic::Background => 0,
            Automatic::Ask => 1,
            Automatic::Manual => 2,
        }
    }

    pub fn from_index(index: u32) -> Self {
        match index {
            0 => Automatic::Background,
            1 => Automatic::Ask,
            _ => Automatic::Manual,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Automatic::Background => "Update in background",
            Automatic::Ask => "Ask",
            Automatic::Manual => "Manual",
        }
    }
}

/// Which connections a download may use. **One switch, not two.**
///
/// *Download on Wi-Fi* used to sit beside this one, and its own description gave
/// the game away: Cordial cannot see whether a link is wireless, so it asked
/// NetworkManager the metered question and called the not-metered answer Wi-Fi.
/// That made the pair two names for one bit — a switch labelled Wi-Fi that a
/// wired desktop was also governed by, and which nobody has a reason to turn off
/// while leaving the metered one on.
///
/// So the question is asked once, in the direction people actually have an
/// opinion about: **may a download run when the connection is metered?** Off
/// means downloads wait for an unmetered link, which is what somebody on a data
/// allowance wants. On means the connection is not consulted at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "kebab-case")]
pub struct DownloadOn {
    /// *Download on metered connection.* Off by default: reading a guess as
    /// cheap is how somebody's data allowance pays for a 115 MB download they
    /// never asked for, and the cost of being wrong the other way is a button
    /// press.
    pub metered: bool,
}

/// The sentence shown where a download is being held back by the connection.
///
/// A constant rather than a formatted string, so every place that explains the
/// switch is demonstrably the same words.
///
/// It was four lines and it is now one. The three that went explained *why* a
/// guess counts as metered and what the switch does not govern — true, and
/// reasoning, and now in [`DownloadOn::metered`]'s comment where reasoning
/// belongs. What survives is the half that changes what somebody does: their
/// download is waiting, and there is a button that ignores the wait.
pub const NEVER_DOWNLOADS: &str =
    "Downloads wait for an unmetered connection. Pressing Update always works.";

/// All three controls. Serialised into whatever document the shell keeps its own
/// preferences in; nothing here reads or writes a file, because where Cordial's
/// settings live is `shell_config`'s answer and ADR-013's, not this crate's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct UpdateSettings {
    pub automatic: Automatic,
    pub download_on: DownloadOn,
}

/// What should happen on launch, given the settings and the connection.
///
/// Total by construction: every combination of settings values maps to exactly
/// one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Check in the background and fetch whatever it finds.
    CheckAndDownload,
    /// Check in the background, then wait to be asked.
    ///
    /// `why` is `Some` when the reason for waiting is the connection or the
    /// switches rather than the mode, so the button can say which — "waiting
    /// because you asked to be asked", "waiting because you are on a hotspot"
    /// and "waiting because you turned both connections off" are three different
    /// things to be looking at.
    CheckAndAsk { why: Option<String> },
    /// No request of any kind. The button offers a manual check.
    DoNotCheck,
}

impl UpdateSettings {
    pub fn plan(self, metered: Metered) -> Plan {
        match self.automatic {
            Automatic::Manual => Plan::DoNotCheck,
            Automatic::Ask => Plan::CheckAndAsk { why: None },
            Automatic::Background => match self.may_download(metered) {
                Ok(()) => Plan::CheckAndDownload,
                Err(why) => Plan::CheckAndAsk { why: Some(why) },
            },
        }
    }

    /// Whether the expensive half may run right now, or why not.
    ///
    /// The refusal is a sentence rather than a flag because it ends up in front
    /// of somebody wondering why their update is sitting there, and "metered"
    /// on its own invites "no it isn't" — [`Metered::describe`] says which of
    /// NetworkManager's four answers it was.
    pub fn may_download(self, metered: Metered) -> Result<(), String> {
        // An unmetered link is never in question: the only switch here is about
        // spending a data allowance, and there is none to spend. This used to
        // consult a second switch called Wi-Fi at this point, which is how a
        // wired desktop with Wi-Fi turned off ended up refusing to download.
        if !metered.is_metered() {
            return Ok(());
        }
        if self.download_on.metered {
            return Ok(());
        }
        Err(format!(
            "{}, and Download on metered connection is off.",
            metered.describe()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_ones_the_design_names() {
        let s = UpdateSettings::default();
        assert!(!s.download_on.metered, "a data allowance is not the default to spend");
        assert_eq!(s.automatic, Automatic::Background);
    }

    #[test]
    fn manual_means_no_request_whatever_the_network_is() {
        // Manual is about background network use, so a fast unmetered connection
        // must not talk it out of the setting. What it does not mean is "never
        // know": the header-bar button still checks on demand, which is the
        // whole reason the option is not called Disabled.
        let s = UpdateSettings {
            automatic: Automatic::Manual,
            download_on: DownloadOn { metered: true },
        };
        assert_eq!(s.plan(Metered::No), Plan::DoNotCheck);
        assert_eq!(s.plan(Metered::Yes), Plan::DoNotCheck);
    }

    #[test]
    fn background_over_a_metered_connection_waits_and_says_why() {
        // Still checking, not yet downloading, and naming which of the four
        // answers it got.
        let s = UpdateSettings::default();
        match s.plan(Metered::GuessYes) {
            Plan::CheckAndAsk { why: Some(why) } => {
                assert!(why.contains("Download on metered connection"), "{why}");
                // Which of the four answers it got, without the hedge being
                // dropped: guess-yes is a guess and saying so flatly is what
                // makes somebody on a wired desktop think Cordial is wrong.
                assert!(why.contains("looks metered"), "{why}");
            }
            other => panic!("expected a reason, got {other:?}"),
        }
    }

    #[test]
    fn off_means_wait_for_an_unmetered_link_rather_than_never() {
        // There used to be a second switch here, and a test pinning the
        // combination that downloaded nothing ever. Both are gone: with one
        // switch, off holds a download on a metered link and releases it on an
        // unmetered one, so there is no settings state that refuses forever and
        // no warning row to announce one.
        let s = UpdateSettings {
            automatic: Automatic::Background,
            download_on: DownloadOn { metered: false },
        };
        assert!(s.may_download(Metered::Yes).is_err());
        assert!(s.may_download(Metered::GuessYes).is_err());
        assert!(s.may_download(Metered::GuessNo).is_err(), "a guess is metered");
        assert!(s.may_download(Metered::Unknown).is_err(), "unknown is metered");
        // The one answer that releases it, and the reason the switch is not
        // called Never.
        assert_eq!(s.may_download(Metered::No), Ok(()));

        // One line, and both halves of it earn their place: the download is
        // waiting rather than refused, and there is a button that ignores the
        // wait. The reasoning that used to sit here is in `DownloadOn::metered`.
        assert!(NEVER_DOWNLOADS.contains("unmetered"), "{NEVER_DOWNLOADS}");
        assert!(NEVER_DOWNLOADS.len() < 120, "{NEVER_DOWNLOADS}");
        // It does not overstate itself: an Update the user presses is not what
        // this switch is about, and saying otherwise would be a second wrong
        // sentence rather than a fix for the first.
        assert!(NEVER_DOWNLOADS.contains("Pressing Update always works"), "{NEVER_DOWNLOADS}");
    }

    #[test]
    fn background_over_an_unmetered_connection_downloads() {
        assert_eq!(UpdateSettings::default().plan(Metered::No), Plan::CheckAndDownload);
    }

    #[test]
    fn turning_the_switch_on_stops_the_connection_being_consulted() {
        let anywhere =
            UpdateSettings { automatic: Automatic::Background, download_on: DownloadOn { metered: true } };
        for metered in [Metered::No, Metered::Yes, Metered::GuessNo, Metered::GuessYes, Metered::Unknown] {
            assert_eq!(anywhere.plan(metered), Plan::CheckAndDownload, "{metered:?}");
        }
    }

    #[test]
    fn a_guess_is_metered_and_holds_the_download() {
        // The consequence nobody expects: an ordinary desktop on a LAN answers
        // guess-no, which is metered, so the default settings hold the download
        // there. Deleting this is how "guesses are metered" quietly becomes
        // "guesses go the way they lean". This is also why the Wi-Fi switch was
        // a poor name for the other side of it -- that desktop is not on Wi-Fi
        // and was governed by it anyway.
        let s = UpdateSettings::default();
        assert!(s.may_download(Metered::GuessNo).is_err());
    }

    #[test]
    fn asking_first_is_not_overridden_by_a_cheap_connection() {
        let s = UpdateSettings {
            automatic: Automatic::Ask,
            download_on: DownloadOn { metered: true },
        };
        assert_eq!(s.plan(Metered::No), Plan::CheckAndAsk { why: None });
    }

    #[test]
    fn every_setting_combination_has_exactly_one_plan() {
        // The property the old two-dropdown shape was chosen for, kept through
        // the change to a dropdown and two switches, and then to a dropdown and
        // one. If a control were ever added back, this is the test that would
        // have to be told what the new combinations mean.
        for automatic in [Automatic::Background, Automatic::Ask, Automatic::Manual] {
            for metered_switch in [true, false] {
                for metered in
                    [Metered::No, Metered::Yes, Metered::GuessNo, Metered::GuessYes, Metered::Unknown]
                {
                    let s =
                        UpdateSettings { automatic, download_on: DownloadOn { metered: metered_switch } };
                    // Total: no panic, no "undefined" arm to fall into.
                    let _ = s.plan(metered);
                }
            }
        }
    }

    #[test]
    fn the_combo_indices_round_trip() {
        for a in [Automatic::Background, Automatic::Ask, Automatic::Manual] {
            assert_eq!(Automatic::from_index(a.index()), a);
        }
    }

    #[test]
    fn the_third_option_is_named_manual_rather_than_disabled() {
        // The word is the setting's meaning. Manual turns off every automatic
        // behaviour and leaves the header-bar button checking on demand;
        // "Disabled" would claim the feature was gone.
        assert_eq!(Automatic::Manual.label(), "Manual");
        assert_eq!(Automatic::Background.label(), "Update in background");
        assert_eq!(Automatic::Ask.label(), "Ask");
    }

    #[test]
    fn the_stored_form_survives_a_round_trip_and_an_absent_key() {
        let s = UpdateSettings {
            automatic: Automatic::Ask,
            download_on: DownloadOn { metered: true },
        };
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains("\"ask\""), "{text}");
        assert_eq!(serde_json::from_str::<UpdateSettings>(&text).unwrap(), s);
        // A settings file written before this existed must not refuse to load.
        assert_eq!(serde_json::from_str::<UpdateSettings>("{}").unwrap(), UpdateSettings::default());
        // Nor one written while the switches were still a single enum.
        assert_eq!(
            serde_json::from_str::<UpdateSettings>(r#"{"automatic":"manual"}"#).unwrap().automatic,
            Automatic::Manual
        );
    }
}
