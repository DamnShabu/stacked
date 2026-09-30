//! `stacked` -- the command-line launcher.
//!
//! It finds a Roblox build, picks a profile, turns the saved settings into the
//! client's environment, and starts `cordial-run`, which opens the one window
//! there is: the game's. Everything the GTK launcher that used to live here
//! did through windows -- the chooser, the settings page, the profile
//! switcher, the update dialog, the plugin pages -- is a subcommand instead,
//! and the pure logic underneath each of them (`install.rs`, `launch.rs`,
//! `shell_config.rs`, `browser_account`) is the same code it always was.
//!
//! Why a CLI rather than a window: the launcher window stayed resident for the
//! whole of a session (ADR-031) with its own GTK main loop, watchers and update
//! timer, for a user whose only interaction with it was pressing Play. A
//! terminal command that starts the game and waits costs nothing while the
//! game runs, and a desktop entry that runs it is still one click.
//!
//! Deep links still work. The desktop entry passes `%u`, and a bare
//! `roblox-player:` or `roblox:` argument is taken as "play, joining this".
//! What is gone is single-instance forwarding: the GTK application registered
//! on the session bus and handed a second invocation's link to the first. A
//! second `stacked` against a profile that is already open is refused by the
//! profile lock (ADR-012) with the holder named, which is the same outcome a
//! user saw from the old launcher when both tried to start a client.

// This binary compiles its own copies of `audio_devices.rs` and
// `root_warning.rs` rather than depending on the `cordial_shell` lib crate for
// them, so `[lints] workspace = true`'s `unsafe_code = "deny"` applies to this
// crate root independently of the `#![allow(unsafe_code)]` on `lib.rs` -- see
// that file's comment, and ADR-036, for why cordial-shell carries the allow.
#![allow(unsafe_code)]

mod audio_devices;
mod auto_update;
mod browser_account;
mod cli_config;
mod cli_flags;
mod cli_plugins;
mod cli_roblox;
mod completions;
mod crash;
mod deep_link;
mod desktop;
mod diagnostics;
mod doctor;
mod install;
mod launch;
mod root_warning;
mod shell_config;

use cordial_shell::profile;
use std::process::ExitCode;

/// Guards `CORDIAL_PROFILE_ROOT` across every test in this binary that points
/// it at a scratch directory.
///
/// **Shared rather than one per file, and that distinction is load-bearing.**
/// Two files each keeping a private mutex for the same process-wide variable
/// serialise nothing against each other; measured, when a vpn-gate test in
/// `launch.rs` read another test's scratch directory mid-assertion on one run
/// in several.
#[cfg(test)]
pub(crate) static PROFILE_ROOT_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

const USAGE: &str = "\
Usage: stacked [COMMAND] [ARGS]

With no command, starts Roblox on the current profile. A roblox-player: or
roblox: link on its own joins that experience, which is what a browser hands
over when you press Play on the website.

Playing
  play [--profile NAME] [--no-update] [--run SECS] [LINK]
                          Start Roblox. A newer Roblox build is installed
                          first unless --no-update (or `auto_update false`);
                          --run stops it after SECS seconds.
  status                  Which Roblox build and profile a launch would use.

Roblox builds
  install                 Find a Roblox build, or download one if there is none.
                          A copy Sober already downloaded is used first.
  update [--force]        Download the newest Roblox build, if it is newer than
                          the one you have. --force downloads it regardless.
  versions                List the builds kept on disk.
  versions available      List the builds that can be downloaded.
  versions get VERSION    Download one build into the store.
  versions remove VERSION Delete a kept build.
  pin VERSION [--profile NAME]   Always run VERSION on a profile.
  unpin [--profile NAME]         Run the current build again.

Profiles (each is a separate sign-in and data directory)
  profiles                List profiles; * marks the current one.
  profiles new NAME       Create a profile.
  profiles use NAME       Make NAME the current profile.
  profiles remove NAME    Delete a profile, its sign-in and its data. Asks first.

Settings
  config                  Show every setting and its value.
  config get KEY          Show one setting.
  config set KEY VALUE    Change a setting. `stacked config` lists the keys.
  config unset KEY        Put a setting back to its default.
  config path             Where the settings file is.

FastFlags (each takes --profile NAME; changes apply at the next start)
  flags                   List this profile's FastFlags.
  flags set NAME VALUE    Set one. The value is checked against the name's type.
  flags get NAME          Show one.
  flags unset NAME        Remove one.
  flags import FILE       Merge a Bloxstrap-style JSON export; - reads stdin.
                          --sober takes Sober's instead of a FILE.
                          --replace drops the flags that were there.
  flags edit              Edit the file in $EDITOR; saved only if it is valid.
  flags clear             Remove every flag.
  flags path              Where the file is.
  audio-outputs           List the audio outputs `audio_output` can name.

Plugins
  plugins                 List installed plugins and whether each is on.
  plugins install FILE    Install a plugin from a .tar.zst archive.
  plugins remove ID       Uninstall a plugin.
  plugins enable|disable ID [--profile NAME]
  plugins grant|revoke ID CAPABILITY [--profile NAME]
                          Grant only what the plugin asks for; `plugins` lists it.
  plugins prefs ID [KEY VALUE | --reset] [--profile NAME]
                          Show or change a plugin's preferences.
  plugins deno            Install Deno, which plugins with code run on.

Setting up
  doctor [--offline]      Check this machine for what would stop Roblox
                          working, and say what to run about each.
  desktop install         Add Stacked to the app menu and make it what the
                          website's Play button opens. `desktop remove` undoes it.
  completions SHELL       Print tab completion for bash, zsh or fish.

Other
  logs [--path] [--lines N] [--profile NAME]
                          Show the end of Roblox's own newest log.
  diagnostics             Print the build, distribution and install method,
                          for a bug report. Also --diagnostics.
  help                    This. Also -h, --help.
  version                 The version. Also -V, --version.

cordial-run is the loader this starts and is not meant to be run by hand.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, rest) = match args.split_first() {
        Some((first, rest)) => (first.as_str(), rest),
        None => ("play", &[][..]),
    };

    // Answered before anything touches a profile, a lock or a display: this is
    // the report that most needs to work on a machine where nothing else does.
    let code = match command {
        "diagnostics" | "--diagnostics" => {
            print!("{}", diagnostics::report());
            0
        }
        "help" | "-h" | "--help" => {
            println!("{} {}\n\n{USAGE}\n\n{}", cordial_shell::branding::NAME.to_lowercase(),
                cordial_shell::version::full(), cordial_shell::version::NOTICE);
            0
        }
        "version" | "-V" | "--version" => {
            println!("{} {}", cordial_shell::branding::NAME, cordial_shell::version::full());
            0
        }
        _ => {
            // Before anything can be launched or listed, because the storage
            // that has a login in it may still be at the pre-ADR-012 path.
            // Skipped when there is nothing to move, which is every run after
            // the first.
            profile::migrate_legacy_layout();
            match command {
                "play" => play(rest),
                "status" => status(rest),
                "install" => cli_roblox::install(),
                "update" => cli_roblox::update(rest),
                "versions" => cli_roblox::versions(rest),
                "pin" => cli_roblox::pin(rest),
                "unpin" => cli_roblox::unpin(rest),
                "profiles" | "profile" => profiles(rest),
                "config" => cli_config::run(rest),
                "flags" | "flag" | "fflags" => cli_flags::run(rest),
                "audio-outputs" => {
                    for sink in audio_devices::sinks() {
                        println!("{}", audio_devices::row_label(&sink));
                    }
                    0
                }
                "plugins" | "plugin" => cli_plugins::run(rest),
                "doctor" => doctor::run(rest),
                "logs" | "log" => logs(rest),
                "desktop" => desktop::run(rest),
                "completions" => completions::run(rest),
                "__complete" => completions::dynamic(rest),
                // A bare link is the desktop entry's `%u`: play, joining it.
                link if looks_like_link(link) => play(&args),
                other => {
                    match completions::suggest(other, completions::COMMANDS.iter().map(|(n, _, _)| *n)) {
                        Some(near) => eprintln!("stacked: unknown command {other:?}. Did you mean `stacked {near}`?"),
                        None => eprintln!("stacked: unknown command {other:?}. Try `stacked help`."),
                    }
                    2
                }
            }
        }
    };
    ExitCode::from(code)
}

/// A value that should be treated as a Roblox link rather than a command.
///
/// Only a prefix check: [`deep_link::accept`] does the real validation, and
/// the point here is only to route a link to `play` rather than report it as
/// an unknown command.
fn looks_like_link(arg: &str) -> bool {
    // `get` rather than indexing: an argument is arbitrary text, and slicing
    // one at a byte offset inside a multi-byte character panics.
    deep_link::SCHEMES.iter().any(|s| {
        arg.get(..s.len()).is_some_and(|head| head.eq_ignore_ascii_case(s))
            && arg.get(s.len()..).is_some_and(|rest| rest.starts_with(':'))
    })
}

/// Pull `--profile NAME` out of an argument list, returning the rest.
pub(crate) fn take_profile(args: &[String]) -> Result<(Option<String>, Vec<String>), String> {
    let mut profile = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "--profile" || arg == "-p" {
            let name = it.next().ok_or("--profile needs a name")?;
            profile = Some(name.clone());
        } else if let Some(name) = arg.strip_prefix("--profile=") {
            profile = Some(name.to_string());
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((profile, rest))
}

/// The profile a command acts on: the one named, or the current one, refusing
/// a named profile that does not exist.
///
/// For the commands that write into a profile -- `pin`, `flags`, `plugins`.
/// Without it a mistyped `--profile` made a new directory, wrote the change
/// there and reported success, and the profile the user meant was untouched.
/// The current profile is accepted even before its directory exists, because
/// on a fresh install that is `default` and nothing has created it yet.
pub(crate) fn existing_profile(named: Option<String>) -> Result<String, String> {
    let current = shell_config::load(&shell_config::path()).profile;
    match named {
        Some(name) if name != current && !profile::list().contains(&name) => Err(format!(
            "there is no profile {name:?}. `stacked profiles` lists them, and \
             `stacked profiles new {name}` makes one."
        )),
        Some(name) => Ok(name),
        None => Ok(current),
    }
}

fn play(args: &[String]) -> u8 {
    let (named, rest) = match take_profile(args) {
        Ok(v) => v,
        Err(e) => {
            report(&e.to_string());
            return 2;
        }
    };
    let mut run_seconds = None;
    let mut link = None;
    let mut no_update = false;
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--no-update" | "--offline" => no_update = true,
            "--run" => match it.next().and_then(|s| s.parse::<u64>().ok()) {
                Some(n) => run_seconds = Some(n),
                None => {
                    eprintln!("stacked: --run needs a number of seconds");
                    return 2;
                }
            },
            // Before the link arm, or a mistyped option is taken for a link
            // and refused as one, which names the wrong mistake.
            other if other.starts_with("--") => {
                eprintln!("stacked: unknown option {other:?}. Try `stacked help`.");
                return 2;
            }
            other if link.is_none() => link = Some(other.to_string()),
            other => {
                eprintln!("stacked: unexpected argument {other:?}");
                return 2;
            }
        }
    }

    let config = shell_config::load(&shell_config::path());
    let explicit_profile = named.is_some();
    let mut profile_name = named.unwrap_or_else(|| config.profile.clone());

    // Nothing about a link is trusted: it was produced by a browser acting on
    // somebody's click. Refused links are reported rather than dropped,
    // because somebody whose browser opens Stacked and sees nothing happen has
    // no other way to find out why.
    let mut join_url = None;
    let mut matched = None;
    if let Some(raw) = link {
        let url = match deep_link::accept(&raw) {
            Ok(url) => url,
            Err(why) => {
                report(&format!("ignoring the link: {why}"));
                return 2;
            }
        };
        let routing = std::env::var("CORDIAL_BROWSER_ACCOUNT_ROUTING").as_deref() != Ok("0");
        match routing.then(|| browser_account::LaunchTicket::parse(&url)).flatten() {
            // ADR-035: the browser's one-use ticket says which account pressed
            // Play. If a saved profile is signed in as that account, it is the
            // one to launch -- unless a profile was named on the command line,
            // which is the more specific instruction. The ticket itself is
            // stripped from what the engine is handed either way.
            Some(ticket) => {
                join_url = Some(ticket.join_url.clone());
                if !explicit_profile {
                    println!("stacked: checking which account the browser is signed in as");
                    if let Some(found) = browser_account::resolve(ticket) {
                        println!("stacked: the browser's account is saved as profile {:?}", found.name());
                        profile_name = found.name().to_string();
                        matched = Some(found);
                    } else {
                        println!("stacked: no saved profile matches the browser's account; using {profile_name:?}");
                    }
                }
            }
            None => join_url = Some(url),
        }
    }

    if root_warning::running_as_root() {
        eprintln!("stacked: {}", root_warning::WARNING);
    }

    // ADR-044: the build is fetched when there is none, and brought up to date
    // when there is a newer one, unless the user has said not to. Neither can
    // stop a launch that has a build to start; see `auto_update.rs`.
    let may_fetch = config.auto_update && !no_update;
    let mut just_installed = false;
    let mut build = match install::locate(&config.roblox) {
        Ok(build) => build,
        Err(install::NotFound::NoBuild) if may_fetch => {
            if let Err(e) = auto_update::first_install() {
                if e == auto_update::METERED {
                    report(&e);
                } else {
                    report(&format!(
                        "there is no Roblox build yet, and downloading one failed: {e}\n\
                         `stacked install` tries again."
                    ));
                }
                return 1;
            }
            just_installed = true;
            match install::locate(&config.roblox) {
                Ok(build) => build,
                Err(install::NotFound::NoBuild) => {
                    report("Roblox was installed and then could not be found. `stacked status` shows where Stacked looked.");
                    return 1;
                }
                Err(install::NotFound::Unusable(message)) => {
                    report(&message);
                    return 1;
                }
            }
        }
        Err(install::NotFound::NoBuild) => {
            report(
                "no Roblox build found. `stacked install` downloads one, or uses the copy \
                 Sober downloaded if Sober is installed.",
            );
            return 1;
        }
        Err(install::NotFound::Unusable(message)) => {
            report(&message);
            return 1;
        }
    };
    if may_fetch && !just_installed {
        let pinned = profile::dir(&profile_name).ok().and_then(|d| profile::pinned_version(&d)).is_some();
        if let Some(origin) = auto_update::origin_of(&config) {
            if auto_update::before_launch(&build, origin, pinned) {
                build = match install::locate(&config.roblox) {
                    Ok(build) => build,
                    Err(install::NotFound::NoBuild) => {
                        report("Roblox was updated and then could not be found. `stacked status` shows where Stacked looked.");
                        return 1;
                    }
                    Err(install::NotFound::Unusable(message)) => {
                        report(&message);
                        return 1;
                    }
                };
            }
        }
    }
    // A profile that names a Roblox version gets that one, whatever the
    // current build is -- and a pin that cannot be honoured refuses the launch
    // rather than quietly running the build it was pinned away from.
    let build = match profile::dir(&profile_name)
        .map_err(install::NotFound::Unusable)
        .and_then(|d| install::apply_pin(build, &d))
    {
        Ok(build) => build,
        Err(install::NotFound::NoBuild) => {
            report(&format!(
                "profile {profile_name:?} is pinned to a build that is not on disk. \
                 `stacked versions get VERSION` fetches it, or `stacked unpin` clears the pin."
            ));
            return 1;
        }
        Err(install::NotFound::Unusable(message)) => {
            report(&message);
            return 1;
        }
    };

    // `play --profile NEW` makes the profile, as it always has. Noted, so the
    // plugins a new profile should not start are switched off in it below.
    let is_new_profile = !profile::list().contains(&profile_name);

    // ADR-012's claim, taken before the process exists so that a refusal
    // costs nothing, and naming the profile because "already open" on its own
    // does not tell anyone which one to close.
    let claim = match profile::acquire(&profile_name) {
        Ok(claim) => claim,
        Err(e @ profile::Error::Busy(..)) => {
            report(&format!(
                "{e}\nTo run a second client alongside it, give it a profile of its own: \
                 `stacked profiles new NAME`, then `stacked play --profile NAME`."
            ));
            return 3;
        }
        Err(e) => {
            report(&e.to_string());
            return 1;
        }
    };

    if is_new_profile {
        println!("stacked: profile {profile_name:?} is new; it starts signed out");
        cli_plugins::settle_new_profile(claim.profile_dir());
    }

    // Account routing authenticated exact identity and cookie bytes before
    // the lock was taken. If the saved values changed in between, the name no
    // longer identifies the session that was authenticated, so the launch goes
    // ahead without pinning the secret store to it.
    let secret_store = matched.and_then(|m| {
        m.still_matches(&profile_name, claim.profile_dir()).then(|| m.store())
    });

    match &join_url {
        Some(url) => println!(
            "stacked: starting Roblox on profile {profile_name:?}, joining {}",
            deep_link::summarise(url)
        ),
        None => println!("stacked: starting Roblox on profile {profile_name:?}"),
    }
    let mut instance = match launch::spawn(
        &build,
        claim,
        launch::LaunchRequest { run_seconds, join_url: join_url.as_deref(), secret_store },
    ) {
        Ok(instance) => instance,
        Err(message) => {
            report(&message);
            return 1;
        }
    };

    println!("stacked: cordial-run is pid {}", instance.pid());
    // Ctrl-C reaches the whole foreground process group, so the client hears
    // it and shuts down in order on its own. This process must not die first:
    // it holds the read end of the client's output, and a client that loses
    // its stdout mid-shutdown fails there instead of exiting cleanly. Set only
    // now, after the spawn, because an ignored signal survives `exec` and a
    // client started with SIGINT ignored would never hear Ctrl-C at all.
    //
    // SAFETY: `signal` with `SIG_IGN` installs no handler code; it only
    // changes this process's disposition.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
    }
    let status = match instance.wait() {
        Ok(status) => status,
        Err(e) => {
            report(&format!("lost track of the client: {e}"));
            return 1;
        }
    };
    if crash::is_crash(&status, &instance.recent_output()) {
        eprintln!();
        report(&format!(
            "{} Its output is above, or in the journal if it was started from the desktop. \
             `stacked logs` shows Roblox's own log, and `stacked diagnostics` prints what a \
             bug report needs alongside it.",
            crash::describe(&status),
        ));
        eprintln!("It was started with:\n{}", instance.command_line);
        return 1;
    }
    0
}

/// A launch that could not happen, said where the person who asked for it
/// will see it.
///
/// On a terminal that is stderr. Launched from the desktop entry or a browser's
/// Play button there is no terminal (`Terminal=false`), and a message on a
/// stderr nobody reads is a Play button that silently does nothing. So when
/// stderr is not a terminal the same sentence also goes out as a desktop
/// notification, through the portal `cordial_plugins::notify` already uses,
/// which reaches the desktop from inside the Flatpak too.
fn report(message: &str) {
    let message = message.trim();
    eprintln!("stacked: {message}");
    // SAFETY: `isatty` reads nothing but its argument.
    if unsafe { libc::isatty(2) } != 1 {
        if let Err(e) = cordial_plugins::notify::send("Stacked could not start Roblox", message) {
            eprintln!("stacked: the desktop notification failed too: {e}");
        }
    }
}

fn status(args: &[String]) -> u8 {
    let (named, _) = match take_profile(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("stacked: {e}");
            return 2;
        }
    };
    let config = shell_config::load(&shell_config::path());
    let name = named.unwrap_or_else(|| config.profile.clone());
    println!("profile:  {name}");
    if let Ok(dir) = profile::dir(&name) {
        println!("          {}", dir.display());
        if let Some(pin) = profile::pinned_version(&dir) {
            println!("pinned:   {pin}");
        }
    }
    match install::effective_apk(&config.roblox) {
        Some((apk, origin)) => {
            // The recorded version only: reading it out of the engine is a
            // 118 MB scan when nothing has memoised it, and a status line is
            // not worth that. Unknown here means "not yet extracted". Only
            // when the cache was extracted from this APK: otherwise the
            // version is some other build's.
            let cache = install::engine_cache();
            let version = cordial_update::cache::is_current(&cache, &apk)
                .then(|| cordial_update::cache::recorded_version(&cache))
                .flatten();
            match version {
                Some(version) => println!("roblox:   {version}, {}", apk.display()),
                None => println!("roblox:   {}", apk.display()),
            }
            println!("          {}", origin.describe());
        }
        None => println!("roblox:   none found -- `stacked` downloads one on first play"),
    }
    let origin = auto_update::origin_of(&config);
    let updates = if config.roblox.lib_dir.is_some() {
        "manual -- roblox.lib_dir names the engine, so Stacked will not replace the build".to_string()
    } else if !config.auto_update {
        "manual -- `stacked update`; `stacked config set auto_update true` checks at every play".to_string()
    } else if auto_update::manages(origin) {
        "checked when you press Play".to_string()
    } else {
        origin.and_then(install::Origin::why_not_updatable).unwrap_or_default().to_string()
    };
    println!("updates:  {updates}");
    if profile::is_held(&name) {
        println!("running:  yes");
    }
    0
}

fn profiles(args: &[String]) -> u8 {
    let config_path = shell_config::path();
    let mut config = shell_config::load(&config_path);
    match args.first().map(String::as_str) {
        None | Some("list") => {
            let mut names = profile::list();
            if !names.contains(&config.profile) {
                names.push(config.profile.clone());
                names.sort();
            }
            for name in names {
                let current = if name == config.profile { "*" } else { " " };
                let running = if profile::is_held(&name) { "  (running)" } else { "" };
                println!("{current} {name}{running}");
            }
            0
        }
        Some("new" | "create" | "add") => {
            let Some(name) = args.get(1) else {
                eprintln!("stacked: profiles new needs a name");
                return 2;
            };
            if !profile::is_valid_name(name) {
                eprintln!("stacked: {name:?} is not a usable profile name: letters, digits, - and _, up to 64");
                return 2;
            }
            if profile::list().iter().any(|n| n == name) {
                eprintln!("stacked: profile {name:?} already exists");
                return 1;
            }
            // Creating a profile is taking its lock once and letting it go,
            // which is also what makes the directory: the same thing the GTK
            // profile switcher did.
            match profile::acquire(name) {
                Ok(claim) => {
                    cli_plugins::settle_new_profile(claim.profile_dir());
                    drop(claim);
                    println!("created profile {name:?}. `stacked profiles use {name}` makes it current.");
                    0
                }
                Err(e) => {
                    eprintln!("stacked: {e}");
                    1
                }
            }
        }
        Some("use" | "switch" | "select") => {
            let Some(name) = args.get(1) else {
                eprintln!("stacked: profiles use needs a name");
                return 2;
            };
            if !profile::list().iter().any(|n| n == name) {
                eprintln!("stacked: there is no profile {name:?}. `stacked profiles new {name}` creates it.");
                return 1;
            }
            config.profile = name.clone();
            if let Err(e) = shell_config::save(&config_path, &config) {
                eprintln!("stacked: could not save {}: {e}", config_path.display());
                return 1;
            }
            println!("current profile is now {name:?}");
            0
        }
        Some("remove" | "delete" | "rm") => {
            let Some(name) = args.get(1).filter(|a| !a.starts_with('-')) else {
                eprintln!("stacked: profiles remove needs a name");
                return 2;
            };
            let yes = args.iter().any(|a| a == "--yes" || a == "-y");
            remove_profile(name, &config.profile, yes)
        }
        Some(other) => {
            eprintln!("stacked: unknown profiles command {other:?}. Try `stacked help`.");
            2
        }
    }
}

/// `stacked logs [--profile NAME] [--path] [--lines N]`: the engine's own
/// newest log, which the bug template calls the most useful attachment and
/// which lives four directories inside the profile where nobody finds it.
///
/// `<profile>/data/files/appData/logs` is where the engine writes when the
/// launcher hands it `CORDIAL_FILES_DIR`; `launch.rs`'s end-to-end test is what
/// established it.
fn logs(args: &[String]) -> u8 {
    let (named, rest) = match take_profile(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("stacked: {e}");
            return 2;
        }
    };
    let mut path_only = false;
    let mut lines = 50usize;
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--path" => path_only = true,
            "--lines" | "-n" => match it.next().and_then(|n| n.parse().ok()) {
                Some(n) => lines = n,
                None => {
                    eprintln!("stacked: --lines needs a number");
                    return 2;
                }
            },
            other => {
                eprintln!("stacked: unexpected argument {other:?}; logs takes --profile, --path and --lines");
                return 2;
            }
        }
    }
    let name = match existing_profile(named) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("stacked: {e}");
            return 1;
        }
    };
    let dir = match profile::dir(&name) {
        Ok(d) => d.join("data/files/appData/logs"),
        Err(e) => {
            eprintln!("stacked: {e}");
            return 1;
        }
    };
    let Some(newest) = newest_log(&dir) else {
        eprintln!("stacked: no Roblox log yet in {}. The engine writes one once it has started.", dir.display());
        return 1;
    };
    if path_only {
        println!("{}", newest.display());
        return 0;
    }
    let text = match std::fs::read(&newest) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => {
            eprintln!("stacked: {}: {e}", newest.display());
            return 1;
        }
    };
    let all: Vec<&str> = text.lines().collect();
    for line in &all[all.len().saturating_sub(lines)..] {
        println!("{line}");
    }
    eprintln!("\n({} lines of {}; --lines N for more)", all.len().min(lines), newest.display());
    0
}

/// The most recently modified `.log` in `dir`.
fn newest_log(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(when, _)| *when)
        .map(|(_, path)| path)
}

/// Delete a profile: its sign-in, settings, FastFlags, plugin grants and
/// Roblox's data for it.
///
/// **The saved sign-in is erased first, from the keyring as well as the
/// profile.** Keyring entries are keyed by the profile's directory
/// (`secrets::attributes`), so deleting the directory alone would leave a live
/// session token in the keyring with nothing that would ever read or remove
/// it -- and a later profile of the same name would inherit it.
///
/// Refused for the current profile, which the next `stacked` would recreate
/// empty, and for one that is running, by taking its lock the way a launch
/// does. The directory is renamed aside before it is deleted, so an
/// interrupted removal leaves no half-profile under the old name.
fn remove_profile(name: &str, current: &str, yes: bool) -> u8 {
    if !profile::list().iter().any(|n| n == name) {
        eprintln!("stacked: there is no profile {name:?}. `stacked profiles` lists them.");
        return 1;
    }
    if name == current {
        eprintln!(
            "stacked: {name:?} is the current profile. `stacked profiles use OTHER` first, then remove it."
        );
        return 1;
    }
    if !yes {
        // SAFETY: `isatty` reads nothing but its argument.
        if unsafe { libc::isatty(0) } != 1 {
            eprintln!("stacked: profiles remove deletes a sign-in and its data; pass --yes to do it without a terminal");
            return 2;
        }
        print!(
            "This deletes profile {name:?}: its sign-in, settings, FastFlags and Roblox data.\n\
             Type the profile's name to confirm: "
        );
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let mut typed = String::new();
        if std::io::stdin().read_line(&mut typed).is_err() || typed.trim() != name {
            println!("not removed");
            return 1;
        }
    }
    let claim = match profile::acquire(name) {
        Ok(claim) => claim,
        Err(e @ profile::Error::Busy(..)) => {
            eprintln!("stacked: {e}
Close it first.");
            return 3;
        }
        Err(e) => {
            eprintln!("stacked: {e}");
            return 1;
        }
    };
    let dir = claim.profile_dir().to_path_buf();
    let store = if cordial_shell::secrets::usable().is_ok() {
        cordial_shell::secrets::Store::Keyring
    } else {
        cordial_shell::secrets::Store::File
    };
    for kind in [cordial_shell::secrets::Kind::Cookies, cordial_shell::secrets::Kind::Identity] {
        if let Err(e) = cordial_shell::secrets::erase(store, &dir, kind) {
            eprintln!("stacked: could not erase the saved {}: {e}. Nothing was removed.", kind.name());
            return 1;
        }
    }
    let aside = dir.with_file_name(format!(".{name}.removing"));
    let _ = std::fs::remove_dir_all(&aside);
    if let Err(e) = std::fs::rename(&dir, &aside).and_then(|()| std::fs::remove_dir_all(&aside)) {
        eprintln!("stacked: {}: {e}", dir.display());
        return 1;
    }
    drop(claim);
    println!("removed profile {name:?}");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_log_is_found_by_time_and_other_files_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        assert!(newest_log(dir.path()).is_none());
        let old = dir.path().join("0.1_old_Player_1.log");
        let new = dir.path().join("0.1_new_Player_2.log");
        std::fs::write(&old, "old").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "not a log").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&new, "new").unwrap();
        assert_eq!(newest_log(dir.path()), Some(new));
    }

    #[test]
    fn a_bare_link_is_routed_to_play_and_a_command_is_not() {
        assert!(looks_like_link("roblox-player:1+launchmode:play"));
        assert!(looks_like_link("roblox://experiences/start?placeId=1"));
        assert!(looks_like_link("ROBLOX-PLAYER:1"));
        for not_a_link in ["play", "roblox", "roblox-player", "robloxian:1", "config", "roblo€x:1", "é"] {
            assert!(!looks_like_link(not_a_link), "{not_a_link}");
        }
    }

    #[test]
    fn the_profile_flag_is_taken_out_wherever_it_is() {
        let args: Vec<String> =
            ["--run", "5", "--profile", "alt", "roblox:x"].iter().map(|s| s.to_string()).collect();
        let (profile, rest) = take_profile(&args).unwrap();
        assert_eq!(profile.as_deref(), Some("alt"));
        assert_eq!(rest, ["--run", "5", "roblox:x"]);

        let (profile, _) = take_profile(&["--profile=main".to_string()]).unwrap();
        assert_eq!(profile.as_deref(), Some("main"));
        assert!(take_profile(&["--profile".to_string()]).is_err());
    }

    #[test]
    fn every_command_in_the_usage_text_is_one_main_dispatches() {
        // A usage line naming a command that falls through to "unknown
        // command" is the `--help` that lies; this keeps the two in step.
        for (command, _, _) in completions::COMMANDS {
            assert!(USAGE.contains(&format!("  {command}")), "{command} missing from usage");
        }
    }
}
