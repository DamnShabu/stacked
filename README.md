<p align="center">
  <img src="packaging/icons/hicolor/scalable/apps/io.github.damnshabu.Stacked.svg" alt="" width="112">
</p>

<h1 align="center">Stacked</h1>

<p align="center">
  <strong>Roblox on Linux, from one command.</strong><br>
  Roblox's official Android client, running natively in a desktop window:
  no emulator, no container, no virtual machine, and no launcher in the way.
</p>

<p align="center">
  <a href="https://github.com/DamnShabu/stacked/releases/latest"><img src="https://img.shields.io/github/v/release/DamnShabu/stacked?label=release" alt="Latest release"></a>
  <a href="https://github.com/DamnShabu/stacked/actions/workflows/test.yml"><img src="https://github.com/DamnShabu/stacked/actions/workflows/test.yml/badge.svg" alt="Tests"></a>
  <a href="https://github.com/DamnShabu/stacked/releases"><img src="https://img.shields.io/github/downloads/DamnShabu/stacked/total" alt="Downloads"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/DamnShabu/stacked" alt="Licence: GPL-3.0"></a>
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="docs/usage.md">Usage</a> ·
  <a href="#whats-broken">What's broken</a> ·
  <a href="#faq">FAQ</a> ·
  <a href="https://github.com/DamnShabu/stacked/releases">Releases</a>
</p>

<p align="center">
  <img src="docs/media/cordial-doors.gif"
       alt="Roblox DOORS running natively on Linux: first-person corridor, candle in hand"
       width="600"><br>
  <sub>DOORS, unmodified, on Fedora and GNOME. Recorded on Cordial, the engine
  Stacked is built on.</sub>
</p>

> [!WARNING]
> Stacked is experimental. Roblox doesn't support third-party clients and bans
> accounts for using them, sometimes by mistake. **Don't play on an account you
> care about.**

## Why Stacked

Stacked is built on [Cordial](https://github.com/luohoa97/cordial)'s engine
and shaped around one idea: **you came to play, so the game should be the only
thing you see.**

- **No launcher.** `stacked` starts Roblox and Roblox is the only window. The
  website's Play button opens it directly.
- **It looks after itself.** The first run downloads Roblox, or reuses Sober's
  copy, and checks it against Roblox's signature. Later runs install a newer
  build before the game starts.
- **Smooth on your screen.** The frame rate follows your monitor's refresh
  rate instead of a fixed 60, with GameMode and low-latency presentation on by
  default.
- **Fullscreen that behaves.** The cursor stays on the game, so a second
  monitor can't steal a click.
- **Easy to get help.** `stacked doctor` checks your machine and says how to fix
  what it finds; `stacked logs --zip` packs a redacted report for an issue.
- **Several accounts.** [Headcount](#several-accounts-headcount) runs a set of
  them side by side on top of Stacked.

## Install

Pick the package for your system from the
[latest release](https://github.com/DamnShabu/stacked/releases/latest)
(x86_64 or aarch64):

| System | Command |
|---|---|
| Debian, Ubuntu | `sudo apt install ./stacked_*.deb` |
| Fedora | `sudo dnf install ./stacked-*.rpm` |
| Arch (x86_64) | `sudo pacman -U stacked-*.pkg.tar.zst` |
| Flatpak | `flatpak install --user Stacked-*.flatpak` |
| Anything else | `chmod +x Stacked-*.AppImage` and run it |

Then hook up the browser and check your setup:

```bash
stacked desktop install   # the website's Play button opens Stacked
stacked doctor            # finds problems and says how to fix them
```

From the Flatpak, prefix these with `flatpak run io.github.damnshabu.Stacked`;
from the AppImage, run `./Stacked-*.AppImage desktop install`.

Every file is signed with Sigstore (the `.cosign.bundle` beside it). There is
no package repository yet, so updating Stacked means installing the next
release. Building from source is in [`docs/install.md`](docs/install.md).

**Uninstall:** `stacked desktop remove`, then remove the package. Profiles stay
in `~/.local/share/cordial` until you delete them.

## Play

```bash
stacked
```

Stacked ships no part of Roblox; it fetches the official build for you.
**F11** toggles fullscreen.

| Command | What it does |
|---|---|
| `stacked play --profile alt` | Play as another account. Each profile signs in separately. |
| `stacked config` | List every setting. `config set fps_cap 90` fixes the frame rate. |
| `stacked flags set NAME VALUE` | Set a FastFlag. `flags import --from sober` copies Sober's. |
| `stacked plugins install x.tar.zst` | Add a plugin ([`docs/plugins.md`](docs/plugins.md)). |
| `stacked diagnostics` | What to paste into a bug report. |

The full reference is [`docs/usage.md`](docs/usage.md).

## Several accounts: Headcount

[**Headcount**](https://github.com/DamnShabu/headcount) is the account manager
built on Stacked. Sign each account in once, launch any set of them into the
same server with one click, and leave key-and-click macros running while you
use your desktop. It installs Stacked for you, and its **Update** button
installs the newest release.

Running more than one client at a time is against Roblox's terms, and people
have reported anti-cheat flags for it. Use alts you can afford to lose.

## What's broken

Signing in, playing, keyboard and mouse, text entry, audio, voice chat,
controllers and two accounts side by side all work. These don't yet:

- **No window at all** on COSMIC, KWin and some wlroots compositors
  ([cordial#38](https://github.com/luohoa97/cordial/issues/38)).
- The first touch on a touchscreen crashes it.
- It crashes at launch on some machines, the Steam Deck among them.
- The keyboard is sometimes dead after joining an experience.

Two of Stacked's own features are untested: running above 60 fps on a faster
screen, and cursor confinement on a real multi-monitor desktop. Wayland is the
main target; X11 works but gets less attention. Each bug is tracked in
[`docs/known-issues.md`](docs/known-issues.md).

## FAQ

**Something went wrong.** Run `stacked doctor` first. If Play in the browser
does nothing, run `stacked` in a terminal to see why. "Profile is already
open" means another client has it: close that one or use `--profile`. After a
crash, attach the `.zip` from `stacked logs --zip` to
[an issue](https://github.com/DamnShabu/stacked/issues).

**I use Sober. How do I switch?** Stacked reuses the Roblox build Sober
already downloaded, `stacked flags import --from sober` copies your FastFlags,
and `stacked desktop install` points the Play button at Stacked. You sign in
once more. Sober keeps working alongside it.

**I used Cordial. What carries over?** Everything. Profiles, sign-ins, flags
and settings live in the same `~/.local/share/cordial` and `~/.config/cordial`.
Both can be installed at once; a profile open in one is locked in the other.

**Is it faster than Sober?** Nobody has measured them side by side. Stacked
uses the same defaults Sober does and targets your screen's refresh rate
rather than 60.

**Is it a cheat or mod injector?** No. There is no script execution, hooking
or access to the game's memory, and the code for it is absent rather than
disabled ([ADR-001](docs/adr/ADR-001-in-process-hooking.md)). Plugins extend
Stacked, not Roblox.

**How does it work?** A ported Android linker loads Roblox's unmodified
`libroblox.so`, `libjnivm` stands in for Android's Java runtime, and a
framework layer answers the platform calls the engine makes
([`docs/architecture.md`](docs/architecture.md)).

## Contributing

Start with [`CONTRIBUTING.md`](CONTRIBUTING.md); the reasons behind each
design decision are in [`docs/adr/`](docs/adr). For help, see
[`SUPPORT.md`](.github/SUPPORT.md); to report a vulnerability,
[`SECURITY.md`](SECURITY.md). Everyone follows the
[Code of Conduct](CODE_OF_CONDUCT.md).

If Stacked got you playing, a star helps other Linux players find it.

## Credits and licence

Stacked is GPL-3.0-or-later ([`LICENSE`](LICENSE)); third-party components
keep their own licences ([`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md)).

- **[Cordial](https://github.com/luohoa97/cordial)**, by luohoa97 and
  contributors. Almost everything that makes Roblox run here is theirs.
- **Sober** proved a native Roblox client on Linux was possible. Its public
  issue tracker is a research source here; its code was never read.
- **mocktail** (Apache-2.0) is credited where its designs were adapted.

Stacked is not affiliated with or endorsed by Roblox Corporation.
