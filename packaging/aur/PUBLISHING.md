# Publishing to the AUR

Three packages live under `packaging/aur/`, none ever published. That is the
single cheapest piece of reach Stacked is leaving on the table: Arch users are a
large share of the people who run Roblox on Linux at all, and a package nobody
has pushed reaches none of them.

- **`stacked-git/`** builds whatever commit is at the tip of `main`.
- **`stacked/`** builds a tagged release, and is also what
  `.github/workflows/release.yml` runs on every tag to produce the `.pkg.tar.zst`
  attached to the GitHub release — so by the time you read this, CI has already
  built it successfully at least once. That is a real test of this exact file,
  not a stand-in for one; it is not the same as this file having been pushed
  to the AUR, which still needs the steps below.
- **`cordial-bin/`** is still Cordial's, not Stacked's. It repackages a
  `.pkg.tar.zst` from a GitHub release, unchanged, pinned by sha256 -- and the
  one it pins is Cordial's own v0.20.1 asset, because Stacked has cut no
  release to pin. It becomes `stacked-bin/` when there is one: rename it, and
  bump the pkgver, the asset URL and the checksum together as below. Until
  then do not publish it; it would install Cordial.

**This cannot be done for you by an agent, and the reason is worth stating
rather than working around.** Publishing needs an AUR account and an SSH key
registered against it. Creating accounts and handling credentials is exactly
the class of thing a coding agent must not do on someone's behalf, so what
follows is the whole procedure for a person to run once per package.

## Once, to set up

Register at <https://aur.archlinux.org/register>, then add your public key under
*My Account → SSH Public Key*. The AUR authenticates by key alone; there is no
password prompt on push.

```bash
cat >> ~/.ssh/config <<'CONF'
Host aur.archlinux.org
  User aur
  IdentityFile ~/.ssh/aur
  IdentitiesOnly yes
CONF
```

## Each time

The AUR wants a repository whose root *is* the package directory — `PKGBUILD`
and `.SRCINFO` at the top level, not under `packaging/aur/<name>/`. So this is a
separate checkout that the files are copied into, rather than a remote on this
repository. Substitute `stacked-git` for `stacked` throughout for the other
package; the steps are the same.

```bash
git clone ssh://aur@aur.archlinux.org/stacked.git /tmp/aur-stacked
cp packaging/aur/stacked/{PKGBUILD,.SRCINFO,stacked.install} /tmp/aur-stacked/
cd /tmp/aur-stacked
git add -A && git commit -m "Update to 0.7.0" && git push
```

**Regenerate `.SRCINFO` on an Arch machine before pushing**, with
`makepkg --printsrcinfo > .SRCINFO`. The copy in this repository is maintained
by hand because the machine Cordial was developed on is Fedora and has neither
`makepkg` nor `namcap`, so it is kept in step deliberately rather than
generated. A hand-maintained `.SRCINFO` that has drifted from its `PKGBUILD` is
the most common way an AUR package breaks, and it breaks silently: the AUR
serves the metadata from `.SRCINFO` and builds from `PKGBUILD`.

For `stacked/PKGBUILD` specifically, bumping `pkgver` and regenerating
`.SRCINFO` is the whole of what a new release needs done to this file by hand.
Nothing else in it should change from one release to the next unless the
dependency list or the build itself changed too.

For the `-bin` PKGBUILD, a new release bumps three things together:
`pkgver`, the pinned asset URL (whose token `0.17.0.r0.g5412f88` is the
`git describe` build id, so it changes on every release), and `sha256sums`,
regenerated with `sha256sum` on the downloaded file. The package stays
version-pinned on purpose — a dynamic "latest release" fetch would need a SKIP
checksum, which violates the AUR trust model, and its declared pkgver could not
be the artifact's real version, which silently breaks helper tools' upgrade
checking.

## Check before pushing, on Arch

```bash
cd packaging/aur/stacked       # or stacked-git
makepkg --printsrcinfo | diff -u .SRCINFO -   # must be empty
makepkg -si                                    # it must actually build
namcap PKGBUILD
```

`makepkg -si` is not optional. The native subtree refuses a non-Clang
compiler outright, needs `binutils` for CMake's archiver step, and compiles the
"this backend is unavailable" arm of each audio backend when its headers are
missing — so a package that builds without `libpipewire`, `libpulse` or
`alsa-lib` present produces a client with no sound and no error, which nobody
would attribute to packaging. For the `-bin` package the equivalent check is
`sourcedir="$(mktemp -d)"; bsdtar -xf cordial.pkg.tar.zst -C "$sourcedir"`,
then a quick `namcap "$sourcedir/cordial-*.pkg.tar.zst"` — the artifact, not
the repack — since the repack is a straight copy of a package CI already
shipped.

## What CI already does, and does not

`.github/workflows/release.yml` runs `makepkg` against `packaging/aur/stacked/PKGBUILD`
on every tag, inside a fresh `archlinux:base-devel` container, and attaches the
resulting package to the GitHub release. That is real evidence the PKGBUILD
builds — but CI cannot register an AUR account or hold the SSH key that pushing
needs, so it stops there. Nothing here is published to the AUR until a
maintainer runs the steps above by hand, and CI building green is not that.

There is a `-bin` package, still named `cordial-bin` for the reason above: it
repackages the `.pkg.tar.zst` CI already attaches to the GitHub release, so the
hosting question is already answered. It is deliberately not built by
`release.yml` — it downloads the arch job's own output, and building it there
would be circular. Like the other two, the actual push to the AUR stays a manual
maintainer step: CI building green is not the same as the package being
published, and an Arch user's `yay -S stacked-bin` won't resolve until that step
happens. Its maintainer keeps pkgver, the pinned asset URL and the checksum in
lockstep, because a bin package that resolves anything other than the exact
version it declares would break the upgrade tracking every AUR helper relies on.