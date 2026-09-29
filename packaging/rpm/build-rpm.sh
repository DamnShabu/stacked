#!/usr/bin/env bash
# Build a binary RPM for Stacked: the SRPM from make-srpm.sh, then rpmbuild
# against it.
#
# make-srpm.sh already does the hard, reproducibility-sensitive part -- the
# tarball, the `cargo vendor` tree and a spec stamped with a matching
# %describe -- and stops at `rpmbuild -bs`, because that is as far as a Copr
# build needs it to go; Copr's own builders take the SRPM from there. This
# script is the other half, for producing a downloadable .rpm directly: it
# adds nothing to the spec, it just runs it.
#
# Usage:
#     packaging/rpm/build-rpm.sh [--outdir DIR]
#
# Needs everything make-srpm.sh needs (git, cargo, rpmbuild, zstd), plus
# whatever the spec's own BuildRequires ask for. Deliberately not listed
# again here: `dnf builddep` reads that list from the SRPM itself once it
# exists, which is one source of truth rather than two that can drift --
# see .github/workflows/release.yml, which is where this runs in practice,
# inside registry.fedoraproject.org/fedora:44 to match the toolchain
# test.yml already pins (gtk4 4.22, libadwaita 1.9; Fedora 43 and Ubuntu
# 24.04 are both older and fail three *-sys build scripts with a pkg-config
# hint that reads as a missing package rather than an old one).
#
# On a system where `dnf builddep` is not available or not wanted, install
# the packages packaging/rpm/stacked.spec's BuildRequires: block names, by
# hand, before running this.
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

need() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "error: $1 is not installed" >&2
        exit 1
    }
}
need rpmbuild

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
srcdir="$work/srpm"

echo "==> building the SRPM"
"$here/make-srpm.sh" --outdir "$srcdir"
srpm=$(find "$srcdir" -maxdepth 1 -name '*.src.rpm' -print -quit)
[ -n "$srpm" ] || { echo "error: make-srpm.sh produced no .src.rpm" >&2; exit 1; }
echo "    $srpm"

if command -v dnf >/dev/null 2>&1; then
    echo "==> installing the spec's own BuildRequires (dnf builddep)"
    # Root is assumed here, which holds inside the Fedora container this
    # normally runs in. Run this step by hand first, as root, if you are
    # calling this script on a host machine instead.
    dnf builddep -y "$srpm"
else
    echo "warning: dnf not found; assuming packaging/rpm/stacked.spec's" >&2
    echo "  BuildRequires are already satisfied on this system" >&2
fi

rpmdir=${outdir:-$repo/dist/rpm}
mkdir -p "$rpmdir"

echo "==> rpmbuild --rebuild"
# --rebuild runs %prep, %build, %install and %check straight from the SRPM's
# own embedded spec and sources -- no network needed at this stage, because
# make-srpm.sh already vendored the crate graph into Source1. %check is the
# same cargo test run the spec documents, with the same three skips, for the
# same reason: two of them talk to whatever org.freedesktop.secrets is on the
# session bus, and a plain container build has none to talk to.
rpmbuild --rebuild "$srpm" --define "_rpmdir $rpmdir"

echo
find "$rpmdir" -name '*.rpm' -exec rpm -qip {} \;
echo "built into $rpmdir"
