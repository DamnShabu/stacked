//! Where the AppImage's own WebKitGTK finds its helper processes.
//!
//! WebKitGTK does not run a page in the process that asked for it. It spawns
//! `WebKitNetworkProcess` and `WebKitWebProcess` by absolute path, from a
//! directory compiled into `libwebkitgtk-6.0.so` (`PKGLIBEXECDIR`), and when
//! that spawn fails it does not return an error: it calls `g_error`, which
//! aborts the whole process. In cordial-run that process is the engine. A user
//! pressing "Servers" on a game page in v0.21.3 lost the client to exactly that,
//! `Failed to spawn child process
//! "/usr/lib/x86_64-linux-gnu/webkitgtk-6.0/WebKitNetworkProcess" (No such file
//! or directory)`, from inside the Roblox manager's Flatpak, whose GNOME runtime
//! has that directory but keeps its own helpers in `/usr/libexec`.
//!
//! The AppImage bundles Ubuntu 24.04's WebKitGTK (ADR-032), and Ubuntu bakes in
//! `/usr/lib/x86_64-linux-gnu/webkitgtk-6.0`. No environment variable moves it:
//! `WEBKIT_EXEC_PATH` is read only in `ENABLE(DEVELOPER_MODE)` builds and the
//! string is not in the shipped library at all. AppRun used to make the path
//! exist with a mount namespace, which cannot work inside a Flatpak and never
//! ran for the manager, which execs `usr/bin/cordial-run` directly.
//!
//! So the build rewrites that one directory string in the bundled library to
//! [`STAGING_PREFIX`] plus a hash of the helpers it bundles, padded to the same
//! length (ADR-045, and `NOTICE` for the modification), and this module puts
//! copies of the helpers there before the first web view exists. The directory
//! is keyed by the helpers' contents so two Stacked versions carrying different
//! WebKit builds never share one, and it holds copies rather than symlinks so it
//! does not dangle when the AppImage mount that staged it goes away.
//!
//! WebKit's own bubblewrap sandbox binds `PKGLIBEXECDIR` at the same path (the
//! same string feeds both), and binds every directory on `LD_LIBRARY_PATH`,
//! which is how the copied helpers find the bundled libraries inside it. They
//! carry no RUNPATH of their own, so a launch without `LD_LIBRARY_PATH` pointing
//! at the AppImage's `usr/lib` cannot run them; AppRun and the Roblox manager's
//! launcher both set it.
//!
//! Anything that would leave WebKit to abort is checked here first and turned
//! into a refusal with a reason, which `webview::open` prints. A web
//! view that does not open is a bug report; a client that vanishes is a lost
//! game.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// The start of the directory the bundled library is patched to look in. In
/// `/tmp` because it is the one directory every host lets an unprivileged user
/// create something in at a fixed path, and the path has to be fixed: it is
/// compiled into the library and cannot depend on who runs it.
pub const STAGING_PREFIX: &str = "/tmp/.stacked-webkit-";

/// The fewest hash characters a staging directory name may carry. Below this
/// two unrelated WebKit builds could plausibly collide on one directory.
const MIN_KEY_CHARS: usize = 12;

/// Written by `build-appimage.sh` beside the bundled libraries, relative to the
/// AppImage's `usr/`: the staging directory the library was patched to. Its
/// absence means this is not the AppImage layout -- a distribution package, the
/// Flatpak, or a development build -- whose WebKitGTK finds its own helpers.
pub const SIDECAR: &str = "share/stacked/webkit-helper-dir";

/// The helpers WebKit spawns, and whether a web view can work without each.
/// The GPU process is only spawned on some configurations; a missing one is
/// left for WebKit to report rather than refused up front.
const HELPERS: [(&str, bool); 3] =
    [("WebKitNetworkProcess", true), ("WebKitWebProcess", true), ("WebKitGPUProcess", false)];

/// Where WebKit's sandbox programs are, as compiled into Ubuntu's library --
/// read off it with `grep -abo`, 2026-10-01. Absolute, like the helpers, and a
/// missing one aborts the client the same way.
const SANDBOX_TOOLS: [&str; 2] = ["/usr/bin/bwrap", "/usr/bin/xdg-dbus-proxy"];

/// What [`prepare`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum Helpers {
    /// Not the AppImage layout; WebKitGTK's own paths are the system's.
    System,
    /// The helpers are in place at this directory.
    Staged(PathBuf),
}

/// The staging directory for a library whose baked helper directory is
/// `baked`, keyed by `key` (lowercase hex). It is exactly as long as `baked`, so
/// the build can overwrite the string in place without moving anything after it.
pub fn staging_dir_for(baked: &str, key: &str) -> Result<String, String> {
    if !key.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(format!("the key {key:?} is not lowercase hex"));
    }
    let room = baked.len().saturating_sub(STAGING_PREFIX.len());
    if room < MIN_KEY_CHARS {
        return Err(format!(
            "{baked:?} is {} bytes; a staging path needs at least {}",
            baked.len(),
            STAGING_PREFIX.len() + MIN_KEY_CHARS
        ));
    }
    if key.len() < room {
        return Err(format!("the key has {} characters and the path needs {room}", key.len()));
    }
    Ok(format!("{STAGING_PREFIX}{}", &key[..room]))
}

/// Rewrite every C string in `lib` that is `from` or starts with `from/` so it
/// starts with `to` instead, in place. Returns how many were rewritten.
///
/// Refuses unless the two are the same length, and refuses if `from` also
/// appears somewhere that is not the start of a string: a path embedded in a
/// longer one is not the constant this is meant to change, and patching it
/// would be a silent edit of something nobody looked at.
pub fn patch_baked_dir(lib: &mut [u8], from: &str, to: &str) -> Result<usize, String> {
    let (from, to) = (from.as_bytes(), to.as_bytes());
    if from.len() != to.len() || from.is_empty() {
        return Err(format!(
            "the replacement must be the same length as the original ({} and {} bytes)",
            to.len(),
            from.len()
        ));
    }
    let mut whole = Vec::new();
    let mut partial = 0;
    let mut i = 0;
    while let Some(at) = find(&lib[i..], from).map(|at| at + i) {
        let starts = at == 0 || lib[at - 1] == 0;
        let ends = matches!(lib.get(at + from.len()), Some(0) | Some(b'/'));
        if starts && ends {
            whole.push(at);
        } else {
            partial += 1;
        }
        i = at + 1;
    }
    if partial > 0 {
        return Err(format!(
            "{} occurs {partial} time(s) inside a longer string; refusing to patch",
            String::from_utf8_lossy(from)
        ));
    }
    if whole.is_empty() {
        return Err(format!("{} does not occur in the library", String::from_utf8_lossy(from)));
    }
    for &at in &whole {
        lib[at..at + to.len()].copy_from_slice(to);
    }
    Ok(whole.len())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// [`patch_baked_dir`] on a file, replaced atomically with its mode kept.
/// Returns the staging directory and the number of strings rewritten. Called by
/// the `patch_webkit_helper_dir` example, which `build-appimage.sh` runs.
pub fn patch_library_file(lib: &Path, baked: &str, key: &str) -> Result<(String, usize), String> {
    let to = staging_dir_for(baked, key)?;
    let mut bytes = fs::read(lib).map_err(|e| format!("{}: {e}", lib.display()))?;
    let count = patch_baked_dir(&mut bytes, baked, &to)?;
    let mode = fs::metadata(lib).map_err(|e| format!("{}: {e}", lib.display()))?.permissions();
    let tmp = lib.with_extension("stacked-patch");
    fs::write(&tmp, &bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::set_permissions(&tmp, mode).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, lib).map_err(|e| format!("{}: {e}", lib.display()))?;
    Ok((to, count))
}

/// Make sure a web view can be created without WebKit aborting the process.
///
/// The AppImage's `usr/` is found from this executable, which is
/// `usr/bin/cordial-run` however it was launched.
pub fn prepare() -> Result<Helpers, String> {
    let exe = std::env::current_exe().map_err(|e| format!("could not find this executable: {e}"))?;
    let Some(usr) = exe.parent().and_then(Path::parent) else {
        return Ok(Helpers::System);
    };
    let dir = prepare_from(usr)?;
    if let Helpers::Staged(_) = dir {
        if let Some(missing) = missing_sandbox_tool() {
            return Err(missing);
        }
    }
    Ok(dir)
}

/// [`prepare`]'s staging half, for an AppImage `usr/` at `usr`.
pub fn prepare_from(usr: &Path) -> Result<Helpers, String> {
    let sidecar = usr.join(SIDECAR);
    let target = match fs::read_to_string(&sidecar) {
        Ok(text) => text.trim().to_owned(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Helpers::System),
        Err(e) => return Err(format!("could not read {}: {e}", sidecar.display())),
    };
    // The sidecar is ours, but it names a directory this is about to create
    // and execute from, so it is held to the one shape the build writes.
    let key = target.strip_prefix(STAGING_PREFIX).unwrap_or_default();
    if key.len() < MIN_KEY_CHARS || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{} names {target:?}, which is not a staging directory", sidecar.display()));
    }
    let target = PathBuf::from(target);
    stage(usr, &target)?;
    Ok(Helpers::Staged(target))
}

/// Copy the helpers and the injected bundle from the AppImage's `usr/` into
/// `target`, creating it if need be, and refuse if it is not safe to run
/// programs from.
///
/// `/tmp` is shared, so `target` may already exist and belong to somebody else.
/// Executing from a directory another user can write would hand them this
/// user's session, so the directory must be a real directory (not a symlink),
/// owned by this user and closed to everyone else. `/tmp`'s sticky bit then
/// keeps anyone else from renaming it away after the check. A directory that
/// fails is refused, not repaired: on a shared machine the likeliest cause is a
/// second user running the same Stacked at once, and there is no fixed path two
/// users can both own.
pub fn stage(usr: &Path, target: &Path) -> Result<(), String> {
    match fs::DirBuilder::new().mode(0o700).create(target) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("could not create {}: {e}", target.display())),
    }
    check_private_dir(target)?;
    let bundle = target.join("injected-bundle");
    match fs::DirBuilder::new().mode(0o700).create(&bundle) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("could not create {}: {e}", bundle.display())),
    }
    check_private_dir(&bundle)?;

    let libexec = usr.join("libexec/webkitgtk-6.0");
    for (name, required) in HELPERS {
        let from = libexec.join(name);
        if !from.is_file() {
            if required {
                return Err(format!("{} is missing from this install", from.display()));
            }
            continue;
        }
        copy_if_changed(&from, &target.join(name))?;
    }
    let bundled = usr.join("lib/webkitgtk-6.0/injected-bundle");
    let entries =
        fs::read_dir(&bundled).map_err(|e| format!("could not read {}: {e}", bundled.display()))?;
    for entry in entries {
        let from = entry.map_err(|e| format!("{}: {e}", bundled.display()))?.path();
        if from.extension().is_some_and(|ext| ext == "so") {
            copy_if_changed(&from, &bundle.join(from.file_name().unwrap_or_default()))?;
        }
    }

    // A `noexec` /tmp is a legitimate hardening choice and would turn every
    // spawn into EACCES, which WebKit answers with the same abort.
    if noexec(target)? {
        return Err(format!(
            "{} is mounted noexec, so WebKit's helper processes cannot run from it",
            target.parent().unwrap_or(target).display()
        ));
    }
    Ok(())
}

fn check_private_dir(dir: &Path) -> Result<(), String> {
    let meta =
        fs::symlink_metadata(dir).map_err(|e| format!("could not inspect {}: {e}", dir.display()))?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let me = unsafe { libc::geteuid() };
    if !meta.file_type().is_dir() {
        return Err(format!("{} exists and is not a directory", dir.display()));
    }
    if meta.uid() != me {
        return Err(format!(
            "{} belongs to another user (uid {}), probably one running Stacked at the same time",
            dir.display(),
            meta.uid()
        ));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(format!("{} is open to other users (mode {:o})", dir.display(), meta.mode() & 0o777));
    }
    Ok(())
}

/// Copy `from` to `to` unless `to` already holds the same bytes. Written under
/// a temporary name and renamed into place, so a second Stacked starting at the
/// same moment, or a helper being spawned from this directory right now, never
/// sees a half-written file.
fn copy_if_changed(from: &Path, to: &Path) -> Result<(), String> {
    let bytes = fs::read(from).map_err(|e| format!("could not read {}: {e}", from.display()))?;
    if let Ok(meta) = fs::symlink_metadata(to) {
        if meta.file_type().is_file()
            && meta.permissions().mode() & 0o700 == 0o700
            && fs::read(to).is_ok_and(|have| have == bytes)
        {
            return Ok(());
        }
    }
    let tmp = to.with_file_name(format!(
        ".{}.{}",
        to.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let _ = fs::remove_file(&tmp);
    let write = || -> io::Result<()> {
        let mut file =
            fs::OpenOptions::new().write(true).create_new(true).mode(0o700).open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, to)
    };
    write().map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("could not write {}: {e}", to.display())
    })
}

fn noexec(dir: &Path) -> Result<bool, String> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes())
        .map_err(|_| format!("{} contains a NUL byte", dir.display()))?;
    // SAFETY: `path` is a valid NUL-terminated string and `stat` is a plain
    // out-parameter of the right type, zero-initialised.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return Err(format!("could not statvfs {}: {}", dir.display(), io::Error::last_os_error()));
    }
    Ok(stat.f_flag & libc::ST_NOEXEC != 0)
}

/// The sandbox program WebKit would fail to exec, if there is one.
///
/// WebKit wraps each web process in `bwrap` (and its session bus in
/// `xdg-dbus-proxy`) unless it is inside a Flatpak, inside a container with a
/// `/run/.containerenv` (where it probes `bwrap` itself and does without when
/// it fails), or told not to by `WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS`.
/// Those are the conditions in upstream's `shouldUseBubblewrap` and
/// `WebProcessPool::setSandboxEnabled`, 2.52. Everywhere else a host without
/// them -- NixOS has neither at that path, Arch has them only with
/// `bubblewrap` and `xdg-dbus-proxy` installed -- gets the same abort as a
/// missing helper. AppRun plants the bundled copies where it can, and when it
/// has they exist by the time this runs.
fn missing_sandbox_tool() -> Option<String> {
    if Path::new("/.flatpak-info").exists() || Path::new("/run/.containerenv").exists() {
        return None;
    }
    if std::env::var_os("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS").is_some_and(|v| v != "0") {
        return None;
    }
    SANDBOX_TOOLS.iter().find(|tool| !is_executable(Path::new(tool))).map(|tool| {
        format!(
            "WebKitGTK sandboxes its web process with {tool}, which this host does not have \
             (install bubblewrap and xdg-dbus-proxy, or start Stacked through its AppImage, \
             which provides them where it can)"
        )
    })
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UBUNTU: &str = "/usr/lib/x86_64-linux-gnu/webkitgtk-6.0";

    #[test]
    fn the_staging_path_is_exactly_as_long_as_the_one_it_replaces() {
        let key = "425510dcc60b03479bd2c5aa0b1d9f2e1c7e6b5a4d3c2b1a0f9e8d7c6b5a4d3c";
        let dir = staging_dir_for(UBUNTU, key).unwrap();
        assert_eq!(dir, "/tmp/.stacked-webkit-425510dcc60b03479b");
        assert_eq!(dir.len(), UBUNTU.len());
    }

    #[test]
    fn a_baked_path_too_short_to_carry_a_key_is_refused() {
        // Fedora's `/usr/libexec/webkitgtk-6.0` is 26 bytes: five hex
        // characters after the prefix, too few to key on.
        assert!(staging_dir_for("/usr/libexec/webkitgtk-6.0", &"a".repeat(64)).is_err());
        assert!(staging_dir_for(UBUNTU, "NOTHEX").is_err());
        assert!(staging_dir_for(UBUNTU, "abc").is_err());
    }

    fn library() -> Vec<u8> {
        let mut lib = b"\x7fELF\0junk\0".to_vec();
        lib.extend_from_slice(UBUNTU.as_bytes());
        lib.extend_from_slice(b"\0other\0");
        lib.extend_from_slice(UBUNTU.as_bytes());
        lib.extend_from_slice(b"/injected-bundle/\0tail\0");
        lib
    }

    #[test]
    fn both_the_helper_dir_and_the_bundle_dir_are_rewritten_and_nothing_else_moves() {
        let mut lib = library();
        let before = lib.clone();
        let to = staging_dir_for(UBUNTU, &"0123456789abcdef".repeat(4)).unwrap();
        assert_eq!(patch_baked_dir(&mut lib, UBUNTU, &to).unwrap(), 2);
        assert_eq!(lib.len(), before.len());
        let text = String::from_utf8_lossy(&lib);
        assert!(text.contains(&format!("\0{to}\0other\0{to}/injected-bundle/\0tail\0")));
        assert!(!text.contains(UBUNTU));
    }

    #[test]
    fn a_path_embedded_in_a_longer_string_stops_the_patch() {
        let mut lib = library();
        lib.extend_from_slice(b"/opt");
        lib.extend_from_slice(UBUNTU.as_bytes());
        lib.push(0);
        let untouched = lib.clone();
        let to = staging_dir_for(UBUNTU, &"f".repeat(64)).unwrap();
        assert!(patch_baked_dir(&mut lib, UBUNTU, &to).is_err());
        assert_eq!(lib, untouched);
    }

    #[test]
    fn a_library_without_the_path_or_a_wrong_length_replacement_is_refused() {
        let mut lib = b"\0nothing here\0".to_vec();
        assert!(patch_baked_dir(&mut lib, UBUNTU, &"x".repeat(UBUNTU.len())).is_err());
        let mut lib = library();
        assert!(patch_baked_dir(&mut lib, UBUNTU, "/tmp/short").is_err());
    }

    /// A fake AppImage `usr/` with the helpers and the bundle in it.
    fn appimage(root: &Path) -> PathBuf {
        let usr = root.join("usr");
        fs::create_dir_all(usr.join("libexec/webkitgtk-6.0")).unwrap();
        fs::create_dir_all(usr.join("lib/webkitgtk-6.0/injected-bundle")).unwrap();
        for name in ["WebKitNetworkProcess", "WebKitWebProcess", "MiniBrowser"] {
            fs::write(usr.join("libexec/webkitgtk-6.0").join(name), name).unwrap();
        }
        fs::write(usr.join("lib/webkitgtk-6.0/injected-bundle/libwebkitgtkinjectedbundle.so"), "so")
            .unwrap();
        usr
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stacked-webkit-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn staging_copies_the_helpers_privately_and_is_idempotent() {
        let root = scratch("stage");
        let usr = appimage(&root);
        let target = root.join("staged");
        stage(&usr, &target).unwrap();
        stage(&usr, &target).unwrap();
        assert_eq!(fs::read(target.join("WebKitWebProcess")).unwrap(), b"WebKitWebProcess");
        assert_eq!(
            fs::read(target.join("injected-bundle/libwebkitgtkinjectedbundle.so")).unwrap(),
            b"so"
        );
        assert!(!target.join("MiniBrowser").exists(), "only what WebKit spawns is staged");
        assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(target.join("WebKitNetworkProcess")).unwrap().mode() & 0o777, 0o700);
        // A stale copy -- a /tmp cleaner truncating it, say -- is replaced.
        fs::write(target.join("WebKitWebProcess"), "stale").unwrap();
        stage(&usr, &target).unwrap();
        assert_eq!(fs::read(target.join("WebKitWebProcess")).unwrap(), b"WebKitWebProcess");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_directory_others_can_reach_or_a_symlink_is_refused_not_used() {
        let root = scratch("refuse");
        let usr = appimage(&root);
        let open = root.join("open");
        fs::create_dir(&open).unwrap();
        fs::set_permissions(&open, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(stage(&usr, &open).unwrap_err().contains("open to other users"));
        let link = root.join("link");
        std::os::unix::fs::symlink(&open, &link).unwrap();
        assert!(stage(&usr, &link).unwrap_err().contains("not a directory"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_helper_is_a_refusal_with_its_name() {
        let root = scratch("missing");
        let usr = appimage(&root);
        fs::remove_file(usr.join("libexec/webkitgtk-6.0/WebKitNetworkProcess")).unwrap();
        let err = stage(&usr, &root.join("staged")).unwrap_err();
        assert!(err.contains("WebKitNetworkProcess"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn no_sidecar_means_the_system_webkit_and_a_bad_one_is_refused() {
        let root = scratch("sidecar");
        let usr = appimage(&root);
        assert_eq!(prepare_from(&usr).unwrap(), Helpers::System);
        fs::create_dir_all(usr.join("share/stacked")).unwrap();
        fs::write(usr.join(SIDECAR), "/home/someone/bin\n").unwrap();
        assert!(prepare_from(&usr).is_err());
        let _ = fs::remove_dir_all(&root);
    }
}
