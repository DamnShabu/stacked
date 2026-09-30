//! Deciding what changed in a profile's plugin set, so a running client can
//! start, stop or restart exactly the plugins that moved rather than the
//! whole set -- and never anything more than the diff says.
//!
//! **What this does not do.** It does not touch a process, a broker or a
//! thread. Everything below is pure: read the disk, compare two snapshots,
//! say what changed. `cordial-runtime`'s `plugin_host` is what acts on the
//! answer -- spawning through the same `spawn_one` a fresh launch uses,
//! killing through `host::kill_process_group`, and giving a stopped plugin's
//! serving thread the chance to run its own ordinary teardown rather than
//! being torn down from outside it. Keeping the decision here and the effect
//! there is what makes [`diff`] testable without a Deno interpreter, a
//! `bwrap`, or a running client at all.
//!
//! ## Why this reimplements `start_all`'s discovery rather than calling it
//!
//! `plugin_host::start_all` (in `cordial-runtime`) answers "what should run,
//! printed for a human watching a launch": built-in counts, shadow warnings,
//! "not granted" lines, one `println!` per decision. [`desired_state`] answers
//! the same question silently, because it runs once a second for the whole
//! life of the client and a launch-shaped log line repeated every tick would
//! drown the one thing worth reading in that log, which is a *change*.
//! `cordial-shell`'s `refresh_watch.rs` sets the precedent for keeping a
//! second, identically-shaped copy of a decision when the two callers
//! genuinely need different things from it (there: a Cargo dependency cycle;
//! here: silence) rather than forcing one shape to serve both. The resolution
//! rule itself -- system root first, then the user root skipping ids a
//! built-in already claims, unpacked plugins excluded -- is the same rule in
//! both places, and `plugin_host::start_all`'s own tests are what would catch
//! the two drifting on what a *fresh launch* does; [`tests`] below are what
//! would catch this file drifting on what a *hot swap* does.
//!
//! ## Why unpacked plugins are invisible here
//!
//! A `CORDIAL_UNPACKED_PLUGINS` entry already reloads on its own -- Deno's own
//! `--watch`, started by `sandbox::command` when `PluginProc::spawn_with` is
//! given `reload: true` -- and it does that by restarting the same process
//! Deno is already supervising, not by anything in this file. Two supervisors
//! deciding to restart the same plugin is not a sharper edge than one, it is
//! a race between them, so [`desired_state`] never returns an unpacked
//! plugin and the reconciler's own "what is running" snapshot must not
//! include one either -- see `cordial-runtime::plugin_host::start_reconciler`.
//!
//! ## Why a plugin with no capability is simply absent
//!
//! `start_all` never spawns a plugin whose grant set is empty, and says why
//! at launch: "no capabilities granted, not started". [`desired_state`]
//! applies the same test continuously rather than only once, so a plugin
//! whose last capability is revoked while it is already running is not a
//! distinct "stop because empty" case to handle -- it simply stops appearing
//! in the desired set, and [`diff`] reports that the same way it reports an
//! uninstall: the plugin the caller asked to keep running is no longer one
//! this profile is willing to run at all. A plugin that keeps at least one
//! capability through a change is never this file's business; the process
//! that is already running it discovers the new set on its own next request,
//! through `plugin_host::refresh_grant`'s existing `mtime` check, which this
//! module's [`Change::Regrant`] only exists to make *visible* in the log,
//! not to *cause*.

use crate::capability::Capability;
use crate::manifest::{self, Plugin};
use crate::{enablement, grants};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// "Are these the same bytes", for one plugin's own files.
///
/// `sha2` rather than a `std::hash::Hasher` is already a direct dependency of
/// this crate for `ADR-037`'s content-addressed build store, so this reaches
/// for the same tool rather than a second, weaker one: `DefaultHasher` only
/// promises agreement within one process, which happens to be all this
/// needed, but "happens to be enough" is how a codebase ends up with two
/// hashing strategies for the same kind of question. A collision would show
/// a real update as unchanged; nothing here is a security boundary that
/// depends on one not happening, which is why this stays plain `Sha256`
/// rather than something collision-resistant against an adversary -- the
/// adversary model for a plugin's own files is ADR-003's process boundary,
/// not this comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    fn of(manifest_text: &[u8], entry: Option<&[u8]>) -> Fingerprint {
        let mut h = Sha256::new();
        h.update(manifest_text);
        // A length-prefix byte tells "no entry" apart from "an empty entry
        // module" -- both are real states (a data-only plugin never reaches
        // this function at all, but a plugin whose entry momentarily reads
        // as empty mid-write should not hash the same as one with none) --
        // and, more importantly, tells `(a, "")` apart from `(a followed by
        // nothing, "")` when two different-length manifests happen to
        // concatenate to the same bytes as entry content shifts between
        // them. Digests are fixed-width, so this only has to separate the
        // two fields, not delimit a list.
        h.update([entry.is_some() as u8]);
        if let Some(bytes) = entry {
            h.update(bytes);
        }
        Fingerprint(h.finalize().into())
    }
}

/// The fingerprint [`desired_state`] would compute for `plugin`, for a
/// caller that already has one and wants to record what a *fresh* start
/// looked like without re-running discovery.
///
/// `plugin_host::spawn_one` is that caller: `start_all` spawns a plugin the
/// ordinary way, outside this module's own diff entirely, and still has to
/// leave a [`Snapshot`] behind for the *next* `reconcile_tick` to compare
/// against -- otherwise a plugin's first tick after an ordinary launch would
/// find no `previous` entry for it and read a client that has been running
/// for five minutes as a fresh install.
pub fn fingerprint_of(plugin: &Plugin) -> Fingerprint {
    let manifest_text = std::fs::read(plugin.dir.join("plugin.json")).unwrap_or_default();
    let entry_bytes = plugin.entry_path().ok().and_then(|p| std::fs::read(p).ok());
    Fingerprint::of(&manifest_text, entry_bytes.as_deref())
}

/// What a running plugin's supervisor remembers about it, without keeping a
/// parsed [`Plugin`] around for the whole life of the process.
///
/// This is deliberately smaller than [`Desired`]: `cordial-runtime`'s `Shared`
/// keeps one of these per running plugin for as long as the process runs, and
/// a `Plugin` carries a manifest, a directory and a dependency list nothing
/// after spawn time still needs. [`diff`] only ever compares the fingerprint
/// and the granted set, so that is all a caller has to keep.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub fingerprint: Fingerprint,
    pub granted: BTreeSet<Capability>,
    /// The manifest's own `version` string, carried along only so a restart
    /// can be logged as "updated to 1.2.0" instead of "content changed" when
    /// there is a version to say. Never compared by [`diff`] itself --
    /// `fingerprint` already covers a version bump, because a version bump
    /// with nothing else different still changes the manifest's bytes.
    pub version: Option<String>,
}

/// One plugin this profile wants running right now: enough to spawn it, and
/// the fingerprint the next tick will compare against.
#[derive(Debug, Clone)]
pub struct Desired {
    pub plugin: Plugin,
    /// The raw grants-file entry for this id -- **not** intersected with
    /// `plugin.requested`. Matches `start_all`'s own launch-time semantics
    /// (`docs/plugin-api.md` calls this out explicitly: "the grants file is
    /// authoritative, and it is not intersected with the manifest"), so a
    /// plugin whose files have not changed gets exactly what a fresh launch
    /// would have given it. The narrower, intersected set a *restart* must
    /// use is computed separately, by [`intersect_for_restart`], only at the
    /// one call site that needs it -- see that function's doc for why.
    pub granted: BTreeSet<Capability>,
    pub fingerprint: Fingerprint,
}

impl Desired {
    /// What a caller that just started or restarted this plugin should keep
    /// for the *next* tick's comparison -- always `self.granted`, the raw
    /// grants-file entry, even when the process that was actually spawned
    /// got the narrower, intersected set [`intersect_for_restart`] computes.
    /// Recording the intersected set here instead would compare it against
    /// next tick's raw `desired_state` output and read as a permanent
    /// `Regrant` for any plugin whose manifest asks for less than the
    /// profile has granted it -- a false change reported every tick,
    /// forever, for a plugin nothing further happened to.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            fingerprint: self.fingerprint,
            granted: self.granted.clone(),
            version: self.plugin.manifest.version.clone(),
        }
    }
}

/// Every plugin this profile would run if Cordial launched fresh right now,
/// keyed by id.
///
/// Mirrors `plugin_host::start_all`'s own gate -- discovered, has code,
/// enabled in this profile, granted at least one capability -- silently; see
/// the module doc for why this is a second copy of that resolution rule
/// rather than a shared one, and why an unpacked plugin never appears here.
///
/// Reads a handful of small files (each plugin's `plugin.json` and, for a
/// plugin with code, its entry module) every call. Cheap enough to call once
/// a second for the plugin counts this project ships with; expensive to call
/// on every request a plugin makes, which is why the reconciler that owns
/// this call is a poll on its own timer and not a check folded into
/// `plugin_host::serve`'s request loop the way `refresh_grant` is.
pub fn desired_state(system_root: &Path, user_root: &Path, profile_dir: &Path) -> BTreeMap<String, Desired> {
    let mut found: Vec<Plugin> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for p in manifest::discover(system_root) {
        seen.insert(p.manifest.id.clone());
        found.push(p);
    }
    for p in manifest::discover(user_root) {
        if seen.insert(p.manifest.id.clone()) {
            found.push(p);
        }
    }

    let approved = grants::load(&grants::path_in(profile_dir));
    let mut out = BTreeMap::new();
    for plugin in found {
        let id = plugin.manifest.id.clone();
        // Data-only plugins (no `entry`) are not this reconciler's business:
        // there is no process to start, stop or restart. Their `flags.json`
        // and `overlay/` directory are read fresh by `flags::collect` and
        // `register_static_overlays` at every launch with no capability
        // check at all (ADR-021), and hot-reloading *those* is a different,
        // unaddressed feature -- see this crate's ADR-038 for the boundary.
        if !plugin.has_code() {
            continue;
        }
        if !enablement::is_enabled(profile_dir, &id) {
            continue;
        }
        let granted = approved.get(&id).cloned().unwrap_or_default();
        if granted.is_empty() {
            continue;
        }
        let fingerprint = fingerprint_of(&plugin);
        out.insert(id, Desired { plugin, granted, fingerprint });
    }
    out
}

/// What ought to happen to one plugin, between one reconcile tick and the
/// next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Not running; [`desired_state`] now wants it running. Covers a fresh
    /// install, a plugin switched on in Settings, and a plugin granted its
    /// first capability -- three user actions producing one outcome, because
    /// `desired_state`'s gate does not distinguish why a plugin newly passes
    /// it.
    Start,
    /// Running; [`desired_state`] no longer wants it running at all.
    /// Covers an uninstall, a plugin switched off in Settings, and a plugin's
    /// last capability being revoked -- see the module doc for why the third
    /// one belongs here rather than being left running with nothing granted.
    Stop,
    /// Running, and still wanted, but its own files changed underneath it --
    /// `plugin.json`, its entry module, or both. The version strings are
    /// carried for the log line: equal (including both absent) means the
    /// change is reported by fingerprint alone, "content changed"; different
    /// means it is reported as a version bump.
    Restart { from_version: Option<String>, to_version: Option<String> },
    /// Running, still wanted, files unchanged, but the profile's grant for it
    /// differs. No action follows from this on its own --
    /// `plugin_host::refresh_grant` already carries a running plugin's own
    /// next request to the new grant, the same mechanism this reconciler
    /// leans on rather than duplicates. `Regrant` exists so a caller can log
    /// that the change was *seen*, not to trigger anything.
    Regrant,
}

/// Compare what is running (`previous`) against what ought to be
/// (`desired`), keyed by plugin id.
///
/// An id present in both with no difference this type tracks is absent from
/// the result -- there is nothing to report, and a caller iterating the
/// result never has to filter out a `NoChange` variant to find the ids that
/// matter.
pub fn diff(
    previous: &BTreeMap<String, Snapshot>,
    desired: &BTreeMap<String, Desired>,
) -> BTreeMap<String, Change> {
    let mut out = BTreeMap::new();
    for (id, want) in desired {
        match previous.get(id) {
            None => {
                out.insert(id.clone(), Change::Start);
            }
            Some(had) if had.fingerprint != want.fingerprint => {
                out.insert(
                    id.clone(),
                    Change::Restart {
                        from_version: had.version.clone(),
                        to_version: want.plugin.manifest.version.clone(),
                    },
                );
            }
            Some(had) if had.granted != want.granted => {
                out.insert(id.clone(), Change::Regrant);
            }
            Some(_) => {}
        }
    }
    for id in previous.keys() {
        if !desired.contains_key(id) {
            out.insert(id.clone(), Change::Stop);
        }
    }
    out
}

/// What a hot-swapped plugin may use once its replacement process starts:
/// the profile's grant for it, narrowed to what the **new** manifest
/// actually requests.
///
/// This is deliberately *not* what `desired_state` hands back for an
/// ordinary start, and the difference is the whole point of this function
/// existing rather than the caller just reusing `Desired::granted`. A
/// plugin's grants file is not intersected with its manifest anywhere else
/// in this codebase -- a capability written into it by hand that the
/// manifest never requested is granted regardless (`docs/plugin-api.md` says
/// so, under "The grants file is authoritative"). That is safe when the
/// files on disk are the ones the user approved. It stops being safe the
/// moment those files can change out from under a grant that was given to a
/// *previous* version: a plugin update that drops a capability from its own
/// declared list must not silently keep whatever the old version earned,
/// because the grants file was never asked about the new version at all. A
/// hot-swapped plugin is exactly that case -- its files changed while the
/// grant stayed put -- so this is the one call site in the whole reconciler
/// that intersects, applied only when [`Change::Restart`] fires.
pub fn intersect_for_restart(
    profile_granted: &BTreeSet<Capability>,
    new_manifest_requested: &BTreeSet<Capability>,
) -> BTreeSet<Capability> {
    profile_granted.intersection(new_manifest_requested).cloned().collect()
}

/// Why a plugin this profile wants running could not be started, and so must
/// not be started again until something changes.
///
/// **The bug this exists for.** [`diff`] says `Start` for any wanted plugin
/// that is not running. A plugin whose spawn *failed* is not running, so it
/// was `Start` again one tick later, forever: three plugins on a machine with
/// no interpreter logged nine lines a second for a whole play session,
/// re-reading three manifests and re-failing three spawns each time, and every
/// one of those retries was certain to fail the same way. "Not running" and
/// "wanted" are not enough to justify an attempt; a failed attempt has to
/// remember what it failed *against*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    /// No interpreter on this machine. Lifts when one appears, and not
    /// before -- which the caller checks by looking for it, not by retrying
    /// the spawn to find out.
    RuntimeMissing,
    /// A failure the same files cannot get past: an `entry` that is not a
    /// usable path. Lifts only when the plugin's own files or grant change.
    Permanent,
    /// A spawn error that may pass by itself (out of descriptors, out of
    /// processes). Retried with a growing wait rather than every tick.
    Transient,
    /// Not a failure: an update whose manifest asks for nothing this profile
    /// has granted, so it has nothing it is allowed to run with. Lifts only
    /// when the grant changes (the user approves it in Settings) or the
    /// plugin's files change again.
    ///
    /// **This exists because the alternative was a bypass.** The restart path
    /// stopped such a plugin and moved on, and the next tick read "wanted and
    /// not running" as a plain `Start`, which hands over the raw grants-file
    /// entry -- exactly the set ADR-038 says an update must not inherit. The
    /// record is what makes "do not run it" survive the tick after the one
    /// that decided it.
    NeedsApproval,
}

/// The health line for an update that cannot run until the profile approves
/// what it asks for: `requested` is what the new manifest declares and
/// `granted` what the profile holds.
///
/// Short on purpose -- it is a tooltip in Settings -- and it names the
/// permissions, because "needs approval" alone leaves a user opening every
/// switch to find the one that changed.
pub fn approval_message(requested: &BTreeSet<Capability>, granted: &BTreeSet<Capability>) -> String {
    let missing: Vec<&str> = requested.difference(granted).map(|c| c.name()).collect();
    if missing.is_empty() {
        format!("{}: it now declares no permissions", crate::health::NEEDS_APPROVAL)
    } else {
        format!("{}: {}", crate::health::NEEDS_APPROVAL, missing.join(", "))
    }
}

/// A failed start, remembered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    kind: BlockKind,
    fingerprint: Fingerprint,
    granted: BTreeSet<Capability>,
    failures: u32,
    retry_at: std::time::Instant,
}

impl Blocked {
    /// Record a failure of `kind` for a plugin whose files and grant were
    /// `want`, given whatever was already recorded for it.
    ///
    /// Consecutive transient failures against unchanged files count up, which
    /// is what lengthens the wait; anything else starts the count again,
    /// because a plugin that was edited or re-granted has earned a prompt
    /// retry rather than inheriting the previous state's patience.
    pub fn record(
        kind: BlockKind,
        previous: Option<&Blocked>,
        want: &Desired,
        now: std::time::Instant,
    ) -> Blocked {
        let same_state = previous
            .is_some_and(|p| p.fingerprint == want.fingerprint && p.granted == want.granted);
        let failures = match previous {
            Some(p) if same_state && kind == BlockKind::Transient && p.kind == BlockKind::Transient => {
                p.failures + 1
            }
            _ => 1,
        };
        Blocked {
            kind,
            fingerprint: want.fingerprint,
            granted: want.granted.clone(),
            failures,
            retry_at: now + Self::backoff(failures),
        }
    }

    /// Two seconds, then four, doubling to about a minute.
    pub fn backoff(failures: u32) -> std::time::Duration {
        std::time::Duration::from_secs(1u64 << failures.clamp(1, 6))
    }

    pub fn kind(&self) -> &BlockKind {
        &self.kind
    }

    /// How long a [`BlockKind::Transient`] failure will wait before the next
    /// attempt, for the log line.
    pub fn wait(&self) -> std::time::Duration {
        Self::backoff(self.failures)
    }

    /// Whether `want` deserves another attempt now.
    ///
    /// A changed fingerprint or grant always does: that is a different plugin
    /// from the one that failed. Otherwise it depends on why it failed.
    /// `runtime_present` is only consulted for [`BlockKind::RuntimeMissing`],
    /// so a caller can make it a closure that stats the filesystem without
    /// paying for that on every tick of every other kind of failure.
    pub fn allows_retry(
        &self,
        want: &Desired,
        runtime_present: impl FnOnce() -> bool,
        now: std::time::Instant,
    ) -> bool {
        if self.fingerprint != want.fingerprint || self.granted != want.granted {
            return true;
        }
        match self.kind {
            BlockKind::RuntimeMissing => runtime_present(),
            BlockKind::Permanent | BlockKind::NeedsApproval => false,
            BlockKind::Transient => now >= self.retry_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write_plugin(root: &Path, id: &str, extra: &str, entry_body: &str) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            format!(r#"{{"id":"{id}","entry":"main.ts","capabilities":["log"]{extra}}}"#),
        )
        .unwrap();
        std::fs::write(dir.join("main.ts"), entry_body).unwrap();
    }

    fn scratch(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("cordial-reconcile-test-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        let system = root.join("system");
        let user = root.join("user");
        let profile = root.join("profile");
        std::fs::create_dir_all(&system).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&profile).unwrap();
        (system, user, profile)
    }

    #[test]
    fn a_plugin_with_no_grant_is_absent_from_the_desired_set() {
        let (system, user, profile) = scratch("no-grant");
        write_plugin(&user, "quiet", "", "");
        let desired = desired_state(&system, &user, &profile);
        assert!(!desired.contains_key("quiet"));
        let _ = std::fs::remove_dir_all(system.parent().unwrap());
    }

    #[test]
    fn a_plugin_disabled_in_settings_is_absent_even_when_granted() {
        let (system, user, profile) = scratch("disabled");
        write_plugin(&user, "off-switch", "", "");
        std::fs::write(
            enablement::path_in(&profile),
            r#"{"off-switch": false}"#,
        )
        .unwrap();
        std::fs::write(grants::path_in(&profile), r#"{"off-switch":["log"]}"#).unwrap();
        let desired = desired_state(&system, &user, &profile);
        assert!(!desired.contains_key("off-switch"));
        let _ = std::fs::remove_dir_all(system.parent().unwrap());
    }

    #[test]
    fn a_data_only_plugin_never_appears_in_the_desired_set() {
        let (system, user, profile) = scratch("data-only");
        let dir = user.join("texture-pack");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.json"), r#"{"id":"texture-pack"}"#).unwrap();
        std::fs::write(grants::path_in(&profile), r#"{"texture-pack":["log"]}"#).unwrap();
        let desired = desired_state(&system, &user, &profile);
        assert!(!desired.contains_key("texture-pack"));
        let _ = std::fs::remove_dir_all(system.parent().unwrap());
    }

    #[test]
    fn a_granted_enabled_plugin_with_code_is_desired() {
        let (system, user, profile) = scratch("desired");
        write_plugin(&user, "hello", "", "console.error('x')");
        std::fs::write(grants::path_in(&profile), r#"{"hello":["log"]}"#).unwrap();
        let desired = desired_state(&system, &user, &profile);
        assert!(desired.contains_key("hello"));
        let _ = std::fs::remove_dir_all(system.parent().unwrap());
    }

    #[test]
    fn a_user_plugin_shadowed_by_a_built_in_id_is_not_desired_twice() {
        let (system, user, profile) = scratch("shadow");
        write_plugin(&system, "dup", "", "// built-in");
        write_plugin(&user, "dup", r#","name":"impostor""#, "// user copy");
        std::fs::write(grants::path_in(&profile), r#"{"dup":["log"]}"#).unwrap();
        let desired = desired_state(&system, &user, &profile);
        assert_eq!(desired.len(), 1);
        // The built-in copy wins; its manifest has no `name` field.
        assert_eq!(desired["dup"].plugin.manifest.name, "");
        let _ = std::fs::remove_dir_all(system.parent().unwrap());
    }

    #[test]
    fn editing_the_entry_module_changes_the_fingerprint_even_with_the_same_manifest() {
        let (system, user, profile) = scratch("entry-edit");
        write_plugin(&user, "p", "", "console.error(1)");
        std::fs::write(grants::path_in(&profile), r#"{"p":["log"]}"#).unwrap();
        let before = desired_state(&system, &user, &profile)["p"].fingerprint;
        write_plugin(&user, "p", "", "console.error(2)");
        let after = desired_state(&system, &user, &profile)["p"].fingerprint;
        assert_ne!(before, after);
        let _ = std::fs::remove_dir_all(system.parent().unwrap());
    }

    #[test]
    fn an_untouched_plugin_has_a_stable_fingerprint_across_two_reads() {
        let (system, user, profile) = scratch("stable");
        write_plugin(&user, "p", "", "console.error(1)");
        std::fs::write(grants::path_in(&profile), r#"{"p":["log"]}"#).unwrap();
        let a = desired_state(&system, &user, &profile)["p"].fingerprint;
        let b = desired_state(&system, &user, &profile)["p"].fingerprint;
        assert_eq!(a, b);
        let _ = std::fs::remove_dir_all(system.parent().unwrap());
    }

    /// A fingerprint distinguishable by nothing but the byte it repeats --
    /// `diff` only ever compares fingerprints for equality, so the `diff`
    /// tests below only need two that differ and one that repeats, never a
    /// real digest of real file bytes. The tests above this one already
    /// exercise the real hasher, against real files on disk.
    fn fp(seed: u8) -> Fingerprint {
        Fingerprint([seed; 32])
    }

    fn snap(seed: u8, granted: &[Capability], version: Option<&str>) -> Snapshot {
        Snapshot {
            fingerprint: fp(seed),
            granted: granted.iter().copied().collect(),
            version: version.map(str::to_string),
        }
    }

    fn desired(dir: &Path, seed: u8, granted: &[Capability], version: Option<&str>) -> Desired {
        let mut manifest_json = format!(
            r#"{{"id":"p","entry":"main.ts","capabilities":["log"]"#
        );
        if let Some(v) = version {
            manifest_json.push_str(&format!(r#","version":"{v}""#));
        }
        manifest_json.push('}');
        let plugin = manifest::parse(&manifest_json, dir).unwrap();
        Desired { plugin, granted: granted.iter().copied().collect(), fingerprint: fp(seed) }
    }

    #[test]
    fn a_plugin_absent_from_previous_starts() {
        let dir = PathBuf::from("/plugins/p");
        let previous = BTreeMap::new();
        let mut want = BTreeMap::new();
        want.insert("p".to_string(), desired(&dir, 1, &[Capability::Log], None));
        let changes = diff(&previous, &want);
        assert_eq!(changes.get("p"), Some(&Change::Start));
    }

    #[test]
    fn a_plugin_absent_from_desired_stops() {
        let mut previous = BTreeMap::new();
        previous.insert("p".to_string(), snap(1, &[Capability::Log], None));
        let desired = BTreeMap::new();
        let changes = diff(&previous, &desired);
        assert_eq!(changes.get("p"), Some(&Change::Stop));
    }

    #[test]
    fn a_changed_fingerprint_with_a_version_bump_restarts_and_names_both_versions() {
        let dir = PathBuf::from("/plugins/p");
        let mut previous = BTreeMap::new();
        previous.insert("p".to_string(), snap(1, &[Capability::Log], Some("1.0.0")));
        let mut want = BTreeMap::new();
        want.insert("p".to_string(), desired(&dir, 2, &[Capability::Log], Some("1.1.0")));
        let changes = diff(&previous, &want);
        assert_eq!(
            changes.get("p"),
            Some(&Change::Restart {
                from_version: Some("1.0.0".to_string()),
                to_version: Some("1.1.0".to_string()),
            })
        );
    }

    #[test]
    fn a_changed_fingerprint_with_no_version_restarts_and_names_neither() {
        let dir = PathBuf::from("/plugins/p");
        let mut previous = BTreeMap::new();
        previous.insert("p".to_string(), snap(1, &[Capability::Log], None));
        let mut want = BTreeMap::new();
        want.insert("p".to_string(), desired(&dir, 2, &[Capability::Log], None));
        let changes = diff(&previous, &want);
        assert_eq!(changes.get("p"), Some(&Change::Restart { from_version: None, to_version: None }));
    }

    #[test]
    fn an_unchanged_fingerprint_with_a_changed_grant_set_regrants_without_restarting() {
        let dir = PathBuf::from("/plugins/p");
        let mut previous = BTreeMap::new();
        previous.insert("p".to_string(), snap(1, &[Capability::Log], None));
        let mut want = BTreeMap::new();
        want.insert(
            "p".to_string(),
            desired(&dir, 1, &[Capability::Log, Capability::FlagsRead], None),
        );
        let changes = diff(&previous, &want);
        assert_eq!(changes.get("p"), Some(&Change::Regrant));
    }

    #[test]
    fn nothing_changed_produces_no_entry_at_all() {
        let dir = PathBuf::from("/plugins/p");
        let mut previous = BTreeMap::new();
        previous.insert("p".to_string(), snap(1, &[Capability::Log], Some("1.0.0")));
        let mut want = BTreeMap::new();
        want.insert("p".to_string(), desired(&dir, 1, &[Capability::Log], Some("1.0.0")));
        let changes = diff(&previous, &want);
        assert!(changes.is_empty());
    }

    #[test]
    fn restart_intersects_the_profiles_grant_with_the_new_manifests_request() {
        // The scenario ADR-038 exists to close: version 1 requested and was
        // granted flags.write; version 2's manifest only requests log. The
        // grants file still says flags.write because nothing has asked the
        // user about it again -- the restarted process must not get it.
        let profile_granted: BTreeSet<Capability> =
            [Capability::Log, Capability::FlagsWrite].into_iter().collect();
        let new_requested: BTreeSet<Capability> = [Capability::Log].into_iter().collect();
        let effective = intersect_for_restart(&profile_granted, &new_requested);
        assert_eq!(effective, [Capability::Log].into_iter().collect());
    }

    #[test]
    fn restart_never_grants_what_the_profile_never_approved() {
        // The other half: version 2 asks for a capability nobody approved at
        // all. Requesting it is not the same as holding it.
        let profile_granted: BTreeSet<Capability> = [Capability::Log].into_iter().collect();
        let new_requested: BTreeSet<Capability> =
            [Capability::Log, Capability::UrlOpen].into_iter().collect();
        let effective = intersect_for_restart(&profile_granted, &new_requested);
        assert_eq!(effective, [Capability::Log].into_iter().collect());
    }

    fn desired_for(user: &Path, profile: &Path) -> Desired {
        desired_state(&user.join("nowhere"), user, profile).remove("quiet").expect("granted and enabled")
    }

    fn blocked_fixture(name: &str) -> (Desired, PathBuf, PathBuf) {
        let (system, user, profile) = scratch(name);
        write_plugin(&user, "quiet", "", "// v1\n");
        std::fs::write(grants::path_in(&profile), r#"{"quiet":["log"]}"#).unwrap();
        let want = desired_for(&user, &profile);
        drop(system);
        (want, user, profile)
    }

    #[test]
    fn a_missing_runtime_blocks_until_it_appears_and_not_before() {
        let (want, user, _p) = blocked_fixture("blocked-runtime");
        let now = std::time::Instant::now();
        let b = Blocked::record(BlockKind::RuntimeMissing, None, &want, now);
        let later = now + std::time::Duration::from_secs(3600);
        assert!(!b.allows_retry(&want, || false, later), "an hour later, still no Deno, still blocked");
        assert!(b.allows_retry(&want, || true, now), "Deno appeared");
        let _ = std::fs::remove_dir_all(user.parent().unwrap());
    }

    #[test]
    fn the_runtime_is_only_looked_for_when_that_is_what_the_plugin_waits_on() {
        let (want, user, _p) = blocked_fixture("blocked-lookup");
        let now = std::time::Instant::now();
        for kind in [BlockKind::Permanent, BlockKind::Transient, BlockKind::NeedsApproval] {
            let b = Blocked::record(kind, None, &want, now);
            let _ = b.allows_retry(&want, || panic!("a filesystem lookup nothing asked for"), now);
        }
        let _ = std::fs::remove_dir_all(user.parent().unwrap());
    }

    #[test]
    fn a_permanent_failure_lifts_only_when_the_plugin_or_its_grant_changes() {
        let (want, user, profile) = blocked_fixture("blocked-permanent");
        let now = std::time::Instant::now();
        let b = Blocked::record(BlockKind::Permanent, None, &want, now);
        let much_later = now + std::time::Duration::from_secs(86_400);
        assert!(!b.allows_retry(&want, || true, much_later));

        std::fs::write(user.join("quiet").join("main.ts"), "// v2\n").unwrap();
        let edited = desired_for(&user, &profile);
        assert!(b.allows_retry(&edited, || true, now), "edited files are a different plugin");

        std::fs::write(grants::path_in(&profile), r#"{"quiet":["log","notify.send"]}"#).unwrap();
        let regranted = desired_for(&user, &profile);
        assert!(b.allows_retry(&regranted, || true, now), "a new grant is a different plugin");
        let _ = std::fs::remove_dir_all(user.parent().unwrap());
    }

    #[test]
    fn an_update_waiting_for_approval_lifts_only_on_new_files_or_a_new_grant() {
        let (want, user, profile) = blocked_fixture("blocked-approval");
        let now = std::time::Instant::now();
        let b = Blocked::record(BlockKind::NeedsApproval, None, &want, now);
        let much_later = now + std::time::Duration::from_secs(86_400);
        assert!(!b.allows_retry(&want, || true, much_later), "time does not approve anything");

        std::fs::write(user.join("quiet").join("main.ts"), "// v2\n").unwrap();
        assert!(b.allows_retry(&desired_for(&user, &profile), || true, now));

        std::fs::write(grants::path_in(&profile), r#"{"quiet":["log","notify.send"]}"#).unwrap();
        assert!(b.allows_retry(&desired_for(&user, &profile), || true, now));
        let _ = std::fs::remove_dir_all(user.parent().unwrap());
    }

    #[test]
    fn the_approval_message_names_what_the_profile_has_not_granted() {
        let requested = BTreeSet::from([Capability::Log, Capability::UrlOpen, Capability::NotifySend]);
        let granted = BTreeSet::from([Capability::Log]);
        assert_eq!(approval_message(&requested, &granted), "Update needs approval: notify.send, url.open");
        assert_eq!(
            approval_message(&BTreeSet::new(), &granted),
            "Update needs approval: it now declares no permissions"
        );
    }

    #[test]
    fn a_transient_failure_waits_longer_each_time_and_starts_over_when_the_plugin_changes() {
        let (want, user, profile) = blocked_fixture("blocked-transient");
        let t0 = std::time::Instant::now();
        let first = Blocked::record(BlockKind::Transient, None, &want, t0);
        assert!(!first.allows_retry(&want, || true, t0 + std::time::Duration::from_secs(1)));
        assert!(first.allows_retry(&want, || true, t0 + std::time::Duration::from_secs(2)));

        let second = Blocked::record(BlockKind::Transient, Some(&first), &want, t0);
        assert!(!second.allows_retry(&want, || true, t0 + std::time::Duration::from_secs(3)));
        assert!(second.allows_retry(&want, || true, t0 + std::time::Duration::from_secs(4)));

        // Capped, so a plugin does not go a whole session without trying.
        let mut b = first;
        for _ in 0..20 {
            b = Blocked::record(BlockKind::Transient, Some(&b), &want, t0);
        }
        assert_eq!(b.wait(), std::time::Duration::from_secs(64));

        std::fs::write(user.join("quiet").join("main.ts"), "// v2\n").unwrap();
        let edited = desired_for(&user, &profile);
        let reset = Blocked::record(BlockKind::Transient, Some(&b), &edited, t0);
        assert_eq!(reset.wait(), std::time::Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(user.parent().unwrap());
    }
}
