//! The on-disk shape of a flags file, shared by the launcher and the client.
//!
//! **This is in `cordial-plugins` for a dependency reason and not a conceptual
//! one, and that is worth saying plainly so nobody moves it back.**
//! `cordial_runtime::flags` owns everything about flag *layering* -- which file
//! beats which, what a plugin may contribute, how a conflict is reported -- and
//! that is where this logic belongs by subject. It cannot live there, because
//! `cordial-runtime` depends on `cordial-shell` (the client builds its window
//! through the shell's `host_window`), so the settings window cannot depend on
//! the runtime without a cycle. `cordial-plugins` is the crate both already
//! share.
//!
//! **The alternative was two implementations, and that alternative is the bug.**
//! The settings window validates what somebody pasted; the client parses the
//! same file at startup. If those two disagree by one case, the window accepts
//! a document that the client then reports as malformed and ignores -- and the
//! user sees a page that said "Saved 40 flags" and a game with none of them,
//! with nothing anywhere connecting the two. One function, used by both, is
//! what makes `flags::tests::a_written_document_round_trips_through_the_loader`
//! able to assert that at all.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where a profile's own flags document lives.
///
/// **Here rather than only in `cordial_runtime::flags` for the same reason as
/// everything else in this module: the window and the client must name the same
/// file.** The settings window computed `profile_dir.join("flags.json")`
/// directly in its first draft, which is right until somebody sets
/// `CORDIAL_FLAGS` -- at which point the window edits one file and the client
/// reads another, and the page reports a save that changes nothing.
///
/// `CORDIAL_FLAGS` makes one file serve every profile, so it is a development
/// switch rather than a supported arrangement; it is honoured here anyway,
/// because a switch that half the program obeys is worse than one nothing does.
pub fn path_in(profile_dir: &Path) -> PathBuf {
    std::env::var_os("CORDIAL_FLAGS")
        .map(PathBuf::from)
        .unwrap_or_else(|| profile_dir.join("flags.json"))
}

/// Parse the text of a flags document into name/value pairs.
///
/// Bloxstrap's exports are a flat object of string values, so they paste in
/// unchanged. Booleans and numbers are converted to their string form rather
/// than refused, because Roblox stores every setting as a string and a person
/// hand-writing the file should not have to know that -- and because
/// `cordial_runtime::flags::read_layer` has always converted them, so refusing
/// here would make the editor stricter than the loader for no reason.
///
/// What is refused is a value with no sensible string form: an object, an
/// array, or a null. **Refused by name**, because "invalid JSON" against a
/// two-hundred-line paste makes somebody bisect their own document by hand.
///
/// An empty document is `Ok` and empty. That is how the editor clears the file,
/// and it is a different outcome from a parse failure.
pub fn parse(text: &str) -> Result<BTreeMap<String, String>, String> {
    if text.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let parsed: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let obj = parsed
        .as_object()
        .ok_or_else(|| "the document must be a JSON object of flag names to values".to_string())?;

    let mut values = BTreeMap::new();
    for (key, value) in obj {
        if key.trim().is_empty() {
            return Err("a flag with an empty name".to_string());
        }
        values.insert(key.clone(), value_text(key, value)?);
    }
    Ok(values)
}

/// A JSON value as the string Roblox stores, or why it has none.
fn value_text(key: &str, value: &serde_json::Value) -> Result<String, String> {
    match value {
        serde_json::Value::String(s) => Ok(s.clone()),
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => Ok(value.to_string()),
        serde_json::Value::Null => Err(format!("{key}: null is not a value. Remove the line to unset it.")),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            Err(format!("{key}: a flag value must be text, a number or true/false"))
        }
    }
}

/// What a flag's name says its value is. Roblox carries the type in the
/// prefix -- `FFlag` a boolean, `FInt` an integer, `FLog` a log channel,
/// `FString` text -- and each may be `D`- (dynamic) or `S`- (synced) prefixed.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Kind {
    Bool,
    Int,
    /// **Not a number.** A log channel is declared as a bare verbosity
    /// (`FLogNetwork = "7"`) or as a severity name with an optional sub-level
    /// (`FLogAudio = "Info"`, `DFLogWebSocketTraceError = "Warning,6"`), and
    /// which one a channel wants is a property of its own C++ declaration,
    /// which nothing here can see -- see `cordial_runtime::flags::read_layer`.
    /// So it takes anything. Treating `FLog` as an integer, which the first
    /// version of this check did, refused every real severity name and, in an
    /// all-or-nothing import, failed a whole list over one of them.
    Log,
    Text,
    /// No recognised prefix. Kept, and reported, since names such as
    /// `Cordial`-prefixed ones ride the same layering and no list of every
    /// legitimate prefix is something this module could keep complete.
    Unknown,
}

pub fn kind_of(name: &str) -> Kind {
    let bare = name.strip_prefix('D').or_else(|| name.strip_prefix('S')).unwrap_or(name);
    if bare.starts_with("FFlag") {
        Kind::Bool
    } else if bare.starts_with("FInt") {
        Kind::Int
    } else if bare.starts_with("FLog") {
        Kind::Log
    } else if bare.starts_with("FString") {
        Kind::Text
    } else {
        Kind::Unknown
    }
}

/// The value to store for `raw` under `name`, or why it cannot be.
///
/// Booleans come out as `True`/`False`, which is how Roblox's own settings
/// document and every Bloxstrap export spell them. An integer flag takes a
/// whole number, and a JSON number such as `144.0` that is one is written
/// `144`. Everything else is kept exactly as given.
///
/// This is the one implementation `stacked flags set`, `edit` and `import`
/// share, so what one accepts the others accept too.
pub fn check(name: &str, raw: &str) -> Result<String, String> {
    reason_to_refuse(name, raw).map_or_else(|| Ok(normalise(name, raw)), |why| Err(format!("{name}: {why}")))
}

/// Why `raw` cannot be `name`'s value, worded to follow the name, or `None`.
fn reason_to_refuse(name: &str, raw: &str) -> Option<String> {
    if name.trim().is_empty() {
        return Some("a flag needs a name".to_string());
    }
    if name.chars().any(char::is_whitespace) {
        return Some("a flag name has no spaces in it".to_string());
    }
    let value = raw.trim();
    match kind_of(name) {
        Kind::Bool if !matches!(value.to_ascii_lowercase().as_str(), "true" | "false") => {
            Some(format!("a boolean flag, so it takes True or False, not {value:?}"))
        }
        Kind::Int if whole_number(value).is_none() => {
            Some(format!("a number flag, so it takes a whole number, not {value:?}"))
        }
        _ => None,
    }
}

/// The stored spelling of a value [`reason_to_refuse`] accepted.
fn normalise(name: &str, raw: &str) -> String {
    let value = raw.trim();
    match kind_of(name) {
        Kind::Bool => if value.eq_ignore_ascii_case("true") { "True" } else { "False" }.to_string(),
        Kind::Int => whole_number(value).unwrap_or_else(|| value.to_string()),
        Kind::Log | Kind::Text | Kind::Unknown => raw.to_string(),
    }
}

fn whole_number(value: &str) -> Option<String> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        return value.parse::<i64>().ok().map(|n| n.to_string());
    }
    // `144.0`, which is what a JSON writer emits for a float that happens to be
    // whole. Anything with a fraction, an exponent or text is not a whole number.
    let (whole, fraction) = value.split_once('.')?;
    let whole_digits = whole.strip_prefix('-').unwrap_or(whole);
    let ok = !whole_digits.is_empty()
        && whole_digits.chars().all(|c| c.is_ascii_digit())
        && !fraction.is_empty()
        && fraction.chars().all(|c| c == '0');
    ok.then(|| whole.parse::<i64>().ok()).flatten().map(|n| n.to_string())
}

/// One entry an import did not take, and why. Named, because "2 skipped" is a
/// number somebody has to go and find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub name: String,
    pub why: String,
}

/// Which shape a document turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// A flat object of name to value: Bloxstrap's and Fishstrap's
    /// `ClientAppSettings.json`, and what the editor has always taken.
    Flat,
    /// Sober's `config.json`, whose FastFlags are its `fflags` object.
    Sober,
}

/// The result of reading a document to import, entry by entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    pub format: Format,
    /// The entries that passed [`check`], with values normalised.
    pub flags: BTreeMap<String, String>,
    pub skipped: Vec<Skipped>,
    /// Accepted names with no FastFlag prefix. Written anyway; listed so a
    /// typo in a name is visible.
    pub unrecognised: Vec<String>,
}

/// Read a flag list from another launcher, **per entry**.
///
/// A flat object of flags is taken as it is. A Sober `config.json` (an object
/// with an `fflags` object) contributes only that object, minus the
/// `FFlagExample` placeholder a fresh install carries. Which one it is decided
/// by the document, so one `stacked flags import` serves both.
///
/// **One bad line does not cost the rest.** The value check used to be
/// all-or-nothing, so a single `FLogAudio: "Info"` refused by an over-strict
/// type check failed forty valid flags with it. A document that is not JSON, or
/// not an object, is still an error: there is nothing to take entries from.
/// An entry that fails is reported in `skipped` and the others are kept.
pub fn import(text: &str) -> Result<Imported, String> {
    let parsed: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let object = parsed
        .as_object()
        .ok_or_else(|| "the document must be a JSON object of flag names to values".to_string())?;

    let (format, entries) = match object.get("fflags") {
        Some(serde_json::Value::Object(inner)) => (Format::Sober, inner),
        Some(_) => return Err("its \"fflags\" is not an object".to_string()),
        None => (Format::Flat, object),
    };

    let mut out = Imported { format, flags: BTreeMap::new(), skipped: Vec::new(), unrecognised: Vec::new() };
    for (name, value) in entries {
        if format == Format::Sober && name == "FFlagExample" {
            continue;
        }
        // The reasons follow the name and do not repeat it: the list that shows
        // them already has the name in front.
        let raw = match value_text(name, value) {
            Ok(raw) => raw,
            Err(why) => {
                let why = why.strip_prefix(&format!("{name}: ")).unwrap_or(&why).to_string();
                out.skipped.push(Skipped { name: name.clone(), why });
                continue;
            }
        };
        match reason_to_refuse(name, &raw) {
            None => {
                if kind_of(name) == Kind::Unknown {
                    out.unrecognised.push(name.clone());
                }
                out.flags.insert(name.clone(), normalise(name, &raw));
            }
            Some(why) => out.skipped.push(Skipped { name: name.clone(), why }),
        }
    }
    Ok(out)
}

/// Add `incoming` to `into`, replacing a flag already there. Returns how many
/// were new and how many changed a value that was set.
pub fn merge(into: &mut BTreeMap<String, String>, incoming: &BTreeMap<String, String>) -> (usize, usize) {
    let (mut added, mut changed) = (0, 0);
    for (name, value) in incoming {
        match into.insert(name.clone(), value.clone()) {
            None => added += 1,
            Some(old) if old != *value => changed += 1,
            Some(_) => {}
        }
    }
    (added, changed)
}

/// Where Sober keeps its `config.json`, in the order to look: the Flatpak's
/// config directory, which is how VinegarHQ distributes it, then the XDG one a
/// native install would use. **INFERRED** for the second; no native Sober
/// install has been looked at.
pub fn sober_config_candidates() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    vec![
        home.join(".var/app/org.vinegarhq.Sober/config/sober/config.json"),
        config.join("sober/config.json"),
    ]
}

/// Write a flags document to `path`, replacing whatever was there.
///
/// Through a temporary and a rename, like
/// `cordial_runtime::flags::write_plugin_layer`: the client reads this file at
/// startup and a half-written document is reported as malformed and ignored,
/// which loses every flag rather than the one being edited.
///
/// An empty set writes `{}` rather than deleting the file. A file that is
/// present and empty says somebody cleared it; an absent one says nothing, and
/// the two are worth telling apart when a flag has stopped working.
pub fn write(path: &Path, values: &BTreeMap<String, String>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(values).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.new");
    std::fs::write(&tmp, format!("{text}\n")).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A Bloxstrap export pastes in unchanged.** That is the whole point of
    /// the editor, and it is a claim about someone else's format, so it gets a
    /// test rather than a comment.
    #[test]
    fn a_bloxstrap_style_document_parses() {
        let text = r#"{
            "DFIntTaskSchedulerTargetFps": "180",
            "FFlagDebugDisplayFPS": "False",
            "DFIntTextureQualityOverride": "3"
        }"#;
        let values = parse(text).expect("a Bloxstrap export must parse");
        assert_eq!(values.get("DFIntTaskSchedulerTargetFps").map(String::as_str), Some("180"));
        assert_eq!(values.get("FFlagDebugDisplayFPS").map(String::as_str), Some("False"));
        assert_eq!(values.len(), 3);
    }

    /// JSON types are converted, matching what the loader has always done.
    #[test]
    fn the_editor_accepts_everything_the_loader_does() {
        let values = parse(r#"{"A": true, "B": 7, "C": "x"}"#).unwrap();
        assert_eq!(values.get("A").map(String::as_str), Some("true"));
        assert_eq!(values.get("B").map(String::as_str), Some("7"));
        assert_eq!(values.get("C").map(String::as_str), Some("x"));
    }

    /// A refusal names the flag it choked on.
    #[test]
    fn a_value_with_no_string_form_is_refused_by_name() {
        let e = parse(r#"{"Good": "1", "Bad": {"nested": 1}}"#).unwrap_err();
        assert!(e.contains("Bad"), "{e}");
        let e = parse(r#"{"Nulled": null}"#).unwrap_err();
        assert!(e.contains("Nulled"), "{e}");
        let e = parse(r#"{"Listy": [1,2]}"#).unwrap_err();
        assert!(e.contains("Listy"), "{e}");
    }

    /// Clearing is not a parse failure.
    #[test]
    fn an_empty_document_clears_rather_than_failing() {
        assert!(parse("").unwrap().is_empty());
        assert!(parse("   \n  ").unwrap().is_empty());
        assert!(parse("{}").unwrap().is_empty());
    }

    /// Not an object is refused before anything else looks at it.
    #[test]
    fn a_document_that_is_not_an_object_is_refused() {
        for text in ["[1,2,3]", "\"a string\"", "42", "true", "{oops"] {
            assert!(parse(text).is_err(), "{text} must be refused");
        }
    }

    /// **The bug in the fork's version of this check**: `FLog`/`DFLog` were
    /// treated as integers, and Roblox's own settings document has
    /// `FLogAudio = "Info"` and `DFLogWebSocketTraceError = "Warning,6"`.
    #[test]
    fn log_channels_take_severity_names_as_well_as_numbers() {
        assert_eq!(kind_of("FLogAudio"), Kind::Log);
        assert_eq!(kind_of("DFLogWebSocketTraceError"), Kind::Log);
        assert_eq!(check("FLogAudio", "Info").unwrap(), "Info");
        assert_eq!(check("DFLogWebSocketTraceError", "Warning,6").unwrap(), "Warning,6");
        assert_eq!(check("FLogNetwork", "7").unwrap(), "7");
        assert_eq!(check("FLogNativeDM", "Verbose").unwrap(), "Verbose");
    }

    #[test]
    fn the_prefix_decides_the_type_with_or_without_d_and_s() {
        assert_eq!(kind_of("FFlagDebugDisplayFPS"), Kind::Bool);
        assert_eq!(kind_of("DFFlagX"), Kind::Bool);
        assert_eq!(kind_of("SFFlagX"), Kind::Bool);
        assert_eq!(kind_of("DFIntTaskSchedulerTargetFps"), Kind::Int);
        assert_eq!(kind_of("FStringGraphicsTextureManager2DenyPattern2"), Kind::Text);
        assert_eq!(kind_of("CordialSomething"), Kind::Unknown);
        assert_eq!(kind_of("Dx"), Kind::Unknown);
    }

    #[test]
    fn booleans_are_normalised_to_the_spelling_roblox_uses() {
        assert_eq!(check("FFlagA", "true").unwrap(), "True");
        assert_eq!(check("FFlagA", "FALSE").unwrap(), "False");
        assert_eq!(check("DFFlagA", " True ").unwrap(), "True");
        assert!(check("FFlagA", "yes").is_err());
        assert!(check("FFlagA", "1").is_err());
    }

    #[test]
    fn numbers_must_be_whole_numbers() {
        assert_eq!(check("DFIntX", "144").unwrap(), "144");
        assert_eq!(check("FIntX", "-1").unwrap(), "-1");
        assert_eq!(check("FIntX", "144.0").unwrap(), "144", "a whole float is a whole number");
        for bad in ["", "-", "1.5", "fast", "1e3", "99999999999999999999", "144.", ".5", "true"] {
            assert!(check("FIntX", bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn text_and_unknown_flags_take_anything_and_a_bad_name_is_refused() {
        assert_eq!(check("FStringX", ".*").unwrap(), ".*");
        assert_eq!(check("CordialX", "anything at all").unwrap(), "anything at all");
        assert!(check("", "1").is_err());
        assert!(check("FFlag A", "True").is_err());
    }

    /// Bloxstrap's and Fishstrap's `ClientAppSettings.json`.
    #[test]
    fn a_bloxstrap_client_app_settings_file_imports() {
        let text = r#"{
            "DFIntTaskSchedulerTargetFps": "144",
            "FFlagDebugDisplayFPS": "true",
            "FIntRenderShadowIntensity": 0,
            "FLogAudio": "Info",
            "DFLogWebSocketTraceError": "Warning,6"
        }"#;
        let got = import(text).unwrap();
        assert_eq!(got.format, Format::Flat);
        assert!(got.skipped.is_empty(), "{:?}", got.skipped);
        assert_eq!(got.flags.len(), 5);
        assert_eq!(got.flags["FFlagDebugDisplayFPS"], "True");
        assert_eq!(got.flags["FIntRenderShadowIntensity"], "0");
        assert_eq!(got.flags["FLogAudio"], "Info");
        assert_eq!(got.flags["DFLogWebSocketTraceError"], "Warning,6");
    }

    /// Sober's config: only `fflags`, JSON bools and numbers allowed, and the
    /// placeholder a stock install carries is not a flag anybody set.
    #[test]
    fn a_sober_config_contributes_only_its_fflags() {
        let text = r#"{
            "use_opengl": false,
            "discord_rpc_enabled": true,
            "fflags": {
                "FFlagExample": true,
                "DFIntTaskSchedulerTargetFps": 144,
                "FFlagDebugDisplayFPS": true,
                "FLogAudio": "Info"
            }
        }"#;
        let got = import(text).unwrap();
        assert_eq!(got.format, Format::Sober);
        assert_eq!(got.flags.len(), 3, "{:?}", got.flags);
        assert_eq!(got.flags["DFIntTaskSchedulerTargetFps"], "144");
        assert_eq!(got.flags["FFlagDebugDisplayFPS"], "True");
        assert!(!got.flags.contains_key("FFlagExample"));
        assert!(got.skipped.is_empty());

        assert!(import(r#"{"fflags": {"FFlagExample": true}}"#).unwrap().flags.is_empty());
        assert!(import(r#"{"fflags": []}"#).is_err(), "an fflags that is not an object is named");
        // No `fflags` at all is a flat document, not a Sober config.
        assert_eq!(import(r#"{"use_opengl": true}"#).unwrap().format, Format::Flat);
    }

    /// **Per entry.** One bad value skips itself and is named; the rest land.
    #[test]
    fn a_bad_entry_is_skipped_by_name_and_the_rest_are_kept() {
        let text = r#"{
            "FFlagGood": "True",
            "FFlagBad": "maybe",
            "DFIntBad": "fast",
            "DFIntGood": "5",
            "FStringNested": {"a": 1},
            "FStringNull": null,
            "FLogAudio": "Info"
        }"#;
        let got = import(text).unwrap();
        let kept: Vec<&str> = got.flags.keys().map(String::as_str).collect();
        assert_eq!(kept, ["DFIntGood", "FFlagGood", "FLogAudio"]);
        let skipped: Vec<&str> = got.skipped.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(skipped, ["DFIntBad", "FFlagBad", "FStringNested", "FStringNull"]);
        for s in &got.skipped {
            assert!(!s.why.is_empty() && !s.why.starts_with(&s.name), "{s:?}");
        }
    }

    #[test]
    fn a_document_that_is_not_an_object_is_an_error_and_a_prefixless_name_is_kept_and_noted() {
        assert!(import("[1]").is_err());
        assert!(import("{oops").is_err());
        let got = import(r#"{"CordialX": "1", "FFlagA": "True"}"#).unwrap();
        assert_eq!(got.unrecognised, ["CordialX"]);
        assert_eq!(got.flags.len(), 2);
    }

    #[test]
    fn merging_says_what_was_new_and_what_changed() {
        let mut have = parse(r#"{"FFlagA": "True", "FIntB": "1", "FIntSame": "3"}"#).unwrap();
        let incoming = parse(r#"{"FFlagA": "False", "FIntC": "2", "FIntSame": "3"}"#).unwrap();
        assert_eq!(merge(&mut have, &incoming), (1, 1));
        assert_eq!(have.len(), 4);
        assert_eq!(have["FFlagA"], "False");
        assert_eq!(have["FIntB"], "1", "what the import did not mention stays");
    }

    /// The imported document round-trips through the same writer and parser the
    /// client's loader uses.
    #[test]
    fn an_import_written_out_reads_back_identically() {
        let got = import(r#"{"FFlagA": true, "FLogAudio": "Info", "DFIntX": 7.0}"#).unwrap();
        let dir = std::env::temp_dir().join(format!("cordial-flagimport-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("flags.json");
        write(&path, &got.flags).unwrap();
        assert_eq!(parse(&std::fs::read_to_string(&path).unwrap()).unwrap(), got.flags);
        assert_eq!(got.flags["DFIntX"], "7");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The write is atomic in the sense that matters: no reader ever sees the
    /// destination half-written, because it is renamed into place.
    #[test]
    fn writing_replaces_rather_than_appends() {
        let dir = std::env::temp_dir().join(format!("cordial-flagdoc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("flags.json");

        write(&path, &parse(r#"{"A": "1", "B": "2"}"#).unwrap()).unwrap();
        write(&path, &parse(r#"{"C": "3"}"#).unwrap()).unwrap();

        let back = parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back.len(), 1, "the second write must replace the first");
        assert_eq!(back.get("C").map(String::as_str), Some("3"));
        assert!(!dir.join("flags.json.new").exists(), "the temporary must not be left behind");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
