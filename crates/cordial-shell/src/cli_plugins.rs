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
    let profile_name = match crate::existing_profile(named) {
        Ok(name) => name,
        Err(e) => {
            eprintln!("stacked: {e}");
            return 1;
        }
    };
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
                // In every profile, not only the one named: enablement is per
                // profile and a profile with no opinion runs the plugin, so
                // switching it off here alone left it running everywhere else
                // the moment it was installed. `settle_new_profile` covers the
                // profiles created after this.
                let mut names = profile::list();
                if !names.contains(&profile_name) {
                    names.push(profile_name.clone());
                }
                for name in names {
                    let written = profile::dir(&name)
                        .map_err(|e| e.to_string())
                        .and_then(|dir| enablement::set_enabled(&dir, &id, false).map_err(|e| e.to_string()));
                    if let Err(e) = written {
                        eprintln!("stacked: could not leave {id} switched off in profile {name:?}: {e}");
                        return 1;
                    }
                }
                println!(
                    "it contains code, so it is off in every profile. \
                     `stacked plugins enable {id}` switches it on in this one."
                );
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
        Some("prefs" | "preferences") => {
            let Some(id) = arg(1) else {
                eprintln!("stacked: plugins prefs needs an ID; `stacked plugins` lists them");
                return 2;
            };
            prefs(&profile_dir, id, &rest[2..])
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

/// `stacked plugins prefs ID [KEY VALUE | --reset]`: the preferences a plugin
/// declares, which the Settings page used to draw as a form (ADR-020).
///
/// The validation is `preferences::Store::set`'s, the same check the page
/// made, so a value refused here is one the plugin would never have been
/// handed. What this adds is reading a typed word into the field's type:
/// `on`, `yes` and `true` are all a boolean, and a choice is named by its
/// value.
fn prefs(profile_dir: &std::path::Path, id: &str, rest: &[String]) -> u8 {
    use cordial_plugins::preferences::{Field, Store};
    let Some(plugin) = installed().into_iter().find(|p| p.manifest.id == id) else {
        eprintln!("stacked: no plugin {id:?} is installed");
        return 1;
    };
    let fields = &plugin.manifest.preferences;
    if fields.is_empty() {
        println!("{id} has no preferences.");
        return 0;
    }
    let store = Store::new(profile_dir);
    match rest {
        [] => {
            let values = match store.effective_for(id, fields) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("stacked: {e}");
                    return 1;
                }
            };
            for field in fields {
                let value = values.get(&field.key).cloned().unwrap_or_else(|| field.field.default_value());
                let accepts = match &field.field {
                    Field::Bool { .. } => "true | false".to_string(),
                    Field::Int { minimum, maximum, .. } => match (minimum, maximum) {
                        (Some(lo), Some(hi)) => format!("{lo}-{hi}"),
                        (Some(lo), None) => format!("{lo} or more"),
                        (None, Some(hi)) => format!("up to {hi}"),
                        (None, None) => "a whole number".into(),
                    },
                    Field::Choice { options, .. } => {
                        options.iter().map(|o| o.value.as_str()).collect::<Vec<_>>().join(" | ")
                    }
                    Field::Text { .. } => "text".into(),
                };
                println!("{} = {}
    {accepts}
    {}", field.key, shown(&value), field.title);
                if !field.description.is_empty() {
                    println!("    {}", field.description);
                }
            }
            0
        }
        [flag] if flag == "--reset" => match store.reset(id) {
            Ok(()) => {
                println!("{id}'s preferences are back to their defaults");
                0
            }
            Err(e) => {
                eprintln!("stacked: {e}");
                1
            }
        },
        [key, words @ ..] if !words.is_empty() => {
            let Some(field) = fields.iter().find(|f| &f.key == key) else {
                let keys: Vec<&str> = fields.iter().map(|f| f.key.as_str()).collect();
                match crate::completions::suggest(key, keys.iter().copied()) {
                    Some(near) => eprintln!("stacked: {id} has no preference {key:?}. Did you mean {near}?"),
                    None => eprintln!("stacked: {id} has no preference {key:?}; it has {}", keys.join(", ")),
                }
                return 2;
            };
            let raw = words.join(" ");
            let value = typed(&field.field, &raw);
            match store.set(id, fields, key, value.clone()) {
                Ok(()) => {
                    println!("{key} = {}", shown(&value));
                    0
                }
                Err(e) => {
                    eprintln!("stacked: {e}");
                    2
                }
            }
        }
        _ => {
            eprintln!("stacked: plugins prefs ID lists them; plugins prefs ID KEY VALUE sets one; --reset clears them");
            2
        }
    }
}

/// A typed word, read as the kind of value `field` holds. Anything that does
/// not read as that kind is passed through as text, so the store's own check
/// refuses it with the field's type in the message.
fn typed(field: &cordial_plugins::preferences::Field, raw: &str) -> serde_json::Value {
    use cordial_plugins::preferences::Field;
    let word = raw.trim();
    match field {
        Field::Bool { .. } => match word.to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => serde_json::Value::Bool(true),
            "false" | "off" | "no" | "0" => serde_json::Value::Bool(false),
            _ => serde_json::Value::String(raw.to_string()),
        },
        Field::Int { .. } => word
            .parse::<i64>()
            .map(serde_json::Value::from)
            .unwrap_or_else(|_| serde_json::Value::String(raw.to_string())),
        Field::Choice { .. } | Field::Text { .. } => serde_json::Value::String(raw.to_string()),
    }
}

fn shown(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) if s.is_empty() => "\"\"".into(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Switch off, in a profile that has just been created, every plugin with code
/// the user has installed.
///
/// A profile with no opinion about a plugin runs it (`enablement::is_enabled`),
/// which is right for one nobody has installed code into yet and wrong for a
/// new profile: it would start every installed plugin's process the first time
/// it launched, without the per-profile `enable` that installing asks for.
/// First-party plugins are left to `enablement::default_for`.
pub(crate) fn settle_new_profile(profile_dir: &std::path::Path) {
    for plugin in manifest::discover(&manifest::plugin_root()) {
        if consent::starts_disabled(&plugin) {
            if let Err(e) = enablement::set_enabled(profile_dir, &plugin.manifest.id, false) {
                eprintln!("stacked: could not leave {} off in the new profile: {e}", plugin.manifest.id);
            }
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
