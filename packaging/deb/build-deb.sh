#!/usr/bin/env bash
# Build a .deb for Stacked by hand.
#
# Not dpkg-buildpackage or cargo-deb. This is a virtual Cargo workspace with
# no top-level [package] section, which is the same problem that already
# ruled out %cargo_install in packaging/rpm/stacked.spec -- both cargo-deb and
# debhelper's dh_auto_install assume one crate is the package, and this one is
# a workspace of five. So, like the RPM spec, this compiles the two binaries
# directly and stages a package root by hand, then calls dpkg-deb itself
# rather than the tooling built on top of it.
#
# Usage:
#     packaging/deb/build-deb.sh [--outdir DIR]
#
# Needs: cargo (stable, matching the workspace's rust-version), clang and
# clang++ as CC/CXX (native/CMakeLists.txt refuses a non-Clang compiler
# outright -- AOSP bionic uses C11 _Atomic inside C++ headers and GCC rejects
# it with 144 errors), cmake, pkg-config, dpkg-deb, and the GTK4/libadwaita
# development headers README.md §3 names for Debian and Ubuntu:
#
#     apt install libgtk-4-dev libadwaita-1-dev libwebkitgtk-6.0-dev \
#         libpipewire-0.3-dev libpulse-dev libasound2-dev
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(git -C "$here" rev-parse --show-toplevel)
outdir="$repo/dist/deb"
while [ $# -gt 0 ]; do
    case "$1" in
        --outdir) outdir=$2; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

cd "$repo"
eval "$(packaging/version.sh)"

# Debian version syntax: '+' marks a build newer than the release it is
# closest to, which is the true relationship for a snapshot built after a tag.
# '~', the other common separator, sorts *before* the bare version and is for
# pre-releases -- the wrong direction for a commit that came after v$CORDIAL_VERSION.
if [ "$CORDIAL_COMMITS" = "0" ]; then
    pkgversion="$CORDIAL_VERSION"
else
    pkgversion="${CORDIAL_VERSION}+${CORDIAL_COMMITS}.g${CORDIAL_SHORTHASH}"
fi
debversion="${pkgversion}-1"

echo "==> building stacked ${CORDIAL_DESCRIBE} (deb version ${debversion})"

export CC=clang CXX=clang++
# See the %describe comment in packaging/rpm/stacked.spec: without this the
# packaged binary's window title falls back to the bare Cargo version and
# disagrees with the package that shipped it.
# `CORDIAL_GIT_SHA`, not a version. `Cargo.toml` is the version now and a
# packager may not override it -- a release job that stamped its own would
# be the second, disagreeing number this scheme exists to remove. The build
# happens outside a git checkout here, so the commit has to be passed in.
export CORDIAL_GIT_SHA="$CORDIAL_SHORTHASH"

# Both crates' `webview` features, never one alone. The shell holds the
# WebKit window and cordial-runtime holds the presenter that calls it, so
# enabling only one leaves the caller cfg'd out, the linker collects
# webview::open, and the binary carries no WebKitGTK at all -- silently: the
# build still succeeds. This exact shape shipped once in the Flatpak and was
# reported as "webview doesnt work in cordial flatpak". The readelf check
# below is what catches it here instead.
cargo build --release --locked \
    --features cordial-shell/webview,cordial-runtime/webview

readelf -d target/release/cordial-run | grep -qi webkit || {
    echo "cordial-run linked no WebKitGTK; the webview features did not take" >&2
    exit 1
}

root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT

# Both binaries, side by side. launch.rs looks for the loader as the sibling
# of current_exe and nowhere else, so `stacked` installed without cordial-run
# beside it is a launcher with nothing to launch. `stacked` is the command and
# needs no alias; `cordial-run` is the loader it starts and is not what anyone
# should run by hand.
install -Dm755 target/release/stacked     "$root/usr/bin/stacked"
install -Dm755 target/release/cordial-run "$root/usr/bin/cordial-run"

# **Strip our own two binaries, which are almost entirely debug info.**
# `[profile.release]` in Cargo.toml sets `debug = true`, deliberately -- AGENTS.md
# leans on lldb and gdb against a running client and says so at length, and a
# runtime you cannot get a backtrace out of is not worth the disk it saves. But
# that is an argument about the build on a developer's machine, not about what a
# user downloads: `cordial-run` is 207.4 MB unstripped and 15.7 MB with
# `--strip-debug`, measured here, and the GTK launcher `stacked` replaced was
# another 175.7 MB; `stacked` itself has not been measured.
#
# Stripping at packaging time keeps both: full DWARF where somebody is debugging,
# and a package that is not thirteen times larger than it needs to be. rpmbuild
# and makepkg already do this by themselves, which is the whole reason the rpm
# and the Arch package were a tenth the size of the others.
#
# This package is built with plain `dpkg-deb` rather than debhelper -- see the
# note above the control file for why -- so `dh_strip`, which every ordinary
# Debian package gets for free, never runs here. Nothing else was going to.
strip --strip-debug "$root/usr/bin/stacked" "$root/usr/bin/cordial-run"

# The square icon under packaging/icons/hicolor/, not the 680x480 README
# banner.
install -Dm644 packaging/icons/hicolor/scalable/apps/io.github.damnshabu.Stacked.svg \
    "$root/usr/share/icons/hicolor/scalable/apps/io.github.damnshabu.Stacked.svg"

# First-party plugins, read-only beside the binary -- see the same block in
# packaging/rpm/stacked.spec for why this is per-package rather than a path
# compiled in. usr/share/cordial keeps Cordial's name because the code that
# reads it does.
for plugin in plugins/*/; do
    id=$(basename "$plugin")
    [ -f "$plugin/plugin.json" ] || continue
    install -Dm644 "$plugin/plugin.json" "$root/usr/share/cordial/plugins/$id/plugin.json"
    install -Dm644 "$plugin/main.ts"     "$root/usr/share/cordial/plugins/$id/main.ts"
done
# Exec=stacked %u, and the %u is not decorative: the entry registers
# x-scheme-handler/roblox-player, which is how a Play button on the website
# reaches a client at all.
install -Dm644 packaging/io.github.damnshabu.Stacked.desktop \
    "$root/usr/share/applications/io.github.damnshabu.Stacked.desktop"
install -Dm644 packaging/io.github.damnshabu.Stacked.metainfo.xml \
    "$root/usr/share/metainfo/io.github.damnshabu.Stacked.metainfo.xml"
# Tab completion, from the copies a test keeps equal to `stacked completions`.
# Debian's zsh reads vendor completions from vendor-completions, not
# site-functions.
install -Dm644 packaging/completions/stacked.bash "$root/usr/share/bash-completion/completions/stacked"
install -Dm644 packaging/completions/_stacked     "$root/usr/share/zsh/vendor-completions/_stacked"
install -Dm644 packaging/completions/stacked.fish "$root/usr/share/fish/vendor_completions.d/stacked.fish"

# Debian policy puts this under the package's own name, and the package is
# `stacked`.
docdir="$root/usr/share/doc/stacked"
install -Dm644 THIRD-PARTY-NOTICES.md "$docdir/THIRD-PARTY-NOTICES.md"
# Apache-2.0 section 4(d): the NOTICE for mocktail-webview, the basis for
# the in-experience web window, has to travel with a binary distribution and
# not only with the source tree.
install -Dm644 NOTICE "$docdir/NOTICE"
install -Dm644 third_party/libbadcpu/LICENSE.upstream "$docdir/libbadcpu-MIT.txt"
install -Dm644 third_party/mcpelauncher-linker/LICENSE "$docdir/mcpelauncher-linker-MIT.txt"
install -Dm644 third_party/mcpelauncher-linker/core/NOTICE "$docdir/aosp-NOTICE.txt"
install -Dm644 third_party/libjnivm/LICENSE "$docdir/libjnivm-MIT.txt"
install -Dm644 third_party/mocktail-webview/LICENSE "$docdir/mocktail-webview-Apache-2.0.txt"
install -Dm644 packaging/deb/copyright "$docdir/copyright"

mkdir -p "$root/DEBIAN"
# `dpkg --print-architecture` reads the *building* dpkg's own configured
# architecture, which on this container is whatever the runner actually is --
# amd64 on an x86_64 runner, arm64 on an aarch64 one -- so this needs no
# uname-based guessing and no case statement mapping Rust's target_arch names
# onto Debian's (which disagree: aarch64 is "arm64" here, not "aarch64").
debarch=$(dpkg --print-architecture)
echo "==> debian architecture: $debarch"

# `control.in` is a template with a comment header explaining itself, and
# **a Debian control file has no comment syntax at all** -- dpkg-deb reads a
# leading `#` as a malformed field and stops with "parsing file ... near line
# 0", which names the line it could not read rather than the one at fault.
# That is exactly how this failed in CI on 2026-08-27. Strip the comments and
# any blank lines they leave in front of the first real field.
sed -e "s/@VERSION@/${debversion}/" -e "s/@ARCH@/${debarch}/" -e "/^#/d" packaging/deb/control.in \
  | sed -e "/./,$ !d" > "$root/DEBIAN/control"
# A control file that begins with anything but a field is not worth handing to
# dpkg-deb, whose error would again name the wrong line.
head -1 "$root/DEBIAN/control" | grep -q "^Package:" || {
  echo "error: generated DEBIAN/control does not start with Package:" >&2
  head -3 "$root/DEBIAN/control" >&2
  exit 1
}
# **No substitution variables may survive into the finished control.**
# `${misc:Depends}` and `${shlibs:Depends}` are debhelper's, expanded by
# `dh_gencontrol` -- and this script builds the package by hand, so nothing
# expands them and dpkg-deb rejects the literal text with "invalid package name
# '${misc'". That reached CI on 2026-08-27, one round after a different
# malformed control, and each round costs a full workspace build to discover a
# defect visible in the file itself. Catch it here instead.
if grep -n '\${' "$root/DEBIAN/control" >&2; then
  echo "error: unexpanded substitution variable in DEBIAN/control (see above)." >&2
  echo "       debhelper expands those and this package is built without it;" >&2
  echo "       list the dependency literally or drop it." >&2
  exit 1
fi
install -m755 packaging/deb/postinst "$root/DEBIAN/postinst"

mkdir -p "$outdir"
out="$outdir/stacked_${debversion}_${debarch}.deb"
dpkg-deb --build --root-owner-group "$root" "$out"

echo
dpkg-deb --info "$out"
echo "built: $out"
