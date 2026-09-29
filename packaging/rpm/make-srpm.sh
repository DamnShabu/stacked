#!/usr/bin/env bash
# Build a source RPM for Stacked from a git checkout.
#
# Two tarballs, and both exist because a Copr build may run with networking
# switched off and a build that only works when it happens to be on is not
# reproducible:
#
#   <archive>.tar.zst         the working tree, submodules included -- `git
#                             archive` cannot do that, so this uses
#                             `git ls-files --recurse-submodules`
#   <archive>-vendor.tar.zst  `cargo vendor`, the 200-odd crates Cargo.lock
#                             already pins
#
# Run it from anywhere; it locates the repository from its own path.
#
#     packaging/rpm/make-srpm.sh              # SRPM into ~/rpmbuild/SRPMS
#     packaging/rpm/make-srpm.sh --outdir /tmp/out
#
# Needs `git`, `cargo`, `rpmbuild` and `zstd`. On an atomic host none of those
# are on the host filesystem; run this inside a toolbox.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(git -C "$here" rev-parse --show-toplevel)
outdir=""
while [ $# -gt 0 ]; do
    case "$1" in
        --outdir) outdir=$2; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

cd "$repo"

# **The version comes from `git describe --tags` and from nowhere else.**
# The window title is `git describe --tags --always --dirty`, stamped by
# crates/cordial-shell/build.rs, so a package numbered from any other source
# would disagree with the string the running client prints -- and a build with
# no tag in reach shows a bare hash, which has already produced one confusing
# bug report.
describe=$(git describe --tags --long --abbrev=7)   # v0.6.0-108-g9d9c980
case "$describe" in
    v*) ;;
    *) echo "expected a v-prefixed tag, got '$describe'" >&2; exit 1 ;;
esac
version=${describe#v}; version=${version%%-*}       # 0.6.0
rest=${describe#v${version}-}                       # 108-g9d9c980
commits=${rest%%-*}                                 # 108
shorthash=${rest#*-g}                               # 9d9c980
commit=$(git rev-parse HEAD)
date=$(git show -s --format=%cd --date=format:%Y%m%d HEAD)

if [ -n "$(git status --porcelain)" ]; then
    # Not fatal, because the tarball is built from HEAD and is therefore
    # unaffected -- see the staging step, and read the warning there before
    # changing it back.
    echo "note: the working tree is dirty; the tarball is built from HEAD ($describe) and ignores it" >&2
fi

if [ "$commits" = "0" ]; then
    snapinfo=""
    release='1%{?dist}'
    archive="stacked-${version}"
else
    snapinfo="${commits}.${date}git${shorthash}"
    release="1.${snapinfo}%{?dist}"
    archive="stacked-${version}-${snapinfo}"
fi

# /var/tmp rather than /tmp: on the host this was written for, /tmp is tmpfs
# with under 5 GB, and the vendored crate tree plus two tarballs comes to a few
# hundred megabytes before rpmbuild has started. Override with TMPDIR.
work=$(mktemp -d -p "${TMPDIR:-/var/tmp}")
trap 'rm -rf "$work"' EXIT
# The directory name is what the spec's %%autosetup -n %%{name}-%%{version}
# expects, so it follows Name: there.
stage="$work/stacked-${version}"

echo "==> staging HEAD (submodules included)"
# **`git archive HEAD`, not the working tree, and this cost a whole build to
# learn.** The first version of this used `git ls-files --recurse-submodules`
# piped into tar, which lists the tracked *paths* and then reads their contents
# out of the working tree. Another agent was editing crates/ at the moment the
# tarball was cut, and it captured a half-finished change: the new
# implementation of DeviceProfile::parse alongside the old assertion about it.
# %check duly failed with
#
#   left: Some(PcWindows11)  right: Some(AndroidTablet)
#
# which looks exactly like a broken package and was a broken snapshot. The same
# commit built from a pristine clone passes. A source tarball must be a
# function of a commit and nothing else.
#
# One `git archive` per repository, because git archive does not descend into
# submodules -- and without third_party/mcpelauncher-linker present,
# native/CMakeLists.txt has no linker to build and the *-sys build script
# panics with "is not checked out". `git submodule status --recursive` prints
# paths relative to the top level, nested ones included, so the loop is flat.
git archive --format=tar --prefix="stacked-${version}/" HEAD | tar -xf - -C "$work"
while read -r path; do
    [ -n "$path" ] || continue
    ( cd "$path" && git archive --format=tar --prefix="stacked-${version}/${path}/" HEAD ) \
        | tar -xf - -C "$work"
done < <(git submodule status --recursive | awk '{print $2}')

echo "==> vendoring crates"
( cd "$stage" && cargo vendor --locked "$work/vendor" >/dev/null )

echo "==> writing tarballs"
srcdir=${outdir:-$(rpm --eval %{_sourcedir})}
mkdir -p "$srcdir"
tar --zstd -cf "$srcdir/${archive}.tar.zst" -C "$work" "stacked-${version}"
tar --zstd -cf "$srcdir/${archive}-vendor.tar.zst" -C "$work" vendor

echo "==> writing the spec"
spec="$work/stacked.spec"
# **On an exact tag `snapinfo` is empty, and `%global snapinfo` with nothing
# after it is a hard rpm error** -- "Macro %snapinfo has empty body", at parse
# time, before anything is built. This path only runs when building from a tag,
# which is the only time it matters and the reason it went unnoticed until
# v0.9.0: every push to main has commits after the tag and takes the other
# branch. `archivename` is rewritten to drop the trailing component too, since
# `stacked-0.9.0-` is not the tarball that was just written.
if [ -z "$snapinfo" ]; then
    snap_edit=(-e "/^%global snapinfo /d"
               -e "s|^%global archivename .*|%global archivename %{name}-%{version}|")
else
    snap_edit=(-e "s|^%global snapinfo .*|%global snapinfo ${snapinfo}|")
fi
sed "${snap_edit[@]}" \
    -e "s|^%global commit   .*|%global commit   ${commit}|" \
    -e "s|^%global describe .*|%global describe ${describe#v}|" \
    -e "s|^Version: *.*|Version:        ${version}|" \
    -e "s|^Release: *.*|Release:        ${release}|" \
    "$here/stacked.spec" > "$spec"

echo "==> rpmbuild -bs"
# **rpm expands macros inside comments.** An unescaped %macro in a comment is
# replaced with its definition and the result is emitted into the generated
# script, where bash reads a leading percent as a job spec and fails with "fg:
# no job control" -- which names neither the macro nor the comment. That reached
# CI on 2026-08-27 from a single comment that wrote %autosetup instead of
# %%autosetup, while every other comment in the spec already doubled them.
# Checked here because the spec is plain text and this costs nothing, whereas
# finding it in CI costs a full workspace build first.
if grep -n '^#' "$spec" | grep -E '[^%]%[a-zA-Z_]+' >&2; then
  echo "error: unescaped rpm macro in a spec comment (see above)." >&2
  echo "       rpm expands macros in comments; double the percent: %%macro." >&2
  exit 1
fi

rpmbuild -bs "$spec" \
    --define "_sourcedir $srcdir" \
    ${outdir:+--define "_srcrpmdir $outdir"}
