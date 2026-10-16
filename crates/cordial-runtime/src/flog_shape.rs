//! Telling the user that a log-channel override is in a shape that silences it
//! (issue #13).
//!
//! A `FLog`/`DFLog` channel's value is either a bare number (`"6"`) or a
//! severity name (`"Verbose"`, `"Warning,6"`), and a bare number given to a
//! channel that wanted a severity name does not error: the channel goes quiet.
//! `FLogNativeDM` printed 30 lines with `"Verbose"` and none with `"100"`. A
//! quiet log reads as a quiet channel, so the override has to be questioned at
//! launch, where the user can still see it.
//!
//! **Only a number given to a channel that needs a name is warned about**, and
//! that is all the evidence supports. The survey in
//! `docs/analysis/flog-channel-shapes.tsv` (engine 2.738.0.1397, signed out,
//! two repeats that agreed) found no channel that logged under a number and not
//! under `"Verbose"`, so a severity name given to a channel Roblox's settings
//! give a number is not a warning. The channels it names are those Roblox's own
//! settings give a severity (`settings_names_a_severity`) and the 66 the survey
//! saw go silent under a number, `flog-number-silences.txt`. That is not every
//! such channel: one the landing page never reached has no evidence either way.

use serde_json::{Map, Value};

/// Channels a bare number silenced and `"Verbose"` did not. See the survey.
const NUMBER_SILENCES: &str = include_str!("flog-number-silences.txt");

fn is_channel(key: &str) -> bool {
    key.starts_with("FLog") || key.starts_with("DFLog")
}

/// Whether `value` is a bare integer, the shape that silences those channels.
/// A place-filtered value (`6;12345`) is judged by what precedes the `;`.
pub fn is_bare_number(value: &str) -> bool {
    let head = value.split(';').next().unwrap_or(value);
    let digits = head.strip_prefix(['+', '-']).unwrap_or(head);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Whether `value` starts with a severity name, optionally with `,level`.
fn is_severity(value: &str) -> bool {
    let head = value.split(';').next().unwrap_or(value);
    let name = head.split(',').next().unwrap_or(head);
    matches!(name, "Fatal" | "Error" | "Warning" | "Info" | "Verbose" | "Debug" | "Trace")
}

fn measured_to_need_a_name(key: &str) -> bool {
    NUMBER_SILENCES.lines().any(|l| l.trim() == key)
}

/// One line for each override that gives a bare number to a channel that needs
/// a severity name, judged from the settings document `base` and the survey.
pub fn warnings(base: &Map<String, Value>, overrides: &Map<String, Value>) -> Vec<String> {
    let mut out = Vec::new();
    for (key, given) in overrides {
        if !is_channel(key) {
            continue;
        }
        let given = match given {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if !is_bare_number(&given) {
            continue;
        }
        let settings_names_a_severity = base
            .get(key)
            .and_then(Value::as_str)
            .map(is_severity)
            .unwrap_or(false);
        // The advice names the file and not `stacked flags set`, because the
        // CLI checks every `FLog*` value as a whole number and refuses
        // "Verbose": advice to run it would send the user into a refusal.
        if settings_names_a_severity || measured_to_need_a_name(key) {
            out.push(format!(
                "{key}={given:?} is a bare number, and a channel like this one logs nothing for a number; \
                 give it a severity name such as \"Verbose\" by editing flags.json by hand \
                 (`stacked flags path` prints where it is; `stacked flags set` accepts only numbers \
                 for FLog channels; docs/fastflags.md)"
            ));
        }
    }
    out
}

/// Print them, once, at launch.
pub fn report(base_document: &str, overrides: &Map<String, Value>) {
    if !overrides.keys().any(|k| is_channel(k)) {
        return;
    }
    let Ok(doc) = serde_json::from_str::<Value>(base_document) else { return };
    let Some(base) = doc.get("applicationSettings").and_then(Value::as_object) else { return };
    for line in warnings(base, overrides) {
        println!("  flags: {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn the_issues_own_example_is_warned_about() {
        // FLogNativeDM: 30 lines with "Verbose", none with "100".
        let w = warnings(&map(json!({})), &map(json!({"FLogNativeDM": "100"})));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].starts_with("FLogNativeDM=\"100\""), "{w:?}");
    }

    #[test]
    fn a_severity_name_is_never_warned_about() {
        let base = map(json!({"FLogNetwork": "7", "FLogAudio": "Info"}));
        let w = warnings(&base, &map(json!({"FLogNetwork": "Verbose", "FLogNativeDM": "Verbose", "FLogAudio": "Warning,6"})));
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn a_number_for_a_channel_the_settings_give_a_name_is_warned_about() {
        let base = map(json!({"FLogMadeUp": "Warning,6", "DFLogPlain": "6"}));
        let w = warnings(&base, &map(json!({"FLogMadeUp": 7, "DFLogPlain": "7"})));
        assert_eq!(w.len(), 1, "a number for a channel that takes numbers is fine: {w:?}");
        assert!(w[0].starts_with("FLogMadeUp="), "{w:?}");
    }

    #[test]
    fn what_has_no_evidence_is_not_warned_about() {
        let w = warnings(&map(json!({})), &map(json!({"FLogNeverSeen": "7", "FFlagThing": "True"})));
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn place_filtered_numbers_are_judged_by_their_head() {
        assert!(is_bare_number("6;123;456"));
        assert!(is_bare_number("-1"));
        assert!(!is_bare_number("Verbose,9;123"));
        assert!(!is_bare_number(""));
        assert!(is_severity("Verbose,9;5757772154"));
        assert!(!is_severity("10;214;295"));
    }

    #[test]
    fn the_embedded_survey_list_is_clean() {
        let names: Vec<_> = NUMBER_SILENCES.lines().collect();
        assert!(names.len() > 50 && names.iter().all(|n| is_channel(n) && !n.contains(char::is_whitespace)));
        assert!(names.contains(&"FLogNativeDM"));
    }
}
