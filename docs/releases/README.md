# Cutting a release

One file per release in this directory, addressed to somebody who has just
installed that version — what they can now do, and what will still bite them.
See any recent one for the shape; `AGENTS.md` has the rules that govern the
prose, including the one that matters most: **release notes say what is
broken**, about a third of the way down.

This page is the other half — the mechanical part, which until v0.15.1 was
written down nowhere.

## The version lives in six files, and CI only catches two of them

**v0.15.1 was tagged with four of the six updated, and the tag went red.**
`Publish Arch packaging` refused it: `packaging/aur/cordial/PKGBUILD` still said
`pkgver=0.15.0`, and its `source=` pins `#tag=v$pkgver`, so the packaging
repository would have carried the *previous* release under the new version's
name. That check earned its keep; nothing else would have noticed.

| File | Why it matters |
|---|---|
| `Cargo.toml` (`[workspace.package] version`) | The one true version. `crates/cordial-shell/src/version.rs` reads it, the window title shows it, `cordial-update` sends it in the User-Agent. A CI gate refuses a tag that disagrees with it. |
| `Cargo.lock` | Holds all five workspace crates' versions. `cargo update --workspace` rewrites them; `cargo metadata` alone does **not**. |
| `docs/releases/vX.Y.Z.md` | The notes. |
| `packaging/aur/stacked/PKGBUILD` (`pkgver`) | Its `source=` pins `#tag=v$pkgver`, so a stale value publishes the wrong release. |
| `packaging/aur/stacked/.SRCINFO` (`pkgver` **and** the `source =` line) | Hand-maintained deliberately — there is no `makepkg` on the development host. Two lines, not one. See [PUBLISHING.md](../../packaging/aur/PUBLISHING.md). |
| `packaging/io.github.damnshabu.Stacked.metainfo.xml` | AppStream shows the newest `<release>` as "what's new", so the top entry is the one that is read. |

`CHANGELOG.md` is **not** in this list. It stops at 0.6.0 by design; everything
from 0.7.0 onward lives here instead.

## Two traps worth knowing before you tag

**`release.yml` has a `paths` filter, and a tag can miss it entirely.** It
builds on `crates/**`, `native/**`, `third_party/**`, `Cargo.lock` and
`Cargo.toml`. A tag placed on a commit already on `main` pushes no new commits,
so nothing matches and no build runs — which left v0.11.0 tagged twice with
nothing built. A release commit that bumps `Cargo.toml` and `Cargo.lock` clears
the filter by construction; a tag on an older commit does not.

**Publishing is tag-only.** apt, dnf and pacman publish only when the triggering
run's `head_branch` starts with `v`. A push to `main` builds and is then
deliberately *skipped* by all three publishers — that is the gate working, not a
failure. The Flatpak remote is the exception and still follows `main`, because
its Pages environment carries a deployment branch policy naming `main`; moving
it needs a `v*` policy added there first.

**A pushed tag is not a release, and neither is a release page.** Stacked's
first two tags, v0.21.0 and v0.21.1, reached GitHub and started no workflow run
at all -- `gh run list` has no entry for either, and why was never
established. A "v0.21.0" release was then created by hand in the web UI. It
had no files, and it was marked Latest, which is the release Update Stacked
reads the AppImage from. So for a day the fork had two tags and a release and
nothing anybody could install. **Never create the release by hand**:
`attach-to-release.yml` creates it from the notes when the first build
finishes, and uploads to it. A release only counts once it has all nine packages
on it: `release.yml` attaches seven (AppImage, .deb and .rpm for x86_64 and
aarch64, plus the Arch package) and `flatpak.yml` attaches two Flatpak bundles.
Each one has a `.cosign.bundle` beside it.

**If a tag started nothing, bump to the next patch version instead of moving
the tag.** That is how v0.21.2 was made. Moving a tag that is already on GitHub
needs a force-push that deletes it and pushes it again. A fresh version needs
neither, and its commit touches `Cargo.toml` and `Cargo.lock`, so it clears the
`paths` filter above.

## The order that works

1. Bump the six files; write the notes.
2. Commit, then **check the commit touches what you expect and nothing from a
   submodule**.
3. Tag and push the commit and the tag.
4. **Within a minute, check that the tag started something:**
   `gh run list --branch vX.Y.Z` should list `Native packages`, `Flatpak` and
   `Publish Arch packaging`. An empty list means go to the next patch version,
   as above.
5. Watch them finish. The three publishers should run for the tag and skip for
   `main`.
6. **Check that the files are on the release:** `gh release view vX.Y.Z` should
   show nine packages and nine `.cosign.bundle` files. A release with some
   packages missing is caught by the attach job, but a release with no files
   at all can only be caught here.
7. Avoid pushing again to `main` while those are in flight — each push
   supersedes the previous run through its concurrency group, and a runs list
   full of `cancelled` reads exactly like breakage.

If `Publish Arch packaging` does refuse a tag after the fact, fix `pkgver` on
`main` and re-run it with `workflow_dispatch` and the tag as its input: it
checks out the ref it is dispatched on, so a corrected `main` satisfies it
without moving the tag. Moving a published tag is not the answer — workflows are
usually still building against it.
