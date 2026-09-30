//! `stacked flags`: a profile's FastFlags from the command line.
//!
//! Before this, the only flag command was `flags path`, and everything past
//! finding the file was a text editor and a JSON syntax the user had to get
//! right by hand. A missing comma loses every flag in the file at the next
//! launch -- the client reports the document as malformed and ignores it --
//! and the report goes to a terminal that a desktop launch does not have.
//!
//! So every change here goes through `cordial_plugins::flag_document`, the one
//! parser the client also uses, and is written with its atomic `write`. A value
//! this command accepts is a value the client will read.
//!
//! What this adds beyond the file is a type check. Roblox's flag names carry
//! their type in the prefix: `FFlag` is a boolean, `FInt` and `FLog` are
//! integers, `FString` is text, and each can be `D`- or `S`-prefixed. A
//! boolean flag set to `yes` is not an error anywhere the user would see it;
//! the engine reads it as whatever its parser makes of it and the flag
//! "does nothing". Refused here, by name, it costs the user one retype.
//! A name with no recognised prefix is written with a warning rather than
//! refused, because `Cordial`-prefixed names ride the same layering (see
//! `cordial_runtime::flags`) and a list of every legitimate prefix is not
//! something this file can promise to keep complete.

use cordial_plugins::flag_document;
use cordial_shell::profile;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub fn run(args: &[String]) -> u8 {
    let (named, rest) = match crate::take_profile(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("stacked: {e}");
            return 2;
        }
    };
    let name = crate::chosen_profile(named);
    let path = match profile::dir(&name) {
        Ok(dir) => flag_document::path_in(&dir),
        Err(e) => {
            eprintln!("stacked: {e}");
            return 1;
        }
    };
    let arg = |i: usize| rest.get(i).map(String::as_str);
    match arg(0) {
        None | Some("list" | "show") => list(&path),
        Some("path") => {
            println!("{}", path.display());
            0
        }
        Some("get") => match arg(1) {
            Some(flag) => get(&path, flag),
            None => usage("flags get needs a NAME"),
        },
        Some("set") => match (arg(1), rest.len() > 2) {
            (Some(flag), true) => set(&path, flag, &rest[2..].join(" ")),
            _ => usage("flags set needs a NAME and a VALUE"),
        },
        Some("unset" | "remove" | "rm") => match arg(1) {
            Some(flag) => unset(&path, flag),
            None => usage("flags unset needs a NAME"),
        },
        Some("import") => {
            let replace = rest.iter().any(|a| a == "--replace");
            match rest.iter().skip(1).find(|a| *a != "--replace") {
                Some(file) => import(&path, file, replace),
                None => usage("flags import needs a FILE, or - to read standard input"),
            }
        }
        Some("clear") => match flag_document::write(&path, &BTreeMap::new()) {
            Ok(()) => {
                println!("cleared every flag in {}", path.display());
                0
            }
            Err(e) => fail(&e),
        },
        Some("edit") => edit(&path),
        Some(other) => {
            eprintln!("stacked: unknown flags command {other:?}. Try `stacked help`.");
            2
        }
    }
}

fn usage(message: &str) -> u8 {
    eprintln!("stacked: {message}");
    2
}

fn fail(message: &str) -> u8 {
    eprintln!("stacked: {message}");
    1
}

/// The document at `path`, or an empty one if there is no file yet.
///
/// A file that exists and does not parse is an error rather than an empty set:
/// treating it as empty and then writing one flag into it would replace
/// somebody's two hundred hand-written lines with one.
fn read(path: &Path) -> Result<BTreeMap<String, String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => flag_document::parse(&text).map_err(|e| {
            format!(
                "{} is not a usable flags file ({e}). `stacked flags edit` opens it to fix, \
                 or `stacked flags clear` starts again.",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn list(path: &Path) -> u8 {
    let flags = match read(path) {
        Ok(f) => f,
        Err(e) => return fail(&e),
    };
    if flags.is_empty() {
        println!("no FastFlags set. `stacked flags set NAME VALUE` adds one.");
    } else {
        let width = flags.keys().map(String::len).max().unwrap_or(0);
        for (name, value) in &flags {
            println!("{name:<width$}  {value}");
        }
    }
    println!("\n{}", path.display());
    0
}

fn get(path: &Path, flag: &str) -> u8 {
    match read(path) {
        Ok(flags) => match flags.get(flag) {
            Some(value) => {
                println!("{value}");
                0
            }
            None => {
                eprintln!("stacked: {flag} is not set in this profile's flags");
                1
            }
        },
        Err(e) => fail(&e),
    }
}

fn set(path: &Path, flag: &str, raw: &str) -> u8 {
    let value = match check(flag, raw) {
        Ok(v) => v,
        Err(e) => return usage(&e),
    };
    if kind_of(flag) == Kind::Unknown {
        eprintln!(
            "stacked: warning: {flag} does not start with a FastFlag prefix \
             (FFlag, FInt, FString, FLog, or those with D or S in front). Written anyway."
        );
    }
    let mut flags = match read(path) {
        Ok(f) => f,
        Err(e) => return fail(&e),
    };
    flags.insert(flag.to_string(), value.clone());
    match flag_document::write(path, &flags) {
        Ok(()) => {
            println!("{flag} = {value}{}", when_it_applies(flag));
            0
        }
        Err(e) => fail(&e),
    }
}

fn unset(path: &Path, flag: &str) -> u8 {
    let mut flags = match read(path) {
        Ok(f) => f,
        Err(e) => return fail(&e),
    };
    if flags.remove(flag).is_none() {
        eprintln!("stacked: {flag} was not set");
        return 1;
    }
    match flag_document::write(path, &flags) {
        Ok(()) => {
            println!("unset {flag}");
            0
        }
        Err(e) => fail(&e),
    }
}

/// Merge a Bloxstrap-style export into the profile's flags.
///
/// Every value is checked before anything is written, so a paste with one bad
/// line changes nothing rather than half of it.
fn import(path: &Path, source: &str, replace: bool) -> u8 {
    let text = if source == "-" {
        let mut text = String::new();
        if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut text) {
            return fail(&format!("reading standard input: {e}"));
        }
        text
    } else {
        match std::fs::read_to_string(source) {
            Ok(t) => t,
            Err(e) => return fail(&format!("{source}: {e}")),
        }
    };
    let incoming = match flag_document::parse(&text) {
        Ok(f) => f,
        Err(e) => return fail(&format!("{source}: {e}")),
    };
    let mut checked = BTreeMap::new();
    for (name, value) in incoming {
        match check(&name, &value) {
            Ok(v) => {
                checked.insert(name, v);
            }
            Err(e) => return fail(&format!("{source}: {e}. Nothing was imported.")),
        }
    }
    let mut flags = if replace {
        BTreeMap::new()
    } else {
        match read(path) {
            Ok(f) => f,
            Err(e) => return fail(&e),
        }
    };
    let (mut added, mut changed) = (0, 0);
    for (name, value) in checked {
        match flags.insert(name, value.clone()) {
            None => added += 1,
            Some(old) if old != value => changed += 1,
            Some(_) => {}
        }
    }
    match flag_document::write(path, &flags) {
        Ok(()) => {
            println!("imported: {added} added, {changed} changed, {} in total", flags.len());
            0
        }
        Err(e) => fail(&e),
    }
}

/// Open the document in the user's editor, and only replace it if what comes
/// back parses.
///
/// The edit happens on a copy. Editing the live file directly would let a
/// half-finished edit -- saved, then abandoned -- reach the next launch as a
/// malformed document, which the client ignores whole.
fn edit(path: &Path) -> u8 {
    let current = std::fs::read_to_string(path).unwrap_or_else(|_| "{\n}\n".to_string());
    let draft = draft_path(path);
    if let Some(parent) = draft.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return fail(&format!("{}: {e}", parent.display()));
        }
    }
    if let Err(e) = std::fs::write(&draft, &current) {
        return fail(&format!("{}: {e}", draft.display()));
    }
    let editor = editor();
    // Through `sh -c` so an EDITOR with arguments -- `code --wait` -- works
    // the way it does for git.
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("stacked")
        .arg(&draft)
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            let _ = std::fs::remove_file(&draft);
            return fail(&format!("{editor} exited with {s}; the flags were not changed"));
        }
        Err(e) => {
            let _ = std::fs::remove_file(&draft);
            return fail(&format!("could not start {editor}: {e}. Set EDITOR to one you have."));
        }
    }
    let text = std::fs::read_to_string(&draft).unwrap_or_default();
    let parsed = flag_document::parse(&text).and_then(|flags| {
        flags
            .into_iter()
            .map(|(name, value)| check(&name, &value).map(|v| (name, v)))
            .collect::<Result<BTreeMap<_, _>, _>>()
    });
    match parsed {
        Ok(flags) => match flag_document::write(path, &flags) {
            Ok(()) => {
                let _ = std::fs::remove_file(&draft);
                println!("saved {} flag{}", flags.len(), if flags.len() == 1 { "" } else { "s" });
                0
            }
            Err(e) => fail(&e),
        },
        Err(e) => fail(&format!(
            "{e}\nThe flags were not changed. Your edit is kept at {}; \
             `stacked flags edit` starts again from the saved file.",
            draft.display()
        )),
    }
}

fn draft_path(path: &Path) -> PathBuf {
    path.with_extension("json.edit")
}

fn editor() -> String {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.trim().is_empty())
        .unwrap_or_else(|| {
            ["nano", "vi"]
                .iter()
                .find(|e| on_path(e))
                .map(|e| e.to_string())
                .unwrap_or_else(|| "vi".to_string())
        })
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Kind {
    Bool,
    Int,
    Text,
    Unknown,
}

/// What a flag's name says its value is.
fn kind_of(name: &str) -> Kind {
    let bare = name.strip_prefix('D').or_else(|| name.strip_prefix('S')).unwrap_or(name);
    if bare.starts_with("FFlag") {
        Kind::Bool
    } else if bare.starts_with("FInt") || bare.starts_with("FLog") {
        Kind::Int
    } else if bare.starts_with("FString") {
        Kind::Text
    } else {
        Kind::Unknown
    }
}

/// The value to store for `raw`, or why it cannot be.
///
/// Booleans are written `True`/`False`, which is how Roblox's own settings
/// document and every Bloxstrap export spell them, so a file edited here reads
/// the same as one pasted in.
fn check(name: &str, raw: &str) -> Result<String, String> {
    if name.trim().is_empty() {
        return Err("a flag needs a name".to_string());
    }
    if name.chars().any(char::is_whitespace) {
        return Err(format!("{name:?}: a flag name has no spaces in it"));
    }
    let value = raw.trim();
    match kind_of(name) {
        Kind::Bool => match value.to_ascii_lowercase().as_str() {
            "true" => Ok("True".to_string()),
            "false" => Ok("False".to_string()),
            _ => Err(format!("{name} is a boolean flag and takes True or False, not {value:?}")),
        },
        Kind::Int => {
            let digits = value.strip_prefix('-').unwrap_or(value);
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) && value.parse::<i64>().is_ok() {
                Ok(value.to_string())
            } else {
                Err(format!("{name} is a number flag and takes a whole number, not {value:?}"))
            }
        }
        Kind::Text | Kind::Unknown => Ok(raw.to_string()),
    }
}

/// A note on when a change reaches the engine, for the line that confirms it.
///
/// `cordial_runtime::flags` explains the split: `F*` flags are read once at
/// startup, and only the `DF*` family is re-read in a running client. The
/// difference is invisible from the file, and "I set it and nothing happened"
/// is the report it produces when a client is already open.
fn when_it_applies(name: &str) -> &'static str {
    if kind_of(name) == Kind::Unknown {
        ""
    } else {
        "  (takes effect the next time Roblox starts)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prefix_decides_the_type_with_or_without_d_and_s() {
        assert_eq!(kind_of("FFlagDebugDisplayFPS"), Kind::Bool);
        assert_eq!(kind_of("DFFlagX"), Kind::Bool);
        assert_eq!(kind_of("SFFlagX"), Kind::Bool);
        assert_eq!(kind_of("DFIntTaskSchedulerTargetFps"), Kind::Int);
        assert_eq!(kind_of("FLogNetwork"), Kind::Int);
        assert_eq!(kind_of("DFLogX"), Kind::Int);
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
        for bad in ["", "-", "1.5", "fast", "1e3", "99999999999999999999"] {
            assert!(check("FIntX", bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn text_and_unknown_flags_take_anything() {
        assert_eq!(check("FStringX", ".*").unwrap(), ".*");
        assert_eq!(check("CordialX", "anything at all").unwrap(), "anything at all");
    }

    #[test]
    fn a_name_with_spaces_or_no_name_is_refused() {
        assert!(check("", "1").is_err());
        assert!(check("FFlag A", "True").is_err());
    }

    #[test]
    fn a_malformed_file_is_an_error_and_not_an_empty_set() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flags.json");
        std::fs::write(&path, "{\"FFlagA\": \"True\",}").unwrap();
        assert!(read(&path).is_err(), "a trailing comma must not read as no flags");
        assert!(read(&dir.path().join("absent.json")).unwrap().is_empty());
    }

    #[test]
    fn set_unset_and_import_round_trip_through_the_clients_parser() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flags.json");
        assert_eq!(set(&path, "FFlagDebugDisplayFPS", "true"), 0);
        assert_eq!(set(&path, "DFIntTaskSchedulerTargetFps", "144"), 0);
        assert_eq!(set(&path, "DFIntTaskSchedulerTargetFps", "fast"), 2);
        let flags = read(&path).unwrap();
        assert_eq!(flags.get("FFlagDebugDisplayFPS").map(String::as_str), Some("True"));
        assert_eq!(flags.get("DFIntTaskSchedulerTargetFps").map(String::as_str), Some("144"));

        let export = dir.path().join("bloxstrap.json");
        std::fs::write(&export, r#"{"FFlagDebugDisplayFPS": "False", "FIntNew": 3}"#).unwrap();
        assert_eq!(import(&path, export.to_str().unwrap(), false), 0);
        let flags = read(&path).unwrap();
        assert_eq!(flags.len(), 3);
        assert_eq!(flags.get("FFlagDebugDisplayFPS").map(String::as_str), Some("False"));

        std::fs::write(&export, r#"{"FFlagBad": "maybe", "FIntOther": 1}"#).unwrap();
        assert_eq!(import(&path, export.to_str().unwrap(), false), 1);
        assert_eq!(read(&path).unwrap().len(), 3, "one bad value must import nothing");

        assert_eq!(unset(&path, "FIntNew"), 0);
        assert_eq!(unset(&path, "FIntNew"), 1);
        assert_eq!(read(&path).unwrap().len(), 2);
    }
}
