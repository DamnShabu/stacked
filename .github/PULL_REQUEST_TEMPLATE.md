## What this changes

## Linked issues

<!-- "Fixes #123" closes it on merge; "Refs #123" links without closing. -->

## What you measured

Cordial's rule is that claims are worth what they were measured with. This
project has a documented history of confident wrong conclusions drawn from
reading the binary instead of running it — `docs/NEXT.md` keeps the list so
nobody repeats them.

So: what did you run, and what did it print? Paste it. "Should work" and "builds
cleanly" are not measurements.

- [ ] `cargo test --workspace` passes
- [ ] `cargo build --release` is warning-clean for the code I touched
- [ ] the client still launches — repeatedly, not once
- [ ] any document this makes wrong is fixed in the same change (README,
      `docs/`, an ADR, a comment)

## Anything you disproved

If you found that something already written down is wrong — in a comment, in
`docs/NEXT.md`, in an ADR — say so here. Several commits in this repository exist
only to retract an earlier claim, and that is the highest-value thing a change
can carry.

## Breaking changes

<!-- Anything that changes a user's setup: a profile layout, a setting or flag
     renamed or removed, a plugin API change, a dropped platform. "None" is a
     fine answer. -->

## Screenshots

<!-- For a change anyone can see: the window, the settings, a dialog. Delete
     this section otherwise. `cordial_screenshot` reads the frame straight out
     of the swapchain -- see AGENTS.md. -->

## Scope check

- [ ] No in-process hooking, script execution, or memory access is added
- [ ] Nothing here came from a decompiler, and no Roblox code, assets or APK
      contents are included
- [ ] If this adds a plugin capability, it exposes an *effect* and not a channel
      ([ADR-007](../docs/adr/ADR-007-host-resources-are-brokered.md))
- [ ] If this contradicts an ADR, the ADR is updated in the same change rather
      than left stale

## For maintainers testing this

Do not use an account you care about, and put the test account on a different IP
— see [CONTRIBUTING.md](../CONTRIBUTING.md). The risk is collateral rather than
causal, and it is not worth taking with your main.

---

Licensed GPL-3.0-or-later. By contributing you agree your work ships under it.
