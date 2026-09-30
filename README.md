<p align="center">
  <img src="packaging/icons/hicolor/scalable/apps/io.github.damnshabu.Stacked.svg" alt="" width="96">
</p>

<h1 align="center">Stacked</h1>

<p align="center">
  Roblox's Android build, running natively on Linux.<br>
  No emulator, no container, no virtual machine. GPL-3.0-or-later.
</p>

Stacked is a fork of [Cordial](https://github.com/luohoa97/cordial), and the
engine underneath is Cordial's. What the fork changes:

- **No launcher windows.** `stacked` is a command. It starts the game, and the
  game's window is the only window. A browser's Play button still works.
- **The cursor stays on the game's screen in fullscreen**, even in menus, so a
  second monitor can't take a click meant for the game. It still moves freely.
- **Frame rate follows your screen**, not the engine's built-in 60, with
  GameMode and low-latency MAILBOX presentation on by default.
- **Its own look**, or your desktop's if you prefer.

It is experimental. Roblox doesn't support third-party clients and bans
accounts for using them, in waves, including by mistake. **Don't use an
account you care about.**

## Install

There are no prebuilt packages yet, so build it from source. You need
[Rust](https://rustup.rs) 1.75 or newer, Clang (bionic won't build with GCC),
CMake, and GTK >= 4.12 with libadwaita >= 1.5. That means Ubuntu 24.04,
Debian 13, Fedora 40 or newer, or Arch.

```bash
# Ubuntu/Debian: sudo apt install clang cmake pkg-config libgtk-4-dev libadwaita-1-dev libpipewire-0.3-dev
# Fedora:        sudo dnf install clang cmake gtk4-devel libadwaita-devel pipewire-devel
# Arch:          sudo pacman -S clang cmake gtk4 libadwaita pipewire
git clone --recursive https://github.com/DamnShabu/stacked
cd stacked
cargo build --release
install -Dm755 target/release/stacked target/release/cordial-run -t ~/.local/bin/
```

PipeWire's headers are optional, but without them there is no sound.
`stacked` and `cordial-run` have to stay in the same directory.

Then add Stacked to your app menu and make it what the website's Play button
opens, and check the machine for anything that would stop it running:

```bash
~/.local/bin/stacked desktop install
~/.local/bin/stacked doctor
```

Tab completion: `stacked completions bash`, `zsh` or `fish` prints the script
for your shell ([`docs/usage.md`](docs/usage.md#tab-completion) says where to
put it).

**To update Stacked**, run `git pull --recurse-submodules` and repeat the
`cargo build` and first `install` lines.

**To remove it**, run `stacked desktop remove` and delete the two binaries.
Profiles stay in `~/.local/share/cordial` until you delete them, and
`stacked profiles remove NAME` also clears that profile's sign-in from the
keyring.

## Use

```bash
stacked            # play
```

The first time, that downloads Roblox, using Sober's copy if you have one, and
checks it against Roblox's own signature. After that, each launch installs a
newer build first if there is one, because Roblox turns old clients away.
Stacked ships no part of Roblox.

| | |
|---|---|
| `stacked play --profile alt` | Play signed in as someone else. Each profile is its own sign-in. |
| `stacked profiles new alt` | Make a profile. `profiles use alt` makes it the default, and `profiles remove alt` deletes it. |
| `stacked "roblox-player:…"` | Join a link. This is what the browser runs. |
| `stacked update` | Download the newest Roblox build now. `stacked` also does this by itself when you press Play. |
| `stacked flags set NAME VALUE` | Set a FastFlag. `stacked flags import --sober` copies Sober's, and `import file.json` takes a Bloxstrap export. |
| `stacked config` | List every setting and its current value. |
| `stacked plugins install x.tar.zst` | Add a plugin. One that runs code starts switched off with no permissions. |
| `stacked doctor` | Check this machine for problems, with the fix for each. |
| `stacked diagnostics` | Print what a bug report needs. |

**F11** toggles fullscreen. **Ctrl-C** in the terminal closes the game. Every command and setting is in [`docs/usage.md`](docs/usage.md).

### Settings worth knowing

```bash
stacked config set fps_cap 90               # fixed frame-rate target (unset = your screen's refresh rate, 0 = the engine's 60)
stacked config set present_mode fifo        # vsync: uses less power, adds latency
stacked config set fullscreen_confine false # let the cursor leave a fullscreen window
stacked config set theme system             # your desktop's colours instead of Stacked's
```

FastFlags: `stacked flags set NAME VALUE`, checked against the flag's type;
see [`docs/fastflags.md`](docs/fastflags.md).

## Troubleshooting

Start with `stacked doctor`. It checks the display, Vulkan, sound, the
keyring, the browser handler and the Roblox build, and says what to run about
anything it finds.

- **Pressing Play in the browser does nothing.** Run `stacked` in a terminal
  to see why. When it's started from the browser, Stacked also sends the
  reason as a desktop notification.
- **"profile is already open"** means another client is using that profile.
  Close it, or play on a second profile with `--profile`.
- **The cursor won't leave the window.** Alt-tab, or leave fullscreen with F11.
  To turn confinement off, run `stacked config set fullscreen_confine false`.
- **No window appears at all** on COSMIC, KWin or some wlroots compositors.
  This is a known bug with no fix yet
  ([cordial#38](https://github.com/luohoa97/cordial/issues/38)).
- **It closed on its own.** The output above the prompt is what it printed.
  Include it, and `stacked diagnostics`, in a bug report.

## Status

What works: signing in, loading and playing experiences, keyboard and mouse,
camera, text entry, audio, voice chat, controllers, fullscreen, and two
accounts side by side.

**What doesn't yet.** These were reported against Cordial, and apply here too:

- No window on COSMIC, KWin or some wlroots compositors.
- The first touch on a touchscreen crashes.
- A crash at launch on some machines, including the Steam Deck.
- The keyboard is sometimes dead after joining an experience.

Fixes for the fullscreen freeze, the hang on exit, X11 keys staying held, and
cursor confinement on Hyprland and KWin are in the code. Nobody has confirmed
them on an affected machine yet. [`docs/known-issues.md`](docs/known-issues.md)
has the state of each bug and what to run if you can help.

Two of the fork's own features are untested:

- Running above 60 fps on a faster screen. The frame-rate flag Stacked sets is
  known to *lower* the rate; raising it hasn't been measured.
- Cursor confinement on a real multi-monitor desktop. So far it has only run
  in a nested compositor.

Wayland is the main target. X11 works but gets less attention.

## FAQ

**Is it faster than Sober?** It hasn't been measured side by side. Stacked uses
the same defaults Sober does: GameMode for the performance governor, MAILBOX
rather than FIFO presentation, and no launcher left running while you play.
Beyond that, it targets your screen's refresh rate rather than 60.

**I use Sober. How do I switch?** Stacked starts from the Roblox build Sober
already downloaded, and only downloads one when a newer build is out.
`stacked flags import --sober` copies your FastFlags, and
`stacked desktop install` makes the website's Play button open Stacked
instead. You sign in again, once per
profile. Sober keeps working alongside it.

**I used Cordial. What carries over?** Everything: profiles, sign-ins,
FastFlags, and settings. Stacked reads the same `~/.local/share/cordial` and
`~/.config/cordial` paths. Cordial's Settings window is `stacked config` here.

**Can I keep Cordial installed too?** A source build in `~/.local/bin` can sit
beside Cordial's Flatpak, since the two have different application ids.
Native packages of the two can't be installed together. Both use the same
profiles, and a profile open in one is locked against the other.

**Is it a cheat or a mod injector?** No. There is no script execution, no
hooking and no access to the Roblox process's memory. That code is absent, not
just disabled ([ADR-001](docs/adr/ADR-001-in-process-hooking.md)). Plugins
extend Stacked, not Roblox.

**How does it work?** A ported AOSP bionic linker loads Roblox's unmodified
`libroblox.so`. `libjnivm` stands in for Android's Java runtime, and a
framework layer answers the platform calls the engine makes. See
[`docs/architecture.md`](docs/architecture.md).

## Contributing

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`AGENTS.md`](AGENTS.md).
Decisions and the reasons behind them are in [`docs/adr/`](docs/adr). Report
bugs [here](https://github.com/DamnShabu/stacked/issues) with the output of
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
