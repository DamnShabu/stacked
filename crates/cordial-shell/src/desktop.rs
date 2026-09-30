//! `stacked desktop install|remove`: the desktop entry, icon and browser
//! handler for a copy of Stacked that no package installed.
//!
//! A package or the Flatpak ships these itself. A source build did not, and
//! the README asked for five commands to put them in place by hand -- the
//! part of an install people skip, after which the website's Play button does
//! nothing or opens Sober, and `stacked` never appears in the app menu.
//!
//! **`Exec` names this binary by absolute path.** The packaged entry says
//! `Exec=stacked %u` and relies on `PATH`, which is right for `/usr/bin` and
//! wrong for `~/.local/bin`: several desktops start applications with a `PATH`
//! that does not include it, and the result is a Play button that silently
//! fails. The path is the one `current_exe` reports, so it is wherever the
//! user actually installed the binary, and re-running this after moving it
//! fixes the entry.
//!
//! The entry and icon come from `packaging/`, compiled in, so there is one
//! source for what a Stacked desktop entry says.

use std::path::{Path, PathBuf};

const APP_ID: &str = "io.github.damnshabu.Stacked";
const ENTRY: &str = include_str!("../../../packaging/io.github.damnshabu.Stacked.desktop");
const ICON: &[u8] =
    include_bytes!("../../../packaging/icons/hicolor/scalable/apps/io.github.damnshabu.Stacked.svg");
const SCHEMES: [&str; 2] = ["x-scheme-handler/roblox-player", "x-scheme-handler/roblox"];

pub fn run(args: &[String]) -> u8 {
    match args.first().map(String::as_str) {
        Some("install") => install(),
        Some("remove" | "uninstall") => remove(),
        None | Some("status") => {
            let entry = entry_path();
            println!("desktop entry: {}", if entry.is_file() { entry.display().to_string() } else { "not installed by stacked".into() });
            0
        }
        Some(other) => {
            eprintln!("stacked: unknown desktop command {other:?}; `stacked desktop install` or `remove`");
            2
        }
    }
}

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")).unwrap_or_default()
    })
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")).unwrap_or_default()
    })
}

fn entry_path() -> PathBuf {
    data_home().join("applications").join(format!("{APP_ID}.desktop"))
}

fn icon_path() -> PathBuf {
    data_home().join("icons/hicolor/scalable/apps").join(format!("{APP_ID}.svg"))
}

/// Whether a system package already installed an entry, which this must not
/// shadow with one of its own.
fn packaged_entry() -> Option<PathBuf> {
    let dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join("applications").join(format!("{APP_ID}.desktop")))
        .find(|p| p.is_file())
}

/// The packaged entry with `Exec` and `TryExec` pointing at `exe`.
///
/// Quoted per the Desktop Entry specification, which is what makes a path
/// with a space in it -- a home directory named after a person, often --
/// survive being split into arguments.
fn entry_for(exe: &Path) -> String {
    let quoted = format!("\"{}\"", exe.display().to_string().replace('\\', "\\\\").replace('"', "\\\""));
    let mut out = String::new();
    for line in ENTRY.lines() {
        if line.starts_with("Exec=") {
            out.push_str(&format!("Exec={quoted} %u\n"));
            out.push_str(&format!("TryExec={}\n", exe.display()));
        } else if !line.starts_with("TryExec=") {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn install() -> u8 {
    if Path::new("/.flatpak-info").exists() {
        println!("The Flatpak installs its own desktop entry; there is nothing to do.");
        return 0;
    }
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("stacked: cannot tell where this binary is: {e}");
            return 1;
        }
    };
    if exe.to_string_lossy().contains("/target/debug/") {
        eprintln!(
            "stacked: warning: this is a debug build in a checkout. The entry will run it from \
             there until `stacked desktop install` is run again from the installed copy."
        );
    }
    if let Some(packaged) = packaged_entry() {
        println!("A package already installed {}; only the browser handler is set.", packaged.display());
    } else {
        let entry = entry_path();
        let icon = icon_path();
        let written = write(&entry, entry_for(&exe).as_bytes()).and_then(|()| write(&icon, ICON));
        if let Err(e) = written {
            eprintln!("stacked: {e}");
            return 1;
        }
        println!("installed {}", entry.display());
        println!("installed {}", icon.display());
        // Best effort: without it some menus pick the entry up only after a
        // re-login, which is slow rather than broken.
        let _ = std::process::Command::new("update-desktop-database")
            .arg(entry.parent().unwrap_or(Path::new(".")))
            .stderr(std::process::Stdio::null())
            .status();
    }
    match set_default_handler(&format!("{APP_ID}.desktop")) {
        Ok(()) => {
            println!("the website's Play button now opens Stacked");
            0
        }
        Err(e) => {
            eprintln!("stacked: could not register Stacked for roblox-player: links: {e}");
            1
        }
    }
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension("stacked-new");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Make `desktop_file` the default for Roblox's link schemes.
///
/// `xdg-mime` when it is there, because it knows each desktop's quirks; the
/// user's `mimeapps.list` directly when it is not, which is the file
/// `xdg-mime` itself writes and every desktop reads.
fn set_default_handler(desktop_file: &str) -> Result<(), String> {
    let via_xdg = std::process::Command::new("xdg-mime")
        .arg("default")
        .arg(desktop_file)
        .args(SCHEMES)
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if via_xdg {
        return Ok(());
    }
    let path = config_home().join("mimeapps.list");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = SCHEMES.iter().fold(text, |t, scheme| with_default(&t, scheme, Some(desktop_file)));
    write(&path, updated.as_bytes())
}

/// The user's own default for `scheme`, from `mimeapps.list`, or `None` when
/// there is no file or it names nothing.
pub(crate) fn user_default(scheme: &str) -> Option<String> {
    let text = std::fs::read_to_string(config_home().join("mimeapps.list")).ok()?;
    default_in(&text, scheme)
}

fn default_in(text: &str, scheme: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == "[Default Applications]";
        } else if in_section {
            if let Some((key, value)) = trimmed.split_once('=') {
                if key.trim() == scheme {
                    // A list is allowed; the first entry is the default.
                    return value.split(';').map(str::trim).find(|v| !v.is_empty()).map(String::from);
                }
            }
        }
    }
    None
}

/// `text`, a `mimeapps.list`, with `key` in `[Default Applications]` set to
/// `value`, or removed for `None`. Every other line is kept as it was.
fn with_default(text: &str, key: &str, value: Option<&str>) -> String {
    const SECTION: &str = "[Default Applications]";
    let mut out: Vec<String> = Vec::new();
    let mut in_section = false;
    let mut seen_section = false;
    let mut done = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_section && !done {
                if let Some(v) = value {
                    out.push(format!("{key}={v}"));
                }
                done = true;
            }
            in_section = trimmed == SECTION;
            seen_section |= in_section;
        } else if in_section && trimmed.split('=').next().map(str::trim) == Some(key) {
            if !done {
                if let Some(v) = value {
                    out.push(format!("{key}={v}"));
                }
                done = true;
            }
            continue;
        }
        out.push(line.to_string());
    }
    if !done {
        if let Some(v) = value {
            if !seen_section {
                if !out.is_empty() {
                    out.push(String::new());
                }
                out.push(SECTION.to_string());
            }
            out.push(format!("{key}={v}"));
        }
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    joined
}

fn remove() -> u8 {
    let mut code = 0;
    for path in [entry_path(), icon_path()] {
        match std::fs::remove_file(&path) {
            Ok(()) => println!("removed {}", path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!("stacked: {}: {e}", path.display());
                code = 1;
            }
        }
    }
    // Only a default that names Stacked is removed; one the user pointed at
    // Sober since is theirs.
    let list = config_home().join("mimeapps.list");
    if let Ok(text) = std::fs::read_to_string(&list) {
        let ours = format!("{APP_ID}.desktop");
        let mut updated = text.clone();
        for scheme in SCHEMES {
            let current = format!("{scheme}={ours}");
            if updated.lines().any(|l| l.trim() == current) {
                updated = with_default(&updated, scheme, None);
            }
        }
        if updated != text {
            match write(&list, updated.as_bytes()) {
                Ok(()) => println!("Stacked no longer handles the website's Play button"),
                Err(e) => {
                    eprintln!("stacked: {e}");
                    code = 1;
                }
            }
        }
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_names_the_binary_by_quoted_absolute_path_and_keeps_the_url() {
        let entry = entry_for(Path::new("/home/a user/.local/bin/stacked"));
        assert!(entry.contains("\nExec=\"/home/a user/.local/bin/stacked\" %u\n"), "{entry}");
        assert!(entry.contains("\nTryExec=/home/a user/.local/bin/stacked\n"), "{entry}");
        assert!(entry.contains("MimeType=x-scheme-handler/roblox-player;x-scheme-handler/roblox;"));
        assert_eq!(entry.matches("Exec=").count(), 2, "one Exec and one TryExec:\n{entry}");
    }

    #[test]
    fn the_default_is_set_in_its_section_and_nothing_else_moves() {
        let before = "[Added Associations]\ntext/plain=gedit.desktop;\n\n[Default Applications]\nx-scheme-handler/roblox-player=org.vinegarhq.Sober.desktop\ntext/html=firefox.desktop\n";
        let after = with_default(before, "x-scheme-handler/roblox-player", Some("s.desktop"));
        assert!(after.contains("[Default Applications]\nx-scheme-handler/roblox-player=s.desktop\ntext/html=firefox.desktop\n"), "{after}");
        assert!(after.contains("text/plain=gedit.desktop;"), "{after}");
        assert!(!after.contains("Sober"), "{after}");

        let fresh = with_default("", "x-scheme-handler/roblox", Some("s.desktop"));
        assert_eq!(fresh, "[Default Applications]\nx-scheme-handler/roblox=s.desktop\n");

        let removed = with_default(&after, "x-scheme-handler/roblox-player", None);
        assert!(!removed.contains("roblox-player"), "{removed}");
        assert!(removed.contains("text/html=firefox.desktop"), "{removed}");
    }

    #[test]
    fn the_default_is_read_from_its_section_only() {
        let text = "[Added Associations]\nx-scheme-handler/roblox=a.desktop\n[Default Applications]\nx-scheme-handler/roblox=b.desktop;c.desktop;\n";
        assert_eq!(default_in(text, "x-scheme-handler/roblox").as_deref(), Some("b.desktop"));
        assert_eq!(default_in(text, "x-scheme-handler/roblox-player"), None);
    }

    #[test]
    fn a_key_in_another_section_is_not_the_default() {
        let text = "[Added Associations]\nx-scheme-handler/roblox=a.desktop\n";
        let after = with_default(text, "x-scheme-handler/roblox", Some("s.desktop"));
        assert!(after.starts_with("[Added Associations]\nx-scheme-handler/roblox=a.desktop\n"), "{after}");
        assert!(after.ends_with("[Default Applications]\nx-scheme-handler/roblox=s.desktop\n"), "{after}");
    }
}
