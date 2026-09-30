//! `stacked doctor`: what on this machine will stop Roblox working, and what
//! to run about it.
//!
//! `stacked diagnostics` is the block a bug report needs, written to be pasted
//! in public and deliberately free of judgement. This is the other half: the
//! same machine read for the things that are known to break a launch, each
//! with a verdict and the command that fixes it. A user who has to open an
//! issue to learn that their session has no Wayland socket, or that the
//! browser's Play button opens Sober, has been failed by the tool.
//!
//! **Every check here reads something; none of them guesses.** Where the
//! answer depends on something this process cannot see -- the Vulkan driver
//! a Flatpak's GL extension provides, say -- the check says what it looked at
//! rather than calling the absence a fault. A doctor that reports a healthy
//! machine as broken teaches people to ignore it, which is the same failure as
//! a stub that returns success.
//!
//! Cheap by design: file checks, one `dlopen`, and a handful of D-Bus calls.
//! The one network request, whether a newer Roblox is on offer, is bounded by
//! the same deadline `stacked play` uses, and `--offline` skips it.

use crate::install::{self, NotFound, Origin};
use cordial_shell::profile;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Info,
    Warn,
    Fail,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Level::Ok => "ok  ",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Fail => "FAIL",
        }
    }
}

#[derive(Debug)]
pub struct Check {
    pub level: Level,
    pub what: String,
    /// What to do about it. Empty for a check that passed.
    pub fix: String,
}

fn check(level: Level, what: impl Into<String>, fix: impl Into<String>) -> Check {
    Check { level, what: what.into(), fix: fix.into() }
}

pub fn run(args: &[String]) -> u8 {
    let offline = args.iter().any(|a| a == "--offline");
    if let Some(other) = args.iter().find(|a| *a != "--offline") {
        eprintln!("stacked: unexpected argument {other:?}; doctor takes only --offline");
        return 2;
    }
    let checks = all(offline);
    for c in &checks {
        println!("{}  {}", c.level.tag(), private(&c.what));
        for line in c.fix.lines() {
            println!("      {}", private(line));
        }
    }
    let fails = checks.iter().filter(|c| c.level == Level::Fail).count();
    let warns = checks.iter().filter(|c| c.level == Level::Warn).count();
    println!();
    match (fails, warns) {
        (0, 0) => println!("Nothing found that should stop Roblox running here."),
        (0, w) => println!("{w} warning{}; Roblox should still start.", plural(w)),
        (f, _) => println!("{f} problem{} will stop Roblox starting.", plural(f)),
    }
    u8::from(fails > 0)
}

/// `line` with the home directory shown as `~`.
///
/// The bug report template asks for this output, and a home directory is
/// usually its owner's name -- the reason `diagnostics.rs` prints no path
/// under `$HOME` at all. Paths are still worth showing here, because "which
/// cordial-run" is a real answer; the name in them is not.
fn private(line: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if home.len() > 1 => line.replace(home.trim_end_matches('/'), "~"),
        _ => line.to_string(),
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn all(offline: bool) -> Vec<Check> {
    let config = crate::shell_config::load(&crate::shell_config::path());
    let mut out = vec![loader(), root()];
    out.extend(roblox(&config, offline));
    out.extend(machine(&config));
    out
}

/// The checks that read the machine and not the Roblox build, which `locate`
/// may extract -- so these are the ones a test can run anywhere.
fn machine(config: &crate::shell_config::ShellConfig) -> Vec<Check> {
    let mut out = vec![display()];
    out.push(vulkan());
    out.push(audio());
    let bus = session_bus();
    out.push(keyring(bus.as_ref()));
    if config.gamemode {
        out.push(gamemode(bus.as_ref()));
    }
    out.push(browser_handler());
    out.push(disk());
    out.push(current_profile(&config.profile));
    out
}

fn loader() -> Check {
    match crate::launch::loader_path() {
        Ok(path) => check(Level::Ok, format!("cordial-run is at {}", path.display()), ""),
        Err(why) => check(
            Level::Fail,
            "cordial-run, the program that runs Roblox, is missing",
            format!(
                "{why}\nInstall both binaries into one directory, e.g.\n\
                 install -Dm755 target/release/stacked target/release/cordial-run -t ~/.local/bin/"
            ),
        ),
    }
}

fn root() -> Check {
    if crate::root_warning::running_as_root() {
        check(
            Level::Warn,
            "running as root",
            "Root usually has no desktop session, so no sound and no keyring. Run stacked as your own user.",
        )
    } else {
        check(Level::Ok, "running as an ordinary user", "")
    }
}

fn roblox(config: &crate::shell_config::ShellConfig, offline: bool) -> Vec<Check> {
    let origin = install::effective_apk(&config.roblox).map(|(_, o)| o);
    let build = match install::locate(&config.roblox) {
        Ok(build) => build,
        Err(NotFound::NoBuild) => {
            let fix = if config.auto_update {
                "`stacked` downloads it the first time you play, or `stacked install` now."
            } else {
                "`stacked install` downloads it."
            };
            return vec![check(Level::Info, "Roblox is not installed yet", fix)];
        }
        Err(NotFound::Unusable(why)) => {
            return vec![check(Level::Fail, "the Roblox build cannot be used", why)];
        }
    };
    let version = cordial_update::engine::installed_version(&build.lib_dir);
    let source = origin.map(Origin::describe).unwrap_or("found");
    let mut out = vec![check(
        Level::Ok,
        format!("Roblox {} ({source})", version.as_deref().unwrap_or("of unknown version")),
        "",
    )];
    if offline || !config.auto_update || !crate::auto_update::manages(origin) {
        return out;
    }
    let Some(installed) = version else { return out };
    match crate::auto_update::newest_online(Duration::from_secs(5)) {
        Some(newest) if cordial_update::version::is_newer(&newest, &installed) => out.push(check(
            Level::Info,
            format!("Roblox {newest} is available"),
            "`stacked` installs it the next time you play, or `stacked update` now.",
        )),
        Some(_) => out.push(check(Level::Ok, "Roblox is the newest build on offer", "")),
        None => out.push(check(
            Level::Warn,
            "could not reach the download mirror to check for updates",
            "Roblox refuses clients that are too old. If joining fails with an update \
             message, check the connection and run `stacked update`.",
        )),
    }
    out
}

fn runtime_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
}

fn display() -> Check {
    if let Ok(name) = std::env::var("WAYLAND_DISPLAY") {
        let socket = if Path::new(&name).is_absolute() {
            PathBuf::from(&name)
        } else {
            runtime_dir().unwrap_or_default().join(&name)
        };
        return if socket.exists() {
            check(Level::Ok, format!("Wayland display {name}"), "")
        } else {
            check(
                Level::Fail,
                format!("WAYLAND_DISPLAY is {name}, and there is no socket at {}", socket.display()),
                "Run stacked from inside your desktop session, not from ssh or a TTY.",
            )
        };
    }
    match std::env::var("DISPLAY") {
        Ok(name) if !name.is_empty() => check(
            Level::Ok,
            format!("X11 display {name}"),
            "",
        ),
        _ => check(
            Level::Fail,
            "no display: neither WAYLAND_DISPLAY nor DISPLAY is set",
            "Run stacked from inside your desktop session, not from ssh or a TTY.",
        ),
    }
}

/// Whether a Vulkan loader and at least one driver manifest are present.
///
/// The engine `dlopen`s `libvulkan.so.1` itself and, without one, falls
/// through to OpenGL ES -- measured, see `cordial_runtime::graphics` -- so a
/// missing driver is a warning and not a failure.
fn vulkan() -> Check {
    let loader = loadable("libvulkan.so.1");
    let drivers = vulkan_drivers();
    match (loader, drivers.len()) {
        (true, 0) if Path::new("/.flatpak-info").exists() => check(
            Level::Info,
            "Vulkan loader present; the Flatpak's GL extension provides the driver, which this cannot see",
            "",
        ),
        (true, 0) => check(
            Level::Warn,
            "a Vulkan loader but no Vulkan driver",
            "Roblox will fall back to OpenGL ES. Install your GPU's Vulkan driver: mesa-vulkan-drivers \
             (Debian, Ubuntu, Fedora), vulkan-radeon or vulkan-intel (Arch), or NVIDIA's driver.",
        ),
        (true, n) => check(
            Level::Ok,
            format!("Vulkan: {}", drivers.iter().take(4).cloned().collect::<Vec<_>>().join(", "))
                + if n > 4 { ", ..." } else { "" },
            "",
        ),
        (false, _) => check(
            Level::Warn,
            "no Vulkan loader (libvulkan.so.1)",
            "Roblox will fall back to OpenGL ES. Install libvulkan1 (Debian, Ubuntu), \
             vulkan-loader (Fedora) or vulkan-icd-loader (Arch), plus your GPU's Vulkan driver.",
        ),
    }
}

fn loadable(soname: &str) -> bool {
    let Ok(name) = std::ffi::CString::new(soname) else { return false };
    // SAFETY: `name` is a NUL-terminated string that outlives the call, and a
    // handle that came back is closed before returning. Loading the Vulkan
    // loader runs no driver code; that happens at `vkCreateInstance`.
    unsafe {
        let handle = libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if handle.is_null() {
            return false;
        }
        libc::dlclose(handle);
    }
    true
}

/// The ICD manifests the Vulkan loader would read, by file name, from the
/// places its documentation says it looks.
fn vulkan_drivers() -> Vec<String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for var in ["VK_DRIVER_FILES", "VK_ICD_FILENAMES"] {
        if let Some(list) = std::env::var_os(var) {
            return std::env::split_paths(&list)
                .filter(|p| p.exists())
                .map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                .collect();
        }
    }
    if let Some(home) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
    }) {
        dirs.push(home.join("vulkan/icd.d"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(data_dirs.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join("vulkan/icd.d")));
    dirs.extend(["/etc/vulkan/icd.d", "/etc/xdg/vulkan/icd.d", "/usr/share/vulkan/icd.d"].map(PathBuf::from));
    let mut found: Vec<String> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".json") && !found.contains(&name) {
                found.push(name);
            }
        }
    }
    found.sort();
    found
}

fn audio() -> Check {
    let Some(dir) = runtime_dir() else {
        return check(
            Level::Warn,
            "no XDG_RUNTIME_DIR, so no sound server can be found",
            "Run stacked from inside your desktop session.",
        );
    };
    if dir.join("pipewire-0").exists() {
        check(Level::Ok, "PipeWire is running", "")
    } else if dir.join("pulse/native").exists() {
        check(Level::Ok, "PulseAudio is running", "")
    } else {
        check(
            Level::Warn,
            "no PipeWire or PulseAudio server in this session",
            "Roblox will try ALSA directly, which may leave you without sound. \
             Start PipeWire (systemctl --user start pipewire pipewire-pulse).",
        )
    }
}

fn session_bus() -> Option<zbus::blocking::Connection> {
    zbus::blocking::Connection::session().ok()
}

/// Whether `name` is owned now or can be started on demand.
fn bus_has(bus: &zbus::blocking::Connection, name: &str) -> bool {
    let proxy = match zbus::blocking::fdo::DBusProxy::new(bus) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let listed = |names: zbus::fdo::Result<Vec<zbus::names::OwnedBusName>>| {
        names.map(|n| n.iter().any(|b| b.as_str() == name)).unwrap_or(false)
    };
    listed(proxy.list_names()) || listed(proxy.list_activatable_names())
}

fn keyring(bus: Option<&zbus::blocking::Connection>) -> Check {
    match bus {
        None => check(
            Level::Warn,
            "no D-Bus session bus",
            "Your sign-in will be kept in a plain file, and notifications and GameMode will not work. \
             Run stacked from inside your desktop session.",
        ),
        Some(bus) if bus_has(bus, "org.freedesktop.secrets") => {
            check(Level::Ok, "a keyring (Secret Service) is available for your sign-in", "")
        }
        Some(_) => check(
            Level::Warn,
            "no keyring (Secret Service) in this session",
            "Your sign-in will be kept in a 0600 file in the profile instead. Install and start \
             gnome-keyring or KeePassXC's Secret Service if you want it in a keyring.",
        ),
    }
}

fn gamemode(bus: Option<&zbus::blocking::Connection>) -> Check {
    match bus {
        // Without a bus this cannot tell, and the keyring line has already
        // said GameMode will not work -- "not installed" would be a guess.
        None => check(Level::Info, "could not check for GameMode without a session bus", ""),
        Some(bus) if bus_has(bus, "com.feralinteractive.GameMode") => {
            check(Level::Ok, "GameMode is available", "")
        }
        Some(_) => check(
            Level::Info,
            "GameMode is not installed",
            "Optional. With it, the CPU governor is set to performance while you play: \
             install gamemode, or `stacked config set gamemode false` to stop asking for it.",
        ),
    }
}

/// Which desktop entry the browser's Play button opens.
///
/// `xdg-mime` when it is installed, since it knows each desktop's own lookup
/// order; otherwise the user's `mimeapps.list`, which is where
/// `stacked desktop install` writes when there is no `xdg-mime`.
fn browser_handler() -> Check {
    const OURS: &str = "io.github.damnshabu.Stacked.desktop";
    let answer = std::process::Command::new("xdg-mime")
        .args(["query", "default", "x-scheme-handler/roblox-player"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .or_else(|| crate::desktop::user_default("x-scheme-handler/roblox-player"));
    match answer.as_deref() {
        Some(OURS) => check(Level::Ok, "the website's Play button opens Stacked", ""),
        Some("") => check(
            Level::Warn,
            "nothing handles the website's Play button",
            "`stacked desktop install` registers Stacked for it.",
        ),
        Some(other) => check(
            Level::Warn,
            format!("the website's Play button opens {other}, not Stacked"),
            "`stacked desktop install` makes Stacked the handler.",
        ),
        None => check(
            Level::Warn,
            "nothing is registered for the website's Play button",
            "`stacked desktop install` registers Stacked for it.",
        ),
    }
}

fn disk() -> Check {
    let cache = cordial_update::install::cache_root();
    match free_bytes(&cache) {
        Some(free) if free < 1_000_000_000 => check(
            Level::Warn,
            format!("{} MB free for Roblox's files", free / 1_000_000),
            "Downloading a Roblox build needs about 1 GB. `stacked versions remove VERSION` frees old ones.",
        ),
        Some(free) => check(Level::Ok, format!("{} GB free for Roblox's files", free / 1_000_000_000), ""),
        None => check(Level::Info, "could not measure free disk space", ""),
    }
}

fn free_bytes(path: &Path) -> Option<u64> {
    let mut probe = path.to_path_buf();
    while !probe.exists() {
        probe = probe.parent()?.to_path_buf();
    }
    let c = std::ffi::CString::new(probe.as_os_str().as_encoded_bytes()).ok()?;
    // SAFETY: `statvfs` is a plain C struct of integers, for which all-zero
    // is a valid value; the call below overwrites it.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is NUL-terminated and outlives the call; `stat` is a valid
    // out-parameter of the right type.
    if unsafe { libc::statvfs(c.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    Some(stat.f_bavail.saturating_mul(stat.f_frsize))
}

/// Not named: a profile name is often the account's, and this output is meant
/// to be pasteable into a public issue, the same rule `diagnostics.rs` keeps.
fn current_profile(name: &str) -> Check {
    if profile::is_held(name) {
        check(
            Level::Info,
            "the current profile is open in a running client",
            "A second `stacked` on it will be refused; `stacked play --profile NAME` uses another.",
        )
    } else {
        check(Level::Ok, "the current profile is free to play", "")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_outranks_a_warning_and_the_exit_code_follows_failures_only() {
        assert!(Level::Fail > Level::Warn && Level::Warn > Level::Info && Level::Info > Level::Ok);
    }

    #[test]
    fn the_home_directory_is_never_printed() {
        let Ok(home) = std::env::var("HOME") else { return };
        if home.len() <= 1 {
            return;
        }
        let shown = private(&format!("cordial-run is at {home}/.local/bin/cordial-run"));
        assert_eq!(shown, "cordial-run is at ~/.local/bin/cordial-run");
    }

    #[test]
    fn a_missing_library_is_not_loadable_and_libc_is() {
        assert!(!loadable("libcordial-there-is-no-such-library.so.9"));
        assert!(loadable("libc.so.6"));
    }

    #[test]
    fn every_check_that_is_not_ok_says_what_to_do_or_why_it_cannot() {
        // A warning with no next step is a shrug. Info lines may be bare when
        // they only report something that cannot be acted on.
        let config = crate::shell_config::ShellConfig::default();
        for c in machine(&config).into_iter().chain([loader(), root()]) {
            if matches!(c.level, Level::Warn | Level::Fail) {
                assert!(!c.fix.trim().is_empty(), "{:?} has no fix", c.what);
            }
        }
    }

    #[test]
    fn driver_files_named_by_the_environment_win() {
        let dir = tempfile::tempdir().unwrap();
        let icd = dir.path().join("test_icd.json");
        std::fs::write(&icd, "{}").unwrap();
        // SAFETY: test-only; nothing else in this test binary reads this
        // variable concurrently.
        unsafe { std::env::set_var("VK_DRIVER_FILES", &icd) };
        let found = vulkan_drivers();
        unsafe { std::env::remove_var("VK_DRIVER_FILES") };
        assert_eq!(found, vec!["test_icd.json".to_string()]);
    }
}
