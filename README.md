<p align="center">
  <img src="packaging/icons/hicolor/scalable/apps/io.github.damnshabu.Stacked.svg" alt="" width="96">
</p>

<h1 align="center">Stacked</h1>

<p align="center">
  Roblox's Android build, running natively on Linux. Launched from the terminal.<br>
  No emulator, no container, no virtual machine. GPL-3.0-or-later.
</p>

Stacked is a fork of [Cordial](https://github.com/luohoa97/cordial). The
engine underneath is Cordial's. The fork changes four things:

- **A command-line launcher.** There are no launcher, settings or chooser
  windows. `stacked` starts the game, and the game's window is the only
  window.
- **The cursor stays on the window in fullscreen**, including in menus, so a
  second monitor can't take a click meant for the game.
- **A frame-rate target** you can set, with GameMode and low-latency MAILBOX
  presentation on by default.
- **Its own look.** The game window's title bar uses the Stacked palette,
  or your desktop's theme if you prefer.

It is experimental. Roblox does not support third-party clients and bans
accounts for using them, in waves, including by mistake. **Don't use an
account you care about.**

## Install

There are no prebuilt packages for Stacked yet, so build it from source. You
need Rust, Clang (bionic won't build with GCC), and the GTK4 >= 4.12 and
libadwaita >= 1.5 development packages. PipeWire and WebKitGTK-6.0 headers are
optional.

```bash
# Debian/Ubuntu:  sudo apt install clang cmake libgtk-4-dev libadwaita-1-dev libpipewire-0.3-dev
# Fedora:         sudo dnf install clang cmake gtk4-devel libadwaita-devel pipewire-devel
git clone --recursive https://github.com/DamnShabu/stacked
cd stacked
cargo build --release
install -Dm755 target/release/stacked target/release/cordial-run -t ~/.local/bin/
```

`stacked` and `cordial-run` must sit in the same directory, or both must be on
your `PATH`. To make browser Play buttons open Stacked, install the desktop
entry and icon too:

```bash
install -Dm644 packaging/io.github.damnshabu.Stacked.desktop -t ~/.local/share/applications/
install -Dm644 packaging/icons/hicolor/scalable/apps/io.github.damnshabu.Stacked.svg \
  -t ~/.local/share/icons/hicolor/scalable/apps/
update-desktop-database ~/.local/share/applications
```

## Use

```bash
stacked install        # get Roblox: uses Sober's copy if it has one, else downloads and checks it
stacked                # play
```

You supply Roblox yourself. Stacked ships none of it. `stacked install` looks
for a copy [Sober](https://sober.vinegarhq.org/) has already downloaded, and
otherwise fetches one and refuses it unless it is signed with Roblox's own
certificate.

| | |
|---|---|
| `stacked play --profile alt` | Play on another profile. Each profile is a separate sign-in. |
| `stacked profiles new alt` | Make a profile. `profiles use alt` makes it the default. |
| `stacked "roblox-player:…"` | Join a link. This is what the browser runs. |
| `stacked config` | List every setting with its current value. |
| `stacked update` | Download the newest Roblox build. |
| `stacked plugins install x.tar.zst` | Install a plugin. One that runs code starts switched off, and nothing is granted until you grant it. |
| `stacked diagnostics` | Print what a bug report needs. |

F11 toggles fullscreen. [`docs/usage.md`](docs/usage.md) lists every command
and setting.

## Settings worth knowing

```bash
stacked config set fps_cap 144              # frame-rate target (unset = the engine's own)
stacked config set theme system             # desktop colours instead of Stacked's
stacked config set fullscreen_confine false # let the cursor leave a fullscreen window
stacked config set present_mode fifo        # vsync: less power, more latency
```

FastFlags go in the file `stacked flags path` prints
([`docs/fastflags.md`](docs/fastflags.md)).

## Status

What works: signing in, loading and playing experiences, keyboard and mouse,
camera, text entry, audio, voice chat, controllers, fullscreen, and two
accounts side by side.

**What is still broken.** These bugs were reported against Cordial. Where
each stands in Stacked is in [`docs/known-issues.md`](docs/known-issues.md).

- No window at all on COSMIC, KWin, and some wlroots compositors.
- The first touch on a touchscreen crashes.
- A SIGSEGV at launch on some machines, including the Steam Deck.
- The keyboard is sometimes dead after joining an experience.
- Fullscreen freezes, hangs on exit, and X11 keys that stay held have fixes in
  the code that nobody has confirmed on an affected machine yet.

The fork's own new pieces have limits too:

- **Fullscreen cursor confinement** has been checked against a real
  compositor (sway) but not yet on a real multi-monitor desktop, and not at all
  on X11.
- **Raising the frame rate** with `fps_cap` is untested. The flag is known to
  *lower* the rate, but not whether it raises it past the engine's default.
- **Frame rate in general** hasn't been re-measured. The records are in
  [`docs/status.md`](docs/status.md).

Wayland is the main target. X11 works but gets less attention.

## FAQ

**Is this faster than Sober?** It hasn't been measured side by side. Stacked
does what Sober does by default: it asks Feral GameMode for the performance
governor, presents with MAILBOX rather than FIFO, and doesn't keep a launcher
process running while you play. Beyond that, the only lever is `fps_cap`.

**Where did the settings window go?** It's now `stacked config`, and the
settings file is the same `~/.config/cordial/shell.json` as before. Profiles,
sign-ins and FastFlags carry over from Cordial unchanged, because the data
paths haven't moved.

**Can I run it beside Cordial?** A source build in `~/.local/bin` can sit
beside Cordial's Flatpak. It has its own application id, desktop entry and
icon. Native packages can't be installed together, because both ship
`cordial-run`. The two share profiles, and a profile open in one is locked
against the other.

**Is it a cheat or a mod injector?** No. There is no script execution, no
hooking and no access to the Roblox process's memory. Those capabilities are
absent from the code, not just disabled
([ADR-001](docs/adr/ADR-001-in-process-hooking.md)). Plugins extend Stacked,
not Roblox.

**How does it work?** A ported AOSP bionic linker loads Roblox's unmodified
`libroblox.so`. `libjnivm` stands in for Android's Java runtime, and a
framework layer answers the platform calls the engine makes. See
[`docs/architecture.md`](docs/architecture.md).

## Contributing

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) and
[`AGENTS.md`](AGENTS.md). Decisions and their reasons are in
[`docs/adr/`](docs/adr). The launcher's move to a CLI is
[ADR-043](docs/adr/ADR-043-the-launcher-is-a-command-line.md). Report bugs
[here](https://github.com/DamnShabu/stacked/issues) with the output of
`stacked diagnostics`.

## Credits and licence

GPL-3.0-or-later ([`LICENSE`](LICENSE)). Third-party components keep their own
licences ([`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md)).

- **Cordial**, by luohoa97 and contributors, is the project this is forked
  from. Almost everything that makes Roblox run here is theirs.
- **Sober** showed that a native Roblox client on Linux was possible. Its
  public issue tracker is a research corpus here. Its code was never read; it
  is not source-available.
- **mocktail** (Apache-2.0) is credited where its designs were adapted.

Not affiliated with, endorsed by or approved by Roblox Corporation.
