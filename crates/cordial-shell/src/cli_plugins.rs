//! `stacked plugins`: install, list, switch and grant plugins without the
//! Settings page that used to do it.
//!
//! The rules are the Settings page's, because they are `cordial_plugins`'s
//! rather than the page's. A newly installed plugin that carries code starts
//! **switched off** (`consent::starts_disabled`) and holds **no capabilities**:
//! installing is not consenting, and each grant is a separate command naming
//! the capability and printing what it lets the plugin do. The GTK page asked
//! that as one dialog; a terminal asks it as two commands, which is the same
//! consent spelled out.
//!
//! Enablement and grants are per profile (ADR-013), so both take `--profile`
//! and default to the current one. A change reaches an already-running client
//! within a second or two (ADR-038) -- it watches the same files.

use cordial_plugins::capability::Capability;
use cordial_plugins::{consent, enablement, grants, manifest, sandbox, unpack};
use cordial_shell::profile;

pub fn run(args: &[String]) -> u8 {
    let (named, rest) = match crate::take_profile(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("stacked: {e}");
            return 2;
        }
    };
    let profile_name = crate::chosen_profile(named);
    let profile_dir = match profile::dir(&profile_name) {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("stacked: {e}");
            return 1;
        }
    };
    let arg = |i: usize| rest.get(i).map(String::as_str);
    match arg(0) {
        None | Some("list") => {
            let plugins = installed();
            if plugins.is_empty() {
                println!("no plugins installed. `stacked plugins install FILE.tar.zst` adds one.");
                return 0;
            }
            let granted = grants::load(&grants::path_in(&profile_dir));
            println!("profile {profile_name:?}:");
            for p in plugins {
                let id = &p.manifest.id;
                let on = if enablement::is_enabled(&profile_dir, id) { "on " } else { "off" };
                let version = p.version.as_ref().map(|v| format!(" {v}")).unwrap_or_default();
                let name = if p.manifest.name.is_empty() { id.clone() } else { p.manifest.name.clone() };
                println!("  [{on}] {id}{version}  {name}");
                let have = granted.get(id).cloned().unwrap_or_default();
                for cap in &p.requested {
                    let mark = if have.contains(cap) { "granted" } else { "not granted" };
                    println!("        {} ({mark})", cap.name());
                }
            }
            0
        }
        Some("install" | "add") => {
            let Some(file) = arg(1) else {
                eprintln!("stacked: plugins install needs a .tar.zst archive");
                return 2;
            };
            let bytes = match std::fs::read(file) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("stacked: {file}: {e}");
                    return 1;
                }
            };
            let (plugin, dir) = match unpack::install_local(&bytes, &manifest::plugin_root()) {
                Ok(v) => v,
                Err(refusal) => {
                    eprintln!("stacked: {file} was not installed: {refusal}");
                    return 1;
                }
            };
            let id = plugin.manifest.id.clone();
            println!("installed {id} into {}", dir.display());
            if plugin.has_code() && !sandbox::interpreter_present() {
                println!("it needs Deno to run, and there is none. `stacked plugins deno` installs it.");
            }
            if consent::starts_disabled(&plugin) {
                if let Err(e) = enablement::set_enabled(&profile_dir, &id, false) {
                    eprintln!("stacked: could not leave {id} switched off: {e}");
                    return 1;
                }
                println!("it contains code, so it is off. `stacked plugins enable {id}` switches it on.");
            }
            if !plugin.requested.is_empty() {
                println!("it asks for these, none granted yet:");
                for cap in &plugin.requested {
                    println!("  {}: {}", cap.name(), cap.consequence());
                    println!("    stacked plugins grant {id} {}", cap.name());
                }
            }
            0
        }
        Some("remove" | "uninstall" | "rm") => {
            let Some(id) = arg(1) else {
                eprintln!("stacked: plugins remove needs an ID");
                return 2;
            };
            match unpack::uninstall(&manifest::plugin_root(), id) {
                Ok(()) => {
                    println!("removed {id}");
                    0
                }
                Err(e) => {
                    eprintln!("stacked: {e}");
                    1
                }
            }
        }
        Some(verb @ ("enable" | "disable")) => {
            let Some(id) = arg(1) else {
                eprintln!("stacked: plugins {verb} needs an ID");
                return 2;
            };
            let on = verb == "enable";
            match enablement::set_enabled(&profile_dir, id, on) {
                Ok(()) => {
                    println!("{id} is {} on profile {profile_name:?}", if on { "on" } else { "off" });
                    0
                }
                Err(e) => {
                    eprintln!("stacked: {e}");
                    1
                }
            }
        }
        Some(verb @ ("grant" | "revoke")) => {
            let (Some(id), Some(cap_name)) = (arg(1), arg(2)) else {
                eprintln!("stacked: plugins {verb} needs an ID and a CAPABILITY");
                return 2;
            };
            let Some(cap) = Capability::parse(cap_name) else {
                let names: Vec<&str> = Capability::all().iter().map(|c| c.name()).collect();
                eprintln!("stacked: {cap_name:?} is not a capability. Known: {}", names.join(", "));
                return 2;
            };
            let on = verb == "grant";
            // Only what the plugin asked for, which is all the Settings page
            // ever offered: a grant the manifest does not request is consent to
            // something nobody was shown. Revoking is always allowed, so a
            // grant left over from an older version of a plugin can be
            // removed after that version stops asking for it.
            if on {
                match installed().into_iter().find(|p| p.manifest.id == id) {
                    None => {
                        eprintln!("stacked: no plugin {id:?} is installed");
                        return 1;
                    }
                    Some(p) if !p.requested.contains(&cap) => {
                        eprintln!("stacked: {id} does not ask for {}, so it cannot be granted", cap.name());
                        return 1;
                    }
                    Some(_) => {}
                }
            }
            match grants::set(&grants::path_in(&profile_dir), id, cap, on) {
                Ok(()) => {
                    if on {
                        println!("granted {} to {id}: {}", cap.name(), cap.consequence());
                    } else {
                        println!("revoked {} from {id}", cap.name());
                    }
                    0
                }
                Err(e) => {
                    eprintln!("stacked: {e}");
                    1
                }
            }
        }
        Some("deno") => {
            // What the Plugins page's download button did: plugins with code
            // run on Deno, and a machine without one on `PATH` gets a pinned,
            // hash-checked copy in Stacked's own data directory.
            if sandbox::interpreter_present() {
                println!("Deno is already available; plugins with code can run.");
                return 0;
            }
            let Some(dir) = sandbox::managed_deno_dir() else {
                eprintln!("stacked: neither XDG_DATA_HOME nor HOME is set, so there is nowhere to put Deno");
                return 1;
            };
            let mut progress = |done: u64, total: Option<u64>| {
                const MB: u64 = 1024 * 1024;
                match total {
                    Some(t) if t > 0 => eprint!("\rdownloading Deno: {} of {} MB", done / MB, t / MB),
                    _ => eprint!("\rdownloading Deno: {} MB", done / MB),
                }
            };
            let result = cordial_update::deno::install(&dir, &mut progress);
            eprintln!();
            match result {
                Ok(path) => {
                    println!("installed {}. Plugins start with the client, so this takes effect at the next launch.", path.display());
                    0
                }
                Err(why) => {
                    eprintln!("stacked: could not install Deno: {why}");
                    1
                }
            }
        }
        Some(other) => {
            eprintln!("stacked: unknown plugins command {other:?}. Try `stacked help`.");
            2
        }
    }
}

/// Every installed plugin: first-party, the user's, and unpacked ones being
/// developed. The same three roots the runtime reads.
fn installed() -> Vec<manifest::Plugin> {
    let mut plugins = manifest::discover(&manifest::system_plugin_root());
    plugins.extend(manifest::discover(&manifest::plugin_root()));
    plugins.extend(manifest::discover_unpacked());
    plugins
}
