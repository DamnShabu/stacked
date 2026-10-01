# ADR-045: The AppImage's WebKitGTK looks for its helpers where Stacked puts them

**Status:** Accepted
**Date:** 2026-10-01
**Supersedes:** the helper-directory binds in `packaging/appimage/AppRun` (ADR-032's "mount-namespace bind" section)
**Related:** [ADR-032](ADR-032-appimage-build-base-moves-to-ubuntu-24-04.md), [ADR-001](ADR-001-in-process-hooking.md)

## Context

WebKitGTK runs a page in helper processes it spawns by absolute path, from a
directory compiled into `libwebkitgtk-6.0.so` (`PKGLIBEXECDIR`). The AppImage
bundles Ubuntu 24.04's library, which bakes in
`/usr/lib/x86_64-linux-gnu/webkitgtk-6.0`. When the spawn fails WebKit calls
`g_error`, which aborts the process -- and in `cordial-run` that is the engine.

v0.21.3 did exactly that when a user pressed "Servers" on a game page:
`Failed to spawn child process
"/usr/lib/x86_64-linux-gnu/webkitgtk-6.0/WebKitNetworkProcess" (No such file or
directory)`, then `trap int3` in `libglib`. The client was running inside the
Roblox manager's Flatpak (GNOME 50 runtime), which has that directory but keeps
its own helpers in `/usr/libexec`. Reproduced verbatim in that sandbox on
2026-10-01.

Nothing in the environment moves the directory. `WEBKIT_EXEC_PATH` is read only
under `ENABLE(DEVELOPER_MODE)` (upstream `ProcessExecutablePathGLib.cpp`, 2.52)
and the string is absent from the shipped library. AppRun made the path exist
with a bwrap mount namespace instead, and that could not reach this user:

- The manager execs `usr/bin/cordial-run` directly, so AppRun never runs.
- Inside a Flatpak no namespace can be made. bwrap there says "No permissions
  to create a new namespace" (measured).
- AppRun bound the Ubuntu directory only where `/usr/lib/x86_64-linux-gnu`
  already existed, so on Arch, Fedora and NixOS it bound nothing for the
  library actually bundled since ADR-032.

## Decision

**The build rewrites the directory string in the bundled library, and the
client stages the helpers at the new path before its first web view.**

- `build-appimage.sh` overwrites every whole-string occurrence of
  `/usr/lib/x86_64-linux-gnu/webkitgtk-6.0` (two: the helper directory, and the
  injected-bundle directory beneath it) with
  `/tmp/.stacked-webkit-<first 18 hex of sha256 of the helpers and bundle>`, the
  same 39 bytes, so nothing after it moves. The patcher refuses if the path
  occurs inside a longer string, or not at all, and the build stops. The staged
  path goes to `usr/share/stacked/webkit-helper-dir`.
- `cordial_shell::webkit_helpers::prepare`, called by `webview::open` before
  anything WebKit can spawn, copies `WebKitNetworkProcess`, `WebKitWebProcess`,
  `WebKitGPUProcess` and the injected bundle there.
- Anything that would still make WebKit abort is refused with a reason
  instead: a missing helper, a staging directory that is not a private
  directory owned by this user, a `noexec` `/tmp`, or, where WebKit would use
  bubblewrap, no `/usr/bin/bwrap` or `/usr/bin/xdg-dbus-proxy`.

Why each part is the shape it is:

- **`/tmp`, because the path is fixed at build time** and must be creatable by
  any unprivileged user on any host. Nothing per-user fits in a string compiled
  into a library.
- **Keyed by the helpers' hash**, so two Stacked versions carrying different
  WebKit builds never share a directory, and two carrying the same one share
  identical bytes.
- **Copies, not symlinks**, so the directory does not dangle when the AppImage
  mount that staged it goes away under another instance still running.
- **A directory owned by somebody else is refused, not used.** Executing from
  a directory another user can write would hand them this session. On a shared
  machine this means a second user running the same Stacked version at the same
  time gets a refusal with that reason.
- **WebKit's bubblewrap sandbox needs nothing extra.** The same string feeds its
  `--ro-bind-try PKGLIBEXECDIR PKGLIBEXECDIR`, after its `--tmpfs /tmp`, and it
  binds every directory on `LD_LIBRARY_PATH`, where the helpers' libraries are.

Patching our own bundled LGPL library is not what ADR-001 is about: that
forbids touching the Roblox process's code, and this is a packaging change to a
third-party library on disk. It is a modification of an LGPL work all the same,
so it is recorded in `NOTICE` and in the AppImage's licence directory.

AppRun's namespace is kept for the one thing it can still add: planting the
bundled `bwrap` and `xdg-dbus-proxy` over `/usr/bin` on a host with none. Its
helper-directory binds are gone.

## Alternatives considered

- **Re-exec into a bwrap namespace from Rust**, generalising AppRun. Fails in
  exactly the reported case: no namespaces inside a Flatpak.
- **A `/proc/self/fd/N` path to a directory fd.** Works for the direct spawn,
  but WebKit execs the web process inside its own sandbox, where reaching it
  means leaking a directory fd from outside the sandbox root into it, which is
  an escape.
- **Interposing `g_subprocess_launcher_spawnv`.** Hooking inside the client
  process is the shape ADR-001 rules out, whatever is hooked.

## Consequences

Measured on 2026-10-01, details in the change that introduced this:

- Inside the Roblox manager's Flatpak, the unpatched 0.21.3 tree reproduces the
  abort. The patched library with staged helpers runs `WebKitNetworkProcess`
  and `WebKitWebProcess` from `/tmp/.stacked-webkit-…` and the page reaches
  `load-changed Finished`.
- In a Fedora 44 root (no `/usr/lib/x86_64-linux-gnu`, real `/usr/bin/bwrap`),
  WebKit takes its bubblewrap path and the helpers start from the staged
  directory.

Still broken or unverified:

- **HTTPS in the web view on non-Debian hosts.** The bundled `libgio` looks for
  its TLS module only in `/usr/lib/x86_64-linux-gnu/gio/modules`, and the
  AppImage carries none. In the Fedora root the page failed with "TLS support
  is not available". Inside the manager's GNOME runtime that directory exists,
  so this user is not affected. A separate fix.
- **Inside the manager's Flatpak, WebKit's own web-process sandbox is not
  engaged.** Its `flatpak-spawn --sandbox` probe runs from the client's working
  directory (the profile's `run/`), which a sandboxed instance cannot see, so it
  fails and WebKit spawns the web process directly in the manager's sandbox.
  This was so before this change and is unchanged by it. If the probe ever
  passed, the staged directory and the AppImage's `usr/lib` would need
  `webkit_web_context_add_path_to_sandbox`, which is not done.
- A launch with no `LD_LIBRARY_PATH` pointing at the AppImage's `usr/lib` cannot
  run the staged helpers, which carry no RUNPATH. AppRun and the manager's
  launcher both set it.
- On a host with no bwrap at all, or one older than 0.11 (no `--overlay-src`,
  which includes the bundled 0.9.0), AppRun cannot plant the sandbox tools, and
  the web view is refused with that reason.
