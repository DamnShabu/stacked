#!/usr/bin/env bash
# Build Stacked as a Flatpak.
#
# Flatpak is the primary distribution target (spec §11). Building one on every
# change is worth the minute it costs: it is the only check that the runtime's
# dependencies are actually declared rather than merely present on the machine
# that happens to be building it. A Stacked that runs from `cargo build` but not
# from a Flatpak is a Stacked nobody else can install.
#
# Usage:
#     packaging/build-flatpak.sh [--install]

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname "$here")"
manifest="$here/io.github.damnshabu.Stacked.yml"
builddir="$root/target/flatpak"
repo="$root/target/flatpak-repo"

need() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "error: $1 is not installed" >&2
        exit 1
    }
}
need flatpak
need flatpak-builder

# The manifest names these explicitly; installing them here rather than letting
# flatpak-builder fail halfway keeps the error legible.
# Two versions, not one. The GNOME runtime carries GTK4 and libadwaita and is
# numbered 50; the freedesktop SDK extensions it inherits are numbered by the
# base runtime, 25.08. Deriving all four from `runtime-version` asked for a
# rust-stable//50 that does not exist.
runtime_version=$(sed -n "s/^runtime-version: *'\\(.*\\)'/\\1/p" "$manifest")
ext_version=$(sed -n "s/^# sdk-extension-version: *'\\(.*\\)'/\\1/p" "$manifest")
if [[ -z "$runtime_version" || -z "$ext_version" ]]; then
    echo "error: could not read runtime-version / sdk-extension-version from $manifest" >&2
    exit 1
fi
required=(
    "org.gnome.Platform//${runtime_version}"
    "org.gnome.Sdk//${runtime_version}"
    "org.freedesktop.Sdk.Extension.rust-stable//${ext_version}"
    "org.freedesktop.Sdk.Extension.llvm20//${ext_version}"
)
missing=()
for ref in "${required[@]}"; do
    flatpak info "$ref" >/dev/null 2>&1 || missing+=("$ref")
done

if (( ${#missing[@]} )); then
    echo "installing ${#missing[@]} missing runtime(s): ${missing[*]}"
    flatpak install --user --noninteractive --or-update flathub "${missing[@]}"
fi

# There used to be a check here that the submodules were checked out. It is
# gone because it no longer describes what happens: the manifest pins
# third_party/mcpelauncher-linker and third_party/libjnivm as `git` sources by
# commit and skips them out of the `dir` source, so this build clones them at
# those commits and ignores whatever is in the tree. A local edit to either
# submodule will not appear in the Flatpak, which is the point — issue #3 — and
# is worth knowing before spending an afternoon wondering why.

echo "building $(basename "$manifest") ..."
flatpak-builder \
    --force-clean \
    --user \
    --repo="$repo" \
    "$builddir" \
    "$manifest"

# Without this the repository has objects and no summary, and installing from it
# fails with "No remote refs found for '<path>'" — which reads like the path is
# wrong, and it is not. It also builds the appstream branch that lets a software
# centre list the app.
flatpak build-update-repo "$repo"

echo
echo "built into $repo"

# A single-file bundle, which is what you hand someone who wants to try it.
flatpak build-bundle "$repo" "$root/target/stacked.flatpak" io.github.damnshabu.Stacked
echo "bundle: $root/target/stacked.flatpak"

if [[ "${1:-}" == "--install" ]]; then
    flatpak install --user --noninteractive --or-update --reinstall "$repo" io.github.damnshabu.Stacked
    echo "installed. run with: flatpak run io.github.damnshabu.Stacked"
else
    echo "install with: packaging/build-flatpak.sh --install"
fi
