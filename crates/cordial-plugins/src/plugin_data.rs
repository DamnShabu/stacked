//! What a plugin leaves behind in a profile when its files are removed.
//!
//! Uninstalling deletes the plugin: its manifest, its entry module, its assets,
//! and the flag layer it wrote, because all of those live inside its installed
//! directory and `unpack::uninstall` removes that whole. **None of that is
//! data.** The data is what the *profile* remembers about the plugin — the
//! settings somebody chose, the capabilities they allowed, and whether they had
//! switched it off — and that is deliberately kept, so reinstalling picks up
//! where it left off.
//!
//! Kept is the right default and occasionally the wrong answer, which is what
//! this module is for: it measures what is being kept so the Remove dialog can
//! offer to delete it and say how much, and it deletes the lot in one place so
//! no caller has to know that grants and enablement live in shared documents
//! while settings get a directory of their own.
//!
//! The three locations, and why they are not one:
//!
//! ```text
//! <profile>/plugins/<id>/         settings.json and anything else it saved
//! <profile>/plugin-grants.json    one entry, keyed by id
//! <profile>/plugin-enabled.json   one entry, and only when switched off
//! ```
//!
//! Only the first has a size worth quoting. The other two are single entries
//! inside documents shared by every plugin, so removing one frees a line rather
//! than a measurable quantity of disk -- which is why [`Footprint::bytes`] is
//! the directory alone and [`Footprint::is_empty`] is not `bytes == 0`. A
//! plugin that was granted a capability and never wrote a setting has data to
//! delete and occupies nothing, and offering no checkbox for it would be
//! wrong.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::capability::Capability;
use crate::{enablement, grants, manifest, settings};

/// What a profile is holding on behalf of one plugin.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Footprint {
    /// Bytes under `<profile>/plugins/<id>/`, or 0 if it does not exist.
    pub bytes: u64,
    /// Whether that directory exists at all.
    pub has_settings: bool,
    /// Whether the profile records any granted capability for this plugin.
    pub has_grants: bool,
    /// Whether the profile records an enablement opinion. Only a plugin that
    /// has been switched *off* has one, because `set_enabled` stores the
    /// exceptions and lets "absent" mean enabled.
    pub has_enablement: bool,
}

impl Footprint {
    /// Whether there is nothing to delete.
    ///
    /// **Not `bytes == 0`.** Grants are an entry in a shared file and weigh
    /// nothing measurable, and they are the single most important thing on this
    /// list to be able to clear: a reinstall that silently kept its old
    /// permissions would be consent the user gave once and never renewed.
    pub fn is_empty(&self) -> bool {
        !self.has_settings && !self.has_grants && !self.has_enablement
    }
}

/// Measure what `profile_dir` is keeping for `id`.
///
/// An unreadable directory counts as absent rather than propagating an error.
/// This exists to decide whether to draw a checkbox and what to write on it,
/// and a dialog that refuses to open because a size could not be totalled would
/// be worse than one that offers to delete something it under-measured.
pub fn footprint(profile_dir: &Path, id: &str) -> Footprint {
    if !manifest::is_valid_id(id) {
        return Footprint::default();
    }
    let dir = settings_dir(profile_dir, id);
    let has_settings = dir.is_dir();
    Footprint {
        bytes: if has_settings { dir_bytes(&dir) } else { 0 },
        has_settings,
        has_grants: grants::load(&grants::path_in(profile_dir))
            .get(id)
            .is_some_and(|c: &std::collections::BTreeSet<Capability>| !c.is_empty()),
        has_enablement: enablement::load(&enablement::path_in(profile_dir)).contains_key(id),
    }
}

/// Delete everything [`footprint`] counted.
///
/// Best-effort across all three rather than stopping at the first failure: a
/// user who asked to delete their settings and their permissions should not end
/// up with the settings gone and the permissions still granted because a
/// shared file was briefly unwritable. Every failure is collected and reported
/// together, so the caller can say what actually survived instead of implying
/// the whole thing worked.
pub fn forget(profile_dir: &Path, id: &str) -> Result<(), String> {
    if !manifest::is_valid_id(id) {
        return Err(format!("{id:?} is not a usable plugin id"));
    }
    let mut failures: Vec<String> = Vec::new();

    let dir = settings_dir(profile_dir, id);
    if dir.is_dir() {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            failures.push(format!("{}: {e}", dir.display()));
        }
    }

    let grants_path = grants::path_in(profile_dir);
    let mut all = grants::load(&grants_path);
    if all.remove(id).is_some() {
        if let Err(e) = save_grants(&grants_path, &all) {
            failures.push(format!("{}: {e}", grants_path.display()));
        }
    }

    // `set_enabled(.., true)` is precisely "remove the entry", because only the
    // exceptions are stored. Reusing it keeps the write shape -- write a
    // sibling, rename over -- in the one module that owns that file.
    if enablement::load(&enablement::path_in(profile_dir)).contains_key(id) {
        if let Err(e) = enablement::set_enabled(profile_dir, id, true) {
            failures.push(format!("{}: {e}", enablement::path_in(profile_dir).display()));
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// `grants` has no "drop this plugin entirely", because `set` works one
/// capability at a time and empties the entry as a side effect. Writing the
/// document directly here rather than looping `set` over every capability: the
/// loop would rewrite the file once per capability, and a process killed
/// half way through it would leave a plugin holding some of what it had.
fn save_grants(
    path: &Path,
    all: &BTreeMap<String, std::collections::BTreeSet<Capability>>,
) -> std::io::Result<()> {
    let as_names: BTreeMap<&str, Vec<&str>> = all
        .iter()
        .map(|(id, caps)| (id.as_str(), caps.iter().map(|c| c.name()).collect()))
        .collect();
    let text = serde_json::to_string_pretty(&as_names)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.new");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

fn settings_dir(profile_dir: &Path, id: &str) -> PathBuf {
    // Derived from the settings store rather than rebuilt, so a change to where
    // settings live cannot leave this measuring and deleting the wrong path.
    settings::Store::new(profile_dir)
        .path_for(id)
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| profile_dir.join("plugins").join(id))
}

/// Total size of the files under `dir`.
///
/// Symlinks are counted by their own size and never followed, so a link into
/// somewhere enormous cannot make a plugin's settings appear to be gigabytes.
/// Plugin archives may not contain symlinks at all (`unpack`), but this walks a
/// directory in the user's own profile, which nothing stops them putting one in.
fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

/// A size for a sentence, not a table.
///
/// Powers of ten and one decimal place, because this appears mid-label next to
/// a checkbox and "12.4 kB" is read at a glance where "12,678 bytes" is not.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [(&str, u64); 3] = [("GB", 1_000_000_000), ("MB", 1_000_000), ("kB", 1_000)];
    for (unit, scale) in UNITS {
        if bytes >= scale {
            return format!("{:.1} {unit}", bytes as f64 / scale as f64);
        }
    }
    format!("{bytes} bytes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cordial-plugin-data-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_profile_holding_nothing_has_nothing_to_delete() {
        let dir = scratch("empty");
        let f = footprint(&dir, "fps-flex");
        assert!(f.is_empty(), "{f:?}");
        assert_eq!(f.bytes, 0);
    }

    #[test]
    fn settings_are_counted_and_deleted() {
        let dir = scratch("settings");
        let store = settings::Store::new(&dir);
        store.write("fps-flex", &serde_json::json!({"target_fps": 240})).unwrap();

        let f = footprint(&dir, "fps-flex");
        assert!(f.has_settings);
        assert!(f.bytes > 0, "a written document has a size");
        assert!(!f.is_empty());

        forget(&dir, "fps-flex").unwrap();
        assert!(footprint(&dir, "fps-flex").is_empty(), "everything is gone");
    }

    #[test]
    fn a_grant_alone_is_data_even_though_it_weighs_nothing() {
        // The case `bytes == 0` would get wrong, and the most important one to
        // be able to clear: a reinstall must not silently keep permissions the
        // user allowed once.
        let dir = scratch("grant-only");
        grants::set(&grants::path_in(&dir), "fps-flex", Capability::Log, true).unwrap();

        let f = footprint(&dir, "fps-flex");
        assert_eq!(f.bytes, 0, "a grant occupies no measurable disk");
        assert!(f.has_grants);
        assert!(!f.is_empty(), "and is still data worth offering to delete");

        forget(&dir, "fps-flex").unwrap();
        assert!(
            !grants::load(&grants::path_in(&dir)).contains_key("fps-flex"),
            "the grant is withdrawn"
        );
    }

    #[test]
    fn another_plugins_data_is_left_alone() {
        // `forget` rewrites documents shared by every plugin, so this is the
        // property that keeps it from being a footgun.
        let dir = scratch("neighbours");
        let store = settings::Store::new(&dir);
        store.write("fps-flex", &serde_json::json!({"a": 1})).unwrap();
        store.write("flat-textures", &serde_json::json!({"b": 2})).unwrap();
        grants::set(&grants::path_in(&dir), "fps-flex", Capability::Log, true).unwrap();
        grants::set(&grants::path_in(&dir), "flat-textures", Capability::Log, true).unwrap();
        enablement::set_enabled(&dir, "flat-textures", false).unwrap();

        forget(&dir, "fps-flex").unwrap();

        let other = footprint(&dir, "flat-textures");
        assert!(other.has_settings, "its settings survive");
        assert!(other.has_grants, "its grant survives");
        assert!(other.has_enablement, "its off switch survives");
        assert_eq!(
            store.read("flat-textures").unwrap(),
            serde_json::json!({"b": 2}),
            "and the document itself is unchanged"
        );
    }

    /// **Uses a user-installed id, not a built-in one, and that matters.**
    /// This read `fps-flex` when the name was an arbitrary fixture, and broke
    /// the day `fps-flex` became a real plugin that ships disabled: switching
    /// off something already off writes no entry, because `set_enabled` stores
    /// only exceptions to a plugin's default. The property under test here is
    /// about a plugin whose default is on, so it needs a name whose default is
    /// on -- and a fixture that silently starts meaning something is worth a
    /// sentence rather than a rename.
    #[test]
    fn being_switched_off_is_data_and_forgetting_restores_the_default() {
        let dir = scratch("disabled");
        enablement::set_enabled(&dir, "a-plugin-the-user-installed", false).unwrap();
        assert!(footprint(&dir, "a-plugin-the-user-installed").has_enablement);

        forget(&dir, "a-plugin-the-user-installed").unwrap();
        assert!(footprint(&dir, "a-plugin-the-user-installed").is_empty());
        assert!(
            enablement::is_enabled(&dir, "a-plugin-the-user-installed"),
            "absent means enabled, so a reinstall comes back on"
        );
    }

    #[test]
    fn an_unusable_id_measures_nothing_and_deletes_nothing() {
        let dir = scratch("bad-id");
        assert!(footprint(&dir, "../etc").is_empty());
        assert!(forget(&dir, "../etc").is_err());
    }

    #[test]
    fn sizes_read_as_a_sentence() {
        assert_eq!(human_bytes(0), "0 bytes");
        assert_eq!(human_bytes(847), "847 bytes");
        assert_eq!(human_bytes(12_400), "12.4 kB");
        assert_eq!(human_bytes(4_200_000), "4.2 MB");
    }
}
