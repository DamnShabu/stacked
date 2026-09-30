# ADR-044: Roblox updates happen when somebody presses Play

**Status:** Accepted
**Date:** 2026-09-30
**Supersedes:** [ADR-043](ADR-043-the-launcher-is-a-command-line.md), the "Background updates" item only
**Related:** [ADR-025](ADR-025-fetching-from-a-third-party-mirror.md), [ADR-033](ADR-033-roblox-versions-are-a-keyed-store.md)

## Context

ADR-043 removed the resident launcher, and with it the timer that fetched a
new Roblox build in the background. `stacked update` became the only way to
get one.

Roblox refuses clients below a minimum version, and moves that minimum often.
A build that joined last week can be turned away this week. Under ADR-043 the
user finds that out at the join screen and has to know which command fixes it.
Sober updates without being asked. So did Cordial's launcher.

A first run had the same shape. `stacked` with no build printed "`stacked
install` downloads one" and stopped, which from the desktop entry is a
notification and a Play button that did nothing.

## Decision

`stacked play` does both before it starts the client, and `auto_update` in
`shell.json` (default `true`) or `--no-update` on the command line turns both
off.

- **No build:** install one, as `stacked install` would, then play.
- **A build Stacked manages, or Sober's:** ask each networked source for its
  newest version and, if it is newer than the installed engine, install it,
  then play.

The rules that keep this from making a launch worse are in
`crates/cordial-shell/src/auto_update.rs`:

- **A check never stops a launch that has a build.** No answer, a failed
  download, or a metered connection is one line of output, and the build on
  disk starts.
- **The check is bounded.** Five seconds, on its own thread.
- **The check is memoised.** A check that found nothing newer is trusted for
  ten minutes, keyed on the installed version, so relaunching after a crash
  costs nothing.
- **It is never asked when it cannot change anything.** A chosen APK or
  `CORDIAL_APK` is not Stacked's to replace, and a pinned profile does not run
  the current build. Neither makes a request.
- **Only networked sources are asked.** The local source answers by
  decompressing the engine inside Sober's APK. The installed build's version
  comes from `cordial_update::engine::installed_version`, which the client
  already memoises beside the engine on every start.
- **A metered connection holds the download back**, the first-run install
  included (`stacked install` fetches it explicitly), when NetworkManager says,
  or guesses, that it is metered. An unknown answer does not, which is
  narrower than `cordial_update::metered::is_metered`. That reading was for a
  download nobody asked for. This one follows a press of Play, says how big
  it is, and stops on Ctrl-C, and treating "no NetworkManager" as metered
  would mean systemd-networkd and iwd machines never update at all.
- **From the desktop, it is announced.** With no terminal, the first-run
  download and an update are also sent as a desktop notification.

## Consequences

- A launch with a newer build offered costs the download before the game
  opens, as it does in Sober.
- A launch with nothing new costs one small request, at most every ten
  minutes, or nothing when a chosen APK or a pin means there is nothing to do.
- **Not measured end to end.** The decision logic and the deadline are tested.
  The download itself is `provider::obtain_and_install`, the same call
  `stacked update` makes. The mirror was unreachable from the environment
  this was written in, so a real update at launch has not been observed.
