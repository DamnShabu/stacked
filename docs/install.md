# Installing Stacked

The README has the quickstart commands. This is the detail behind them: what
you need, where Roblox comes from, what building takes, and the honest state
of packaging.

## What you need

- x86-64 Linux
- A Wayland session. X11 still starts, but
  [ADR-011](adr/ADR-011-wayland-and-libadwaita.md) makes Wayland the
  backend this targets and says X11 is not developed further
- Roblox's official Android client, which **you supply** — Stacked ships no
  Roblox code, APK or assets and never will

From an installed APK you need the `lib/x86_64/` objects and the base APK.

**The shortest route to one is `stacked install`**, which fetches and verifies
a build from the terminal. Installing Sober first also still works.

[Sober](https://sober.vinegarhq.org/) downloads Roblox's Android build for its
own use, and Stacked looks for it there —
`~/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/`. Nothing is copied
and nothing is modified; Stacked reads the APK where it already is. If you have
Sober, `stacked install` finds its build and downloads nothing. You are free to
keep using Sober afterwards, or not.

If you have an APK of your own, `stacked config set roblox.apk PATH` points at
it, and `CORDIAL_APK=PATH` does the same for one launch. On a split build the
engine is in `split_config.x86_64.apk` rather than `base.apk`; Stacked checks
the siblings itself and says which it tried when it cannot find one.

## There are no Stacked packages yet

**Building from source is the only way to install Stacked today.** There is no
Flatpak remote, no AppImage, no `.deb`, `.rpm` or Arch package, no APT, dnf or
pacman repository and no AUR package for Stacked. The scripts that would build
them are in [`packaging/`](../packaging) and carry Stacked's name and
application id, `io.github.damnshabu.Stacked`, but nothing has been published
from them.

The Flatpak remote, repositories, release artefacts and cosign signatures you
may find documented elsewhere are upstream Cordial's. They install Cordial,
not Stacked, and nothing about their signing or trust carries over to this
fork.

Stacked is not on Flathub, and under Flathub's current generative-AI policy it
could not be: most of its code, Cordial's included, was written with AI
assistance, and the git history records that in `Co-Authored-By` trailers.

## Building from source

Building needs:

- **Rust** (the workspace declares 1.75 as its minimum) and **CMake**, which
  the `cmake` crate drives to build the native tree
- **Clang** — AOSP bionic uses C11 `_Atomic` inside C++ headers and GCC rejects it
- **GTK4 (≥ 4.12) and libadwaita (≥ 1.5)** development packages. The launcher
  opens no window, but the game window `cordial-run` opens is
  `AdwApplicationWindow` end to end
  ([ADR-011](adr/ADR-011-wayland-and-libadwaita.md)), and `gtk4-sys`/
  `libadwaita-sys` link against the system libraries via `pkg-config` at build
  time. Those floors are the feature pins in `crates/cordial-shell/Cargo.toml`
- **PipeWire's development headers** (`pipewire-devel` / `libpipewire-0.3-dev`),
  optional — for OpenSL ES audio. `native/CMakeLists.txt` detects them via
  `pkg-config` and compiles the real audio backend if found, or the previous
  link-only stub (no sound, but everything else works) if not. Either way
  `libpipewire-0.3.so` itself is `dlopen`'d at run time, never linked, so a
  build made with the headers still runs — audio-less — on a machine that
  only has the runtime library, or neither.
- **WebKitGTK 6.0's development headers** (`webkitgtk6.0-devel` /
  `libwebkitgtk-6.0-dev`), optional — for the in-game web views such as the
  Marketplace. They are only used when you ask for them with
  `--features cordial-shell/webview,cordial-runtime/webview`.

```bash
# Fedora:         sudo dnf install clang cmake gtk4-devel libadwaita-devel pipewire-devel
# Debian/Ubuntu:  sudo apt install clang cmake pkg-config libgtk-4-dev libadwaita-1-dev libpipewire-0.3-dev
# Arch:           sudo pacman -S clang cmake gtk4 libadwaita pipewire
git clone --recursive https://github.com/DamnShabu/stacked
cd stacked
cargo build --release
install -Dm755 target/release/stacked target/release/cordial-run -t ~/.local/bin/
```

The submodules are required. `stacked` and `cordial-run` must sit in the same
directory, or both be on your `PATH`. Then `stacked desktop install` adds the
desktop entry, which is what makes a browser's Play button open Stacked, and
`stacked doctor` checks the machine for anything that would stop it running.

To build the Flatpak yourself instead:

```bash
packaging/build-flatpak.sh --install
```

That one needs no submodules: the manifest pins `third_party/libjnivm` and
`third_party/mcpelauncher-linker` by commit and fetches them itself, and it
pins every crate by the sha256 already in `Cargo.lock`
(`packaging/cargo-sources.json`). flatpak-builder downloads the lot up front;
the compile itself runs with the network unshared, so what comes out is
reproducible ([cordial#3](https://github.com/luohoa97/cordial/issues/3)). If you
change a dependency, run `python3 packaging/cargo-sources.py` in the same
commit as the `Cargo.lock` change or the Flatpak build will fail with
`no matching package`. Inside the Flatpak the launcher is still a terminal
program: run it as `flatpak run io.github.damnshabu.Stacked`, with the same
subcommands.
