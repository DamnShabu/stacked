<p align="center">
  <img src="packaging/icons/hicolor/scalable/apps/io.github.damnshabu.Stacked.svg" alt="" width="96">
</p>

<h1 align="center">Stacked</h1>

<p align="center">
  Roblox's Android build, running natively on Linux.<br>
  No emulator, no container, no virtual machine.
</p>

<p align="center">
  <a href="https://github.com/DamnShabu/stacked/releases/latest"><img src="https://img.shields.io/github/v/release/DamnShabu/stacked" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/DamnShabu/stacked" alt="Licence: GPL-3.0"></a>
</p>

<p align="center">
  <img src="docs/media/cordial-doors.gif"
       alt="Roblox DOORS running natively on Linux: first-person corridor, candle in hand"
       width="560"><br>
  <em>DOORS, unmodified, on Fedora and GNOME. Recorded on Cordial, the engine
  Stacked is built on, before the fork was renamed.</em>
</p>

Stacked loads Roblox's official Android client straight into a Linux process
and gives it a normal desktop window. It is a fork of
[Cordial](https://github.com/luohoa97/cordial), whose engine does the heavy
lifting. The fork adds:

- **No launcher.** `stacked` starts the game and the game is the only window.
  The website's Play button still works.
- **Frame rate that follows your screen** instead of a fixed 60, with GameMode
  and low-latency presentation on by default.
- **A cursor that stays put in fullscreen**, so a second monitor can't steal a
  click meant for the game.

> [!WARNING]
> Stacked is experimental. Roblox doesn't support third-party clients and bans
> accounts for using them, sometimes by mistake. **Don't play on an account you
> care about.**

## Install

Download a package from the
[latest release](https://github.com/DamnShabu/stacked/releases/latest), for
x86_64 or aarch64:

| | |
|---|---|
| Debian, Ubuntu | `.deb` — `sudo apt install ./stacked_*.deb` |
| Fedora | `.rpm` — `sudo dnf install ./stacked-*.rpm` |
| Arch (x86_64) | `.pkg.tar.zst` — `sudo pacman -U stacked-*.pkg.tar.zst` |
| Flatpak | `.flatpak` — `flatpak install --user Stacked-*.flatpak` |
| Anything else | `.AppImage` — `chmod +x` it and run it |

Then:

```bash
stacked desktop install   # the browser's Play button opens Stacked
stacked doctor            # checks the machine and says how to fix anything it finds
```

From the Flatpak, run these as `flatpak run io.github.damnshabu.Stacked`;
from the AppImage, as `./Stacked-*.AppImage desktop install`. Every
file is signed with Sigstore; its `.cosign.bundle` sits next to it. There is
no package repository yet, so updating means downloading the next release.
[`docs/install.md`](docs/install.md) covers building from source.

**Remove:** `stacked desktop remove`, then uninstall the package. Profiles
stay in `~/.local/share/cordial` until you delete them.

## Use

```bash
stacked
```

The first run downloads Roblox — or reuses Sober's copy if you have one — and
checks it against Roblox's signature. Stacked ships no part of Roblox. Later
launches install a newer build first when there is one.

| | |
|---|---|
| `stacked play --profile alt` | Play as another account. Each profile is its own sign-in. |
| `stacked flags set NAME VALUE` | Set a FastFlag. `flags import --sober` copies Sober's. |
| `stacked config` | List every setting. `config set fps_cap 90` fixes the frame rate. |
| `stacked plugins install x.tar.zst` | Add a plugin ([`docs/plugins.md`](docs/plugins.md)). |
| `stacked diagnostics` | What to paste into a bug report. |

**F11** toggles fullscreen. Everything else is in [`docs/usage.md`](docs/usage.md).

## What's broken

Signing in, playing, keyboard and mouse, text entry, audio, voice chat,
controllers and two accounts side by side all work. These don't yet:

- **No window at all** on COSMIC, KWin and some wlroots compositors
  ([cordial#38](https://github.com/luohoa97/cordial/issues/38)).
- The first touch on a touchscreen crashes it.
- It crashes at launch on some machines, the Steam Deck among them.
- The keyboard is sometimes dead after joining an experience.

Two of the fork's own features are untested: running above 60 fps on a faster
screen, and cursor confinement on a real multi-monitor desktop. Wayland is the
main target; X11 works but gets less attention.
[`docs/known-issues.md`](docs/known-issues.md) tracks each bug.

## FAQ

**Something went wrong.** Run `stacked doctor` first. If Play in the browser
does nothing, run `stacked` in a terminal to see why. "Profile is already
open" means another client has it — close that one or use `--profile`. For a
crash, attach the terminal output, `stacked logs` and `stacked diagnostics` to
[an issue](https://github.com/DamnShabu/stacked/issues).

**Is it faster than Sober?** Nobody has measured them side by side. It uses the
same defaults Sober does, and targets your screen's refresh rate rather than 60.

**I use Sober. How do I switch?** Stacked reuses the Roblox build Sober already
downloaded, `stacked flags import --sober` copies your FastFlags, and
`stacked desktop install` points the Play button at Stacked. You sign in again
once. Sober keeps working alongside it.

**I used Cordial. What carries over?** Everything — profiles, sign-ins, flags
and settings live in the same `~/.local/share/cordial` and `~/.config/cordial`.
Both can be installed at once; a profile open in one is locked in the other.

**Is it a cheat or mod injector?** No. There is no script execution, hooking or
access to the game's memory, and the code for it is absent rather than disabled
([ADR-001](docs/adr/ADR-001-in-process-hooking.md)). Plugins extend Stacked,
not Roblox.

**How does it work?** A ported Android linker loads Roblox's unmodified
`libroblox.so`, `libjnivm` stands in for Android's Java runtime, and a
framework layer answers the platform calls the engine makes
([`docs/architecture.md`](docs/architecture.md)).

## Contributing

Read [`CONTRIBUTING.md`](CONTRIBUTING.md). Decisions and their reasons are in
[`docs/adr/`](docs/adr). Help: [`SUPPORT.md`](.github/SUPPORT.md). Security:
[`SECURITY.md`](SECURITY.md). Everyone follows the
[Code of Conduct](CODE_OF_CONDUCT.md).

## Credits and licence

GPL-3.0-or-later ([`LICENSE`](LICENSE)); third-party components keep their own
([`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md)).

- **Cordial**, by luohoa97 and contributors — almost everything that makes
  Roblox run here is theirs.
- **Sober** proved a native Roblox client on Linux was possible. Its public
  issue tracker is a research source here; its code was never read.
- **mocktail** (Apache-2.0) is credited where its designs were adapted.

Not affiliated with or endorsed by Roblox Corporation.
