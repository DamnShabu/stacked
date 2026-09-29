#!/usr/bin/env bash
# Build a signed APT repository from one or more already-built .deb files.
#
# This is Stacked's own repository -- the analogue of
# packaging/stacked.flatpakrepo and packaging/build-flatpak.sh for `apt
# install` rather than `flatpak install`. It does not build Stacked and does
# not build a .deb; packaging/deb/build-deb.sh already does that, and this
# script's whole job starts after that one's is finished. See
# docs/design/apt-repository.md for what this is, what it deliberately is
# not (an upload to Debian proper), and the procedure for the key.
#
# Usage:
#     packaging/apt/build-repo.sh [--outdir DIR] [--allow-unsigned] DEB [DEB...]
#
# Signing is driven by APT_GPG_KEY_ID, the fingerprint of a secret key
# already present in the calling GNUPGHOME -- this script never imports a
# key itself, the same division flatpak.yml's "Import the signing key" step
# and this script's own caller keep: importing is CI's job (or a
# maintainer's, by hand), signing-with-an-ID-that's-already-there is this
# script's. Without APT_GPG_KEY_ID set, the script refuses outright rather
# than writing a Packages/Release tree that has every file an installable
# repository needs except the one that makes it trustworthy -- a thing that
# *looks* finished is worse here than a thing that visibly is not, and an
# apt repository with no signature only works at all if a user adds
# `[trusted=yes]`, which is a worse thing to hand someone than an error
# message. --allow-unsigned overrides that refusal, loudly, for testing the
# tree shape on a machine with no key -- see the README note beside it below.
#
# Needs: ar, tar, gzip, and a checksum tool (sha256sum/md5sum -- GNU
# coreutils; this script does not attempt the BSD/macOS equivalents because
# nothing else in packaging/ runs there either). gpg only when signing.
# apt-ftparchive, from Debian/Ubuntu's apt-utils package, is used when
# present and produces byte-for-byte the same tool real Debian mirrors use;
# where it is absent (this repository's own development host is Fedora
# Silverblue, which carries no Debian packaging tools at all and cannot
# gain them without an rpm-ostree rebuild and a reboot -- see AGENTS.md's
# note on gdb for the same constraint) the script falls back to a short
# hand-rolled equivalent using ar/tar/gzip/sha256sum, which is what this
# script has actually been exercised against: see the report for this
# change for the synthetic-.deb run that verified it end to end. Both
# branches scan the whole pool/ tree rather than just the .debs passed to
# this invocation -- an earlier version of the hand-rolled branch did not,
# and so disagreed with apt-ftparchive about which packages a second release
# should list; see the "reading control stanzas from the pool" step below,
# which is what keeps that from recurring.
#
# **The apt-ftparchive branch is therefore INFERRED, not verified** -- it has
# passed `bash -n` and a read-through against apt-ftparchive(1), not a run,
# because no host available while writing this had the tool installed.
# CI's ubuntu container installs apt-utils specifically so that branch gets
# exercised for real the first time this runs there; if it is ever wrong,
# CI is where that will show, not here.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(git -C "$here" rev-parse --show-toplevel)

# SUITE and COMPONENT are still fixed rather than flags: this repository has
# one suite and one component today, and the brief that produced this script
# names dists/stable/ specifically. Widen these into flags the day a second
# suite (e.g. an unstable/nightly channel) is actually wanted -- there is no
# reason to carry the option before there is a caller for it.
#
# ARCH is no longer fixed. It used to be, on the same "widen it the day a
# second one is wanted" reasoning -- that day is release.yml's `deb` job
# gaining an aarch64 leg, so this script now reads each .deb's own
# Architecture field instead of asserting one and refusing the rest. See the
# per-architecture Packages generation below.
SUITE=stable
COMPONENT=main

outdir="$repo/dist/apt-repo"
allow_unsigned=0
debs=()
while [ $# -gt 0 ]; do
    case "$1" in
        --outdir) outdir=$2; shift 2 ;;
        --allow-unsigned) allow_unsigned=1; shift ;;
        --) shift; break ;;
        -*) echo "unknown argument: $1" >&2; exit 2 ;;
        *) debs+=("$1"); shift ;;
    esac
done
debs+=("$@")

if [ "${#debs[@]}" -eq 0 ]; then
    echo "usage: $(basename "$0") [--outdir DIR] [--allow-unsigned] DEB [DEB...]" >&2
    exit 2
fi
for deb in "${debs[@]}"; do
    [ -f "$deb" ] || { echo "error: $deb does not exist" >&2; exit 2; }
done

# The refusal this script exists to make. An empty APT_GPG_KEY_ID and a
# missing one are treated alike -- a secret set to the empty string is not a
# key either.
if [ -z "${APT_GPG_KEY_ID:-}" ] && [ "$allow_unsigned" -ne 1 ]; then
    cat >&2 <<'EOF'
error: APT_GPG_KEY_ID is not set.

This script refuses to build an apt repository with no signature, because an
unsigned one still produces every file `apt` needs to install from it -- the
only difference is that a user has to write [trusted=yes] into their
sources.list to use it, and this script is not going to be the thing that
makes that look like the normal path.

Import a signing key into this GNUPGHOME first (see
docs/design/apt-repository.md for the full procedure -- it mirrors
docs/design/flatpak-remote-signing.md) and set APT_GPG_KEY_ID to its
fingerprint, or pass --allow-unsigned to build the tree anyway, for
checking the layout on a machine with no key. A tree built with
--allow-unsigned is for your own inspection; publishing it is the one thing
--allow-unsigned does not make acceptable.
EOF
    exit 1
fi
if [ "$allow_unsigned" -eq 1 ] && [ -n "${APT_GPG_KEY_ID:-}" ]; then
    echo "APT_GPG_KEY_ID is set; ignoring --allow-unsigned and signing anyway" >&2
    allow_unsigned=0
fi
if [ "$allow_unsigned" -eq 1 ]; then
    echo "::warning::--allow-unsigned: building an UNSIGNED apt repository. Do not publish this tree." >&2
fi

# Resolve to an absolute path now, once. Every path below ($distdir,
# $pooldir, $binarydir, and the Packages/Release targets built from them) is
# captured as a string relative to *this* shell's cwd -- but the
# apt-ftparchive branch further down `cd`s into $outdir first, because that
# is the only way to hand apt-ftparchive the "pool/$COMPONENT" argument in
# the relative form its Filename: output is supposed to take. A relative
# --outdir left unresolved survives that cd as a string that no longer
# points where it did: `> "$binarydir/Packages"` re-relativises against the
# new cwd and tries to create pool/apt-repo/dist/apt-repo/dists/..., which
# does not exist, and the redirection fails outright. Absolute from here on
# makes every later use of $outdir safe regardless of which directory the
# script happens to be sitting in when it runs.
mkdir -p "$outdir"
outdir=$(cd "$outdir" && pwd)

distdir="$outdir/dists/$SUITE"
pooldir="$outdir/pool/$COMPONENT"
rm -rf "$distdir" # dists/ is regenerated wholesale each run; pool/ is not,
                   # so that re-running this against a superset of .debs
                   # (an old release's package alongside a new one, say)
                   # does not require every prior .deb to be passed again.
mkdir -p "$pooldir"

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT

# Returns a package's control stanza on stdout, from the control.tar member
# of a .deb -- a .deb is an `ar` archive of debian-binary, control.tar.*,
# data.tar.*, in that order (this is not a heuristic; it is the literal
# format dpkg-deb writes and the format `ar rc` below in this repository's
# own test harness constructs by hand). Compression on control.tar varies by
# the dpkg that built it -- gzip historically, xz or zstd on newer
# toolchains -- so the member name is read rather than assumed.
read_control() {
    local deb="$1"
    local member
    member=$(ar t "$deb" | grep -m1 '^control\.tar')
    if [ -z "$member" ]; then
        echo "error: $deb has no control.tar member -- is it really a .deb?" >&2
        return 1
    fi
    local raw="$workdir/control.tar"
    case "$member" in
        *.tar.gz)  ar p "$deb" "$member" | gzip -dc  > "$raw" ;;
        *.tar.xz)  ar p "$deb" "$member" | xz -dc    > "$raw" ;;
        *.tar.zst) ar p "$deb" "$member" | zstd -dc  > "$raw" ;;
        *.tar)     ar p "$deb" "$member"              > "$raw" ;;
        *) echo "error: $deb control member '$member' has an unrecognised compression" >&2; return 1 ;;
    esac
    tar -xO -f "$raw" ./control 2>/dev/null || tar -xO -f "$raw" control
}

# Pull one field's value out of a control stanza already on stdin's first
# argument. Single-line fields only (Package, Version, Architecture) --
# Description is multi-line and is never looked up this way; it travels
# with the rest of the stanza verbatim instead.
control_field() {
    local name="$1" text="$2"
    awk -v f="$name:" 'index($0, f) == 1 { sub("^" f " *", ""); print; exit }' <<<"$text"
}

echo "==> reading control stanzas from ${#debs[@]} package(s)"
for deb in "${debs[@]}"; do
    control=$(read_control "$deb")
    pkg=$(control_field Package "$control")
    ver=$(control_field Version "$control")
    arch=$(control_field Architecture "$control")
    if [ -z "$pkg" ] || [ -z "$ver" ]; then
        echo "error: $deb: could not read Package/Version from its control stanza" >&2
        exit 1
    fi
    if [ -z "$arch" ]; then
        echo "error: $deb: could not read Architecture from its control stanza" >&2
        exit 1
    fi

    # pool/<component>/<prefix>/<package>/<file>.deb, the standard Debian
    # archive layout: prefix is the package's first letter, except for a
    # lib-prefixed package, which uses its first four characters (libg,
    # libc...) so that the hundreds of libfoo packages on a real mirror do
    # not all collect under pool/main/l/.  Stacked ships one package and
    # this branch is exercised only by "stacked" today, but a plugin or a
    # future split package should not need this rewritten.
    case "$pkg" in
        lib?*) prefix="${pkg:0:4}" ;;
        *)     prefix="${pkg:0:1}" ;;
    esac
    pkgpooldir="$pooldir/$prefix/$pkg"
    mkdir -p "$pkgpooldir"
    poolfile="$pkgpooldir/$(basename "$deb")"
    install -m644 "$deb" "$poolfile"
    relpath="pool/$COMPONENT/$prefix/$pkg/$(basename "$deb")"

    echo "    $pkg $ver ($arch) -> $relpath"
done

have_apt_ftparchive=0
if command -v apt-ftparchive >/dev/null 2>&1; then
    have_apt_ftparchive=1
fi

# Packages must describe every .deb sitting in pool/, not only the ones this
# invocation was handed -- pool/ persists across runs (see the note by
# `rm -rf "$distdir"` above) while dists/ is rebuilt from scratch every time,
# so a Packages file that only ever looked at this run's arguments would
# forget about an older release's .deb the moment a second one is published
# alongside it, even though the file is still sitting right there in pool/
# and apt would otherwise have served it. apt-ftparchive already walks
# "pool/$COMPONENT" itself and always has; this loop exists so the
# hand-rolled fallback below does the same walk instead of describing a
# strict subset of it -- that is what makes the two branches agree, and it
# is why this scans $pooldir again here rather than reusing the per-.deb
# work the loop above already did for this invocation's own packages.
echo "==> reading control stanzas from the pool"
packages_body=""
declare -A archs_seen=()
while IFS= read -r poolfile; do
    control=$(read_control "$poolfile")
    pkg=$(control_field Package "$control")
    ver=$(control_field Version "$control")
    arch=$(control_field Architecture "$control")
    if [ -z "$pkg" ] || [ -z "$ver" ]; then
        echo "error: $poolfile: could not read Package/Version from its control stanza" >&2
        exit 1
    fi
    if [ -z "$arch" ]; then
        echo "error: $poolfile: could not read Architecture from its control stanza" >&2
        exit 1
    fi
    archs_seen["$arch"]=1
    relpath=${poolfile#"$outdir/"}

    size=$(stat -c%s "$poolfile")
    md5=$(md5sum "$poolfile" | cut -d' ' -f1)
    sha256=$(sha256sum "$poolfile" | cut -d' ' -f1)

    # Strip any trailing blank lines from the stanza before appending the
    # archive-computed fields -- dpkg-deb's control file ends in a newline,
    # not a blank line, but nothing guarantees that and a spurious blank
    # line here would split one stanza into two in the eyes of a Packages
    # parser.
    stanza=$(printf '%s\n' "$control" | sed -e '$ { /^$/d }')
    stanza="${stanza}
Filename: $relpath
Size: $size
MD5sum: $md5
SHA256: $sha256"
    packages_body="${packages_body}${stanza}

"
done < <(find "$pooldir" -type f -name '*.deb' | sort)

if [ "${#archs_seen[@]}" -eq 0 ]; then
    echo "error: no .deb found under $pooldir after copying the arguments in" >&2
    exit 1
fi
# Sorted so the arch list in Release and the order `find` above already
# produced for iteration are both deterministic across runs -- bash's
# associative-array key order otherwise depends on insertion, which here
# means "whichever order the pool happened to be scanned in".
mapfile -t archs < <(printf '%s\n' "${!archs_seen[@]}" | sort)
echo "==> architectures present in the pool: ${archs[*]}"

# One binary-$ARCH directory and one Packages file per architecture, the
# standard multi-architecture apt layout -- Architecture: is part of a
# Packages stanza's own identity, not the directory's, so a repository
# serving both amd64 and arm64 needs both stanza sets kept apart rather than
# interleaved in one file that every apt would then have to filter itself.
# This script used to serve exactly one $ARCH, fixed by a variable rather
# than discovered, from back when release.yml's `deb` job built only one.
raw_packages="$workdir/all-packages"
if [ "$have_apt_ftparchive" -eq 1 ]; then
    echo "==> apt-ftparchive found; using it for Packages"
    ( cd "$outdir" && apt-ftparchive packages "pool/$COMPONENT" ) > "$raw_packages"
else
    echo "==> apt-ftparchive not found; writing Packages by hand (see this script's header)"
    printf '%s' "$packages_body" > "$raw_packages"
fi

for arch in "${archs[@]}"; do
    binarydir="$distdir/$COMPONENT/binary-$arch"
    mkdir -p "$binarydir"
    # Paragraph mode (RS="") treats a Packages file as records separated by a
    # blank line, which is exactly its own format -- both branches above
    # write stanzas in that shape already. Matching "Architecture: $arch" as
    # its own line inside the record, rather than a substring match against
    # the whole record, is what keeps arm64 from also matching arm64-static
    # or some future architecture whose name embeds this one.
    awk -v want="$arch" 'BEGIN { RS=""; ORS="\n\n" }
        $0 ~ ("(^|\n)Architecture: " want "(\n|$)") { print }' \
        "$raw_packages" > "$binarydir/Packages"
    gzip -9 -kf "$binarydir/Packages"
done

echo "==> writing $distdir/Release"
release_date=$(date -u '+%a, %d %b %Y %H:%M:%S UTC')
{
    echo "Origin: Stacked"
    echo "Label: Stacked"
    echo "Suite: $SUITE"
    echo "Codename: $SUITE"
    echo "Components: $COMPONENT"
    echo "Architectures: ${archs[*]}"
    echo "Date: $release_date"
    echo "Description: Stacked's own APT repository -- see docs/design/apt-repository.md"
    # MD5Sum and SHA256 only, not SHA1: apt has treated SHA1 as untrusted
    # for repository metadata since 1.6 (2018) and the only thing MD5Sum is
    # still carried for here is the small number of very old apt versions
    # that look at nothing else. SHA256 is what every apt from the last
    # decade actually checks against.
    echo "MD5Sum:"
    for arch in "${archs[@]}"; do
        for f in "Packages" "Packages.gz"; do
            p="$distdir/$COMPONENT/binary-$arch/$f"
            printf ' %s %16d %s\n' "$(md5sum "$p" | cut -d' ' -f1)" "$(stat -c%s "$p")" "$COMPONENT/binary-$arch/$f"
        done
    done
    echo "SHA256:"
    for arch in "${archs[@]}"; do
        for f in "Packages" "Packages.gz"; do
            p="$distdir/$COMPONENT/binary-$arch/$f"
            printf ' %s %16d %s\n' "$(sha256sum "$p" | cut -d' ' -f1)" "$(stat -c%s "$p")" "$COMPONENT/binary-$arch/$f"
        done
    done
} > "$distdir/Release"

if [ "$allow_unsigned" -eq 1 ]; then
    echo "==> --allow-unsigned: not writing Release.gpg or InRelease"
else
    echo "==> signing with $APT_GPG_KEY_ID"
    # Detached signature for old apt (Release + Release.gpg) and an
    # inline-clearsigned InRelease for apt >= 1.1, which prefers InRelease
    # and only falls back to the Release/Release.gpg pair when it is
    # missing. Both are generated from the same $distdir/Release bytes so
    # they can never disagree with each other.
    gpg --batch --yes --local-user "$APT_GPG_KEY_ID" \
        --detach-sign --armor -o "$distdir/Release.gpg" "$distdir/Release"
    gpg --batch --yes --local-user "$APT_GPG_KEY_ID" \
        --clearsign -o "$distdir/InRelease" "$distdir/Release"

    # The public half, for a user's sources.list -- signing the repository
    # is pointless if there is nothing to check it against. `gpg --export`
    # with no --armor writes the binary OpenPGP form directly, which is what
    # apt's `signed-by=` wants; unlike the deprecated `apt-key add`, nothing
    # here touches a system-wide trusted.gpg.d. Named after the Debian
    # convention (debian-archive-keyring) rather than something generic like
    # pubkey.gpg, so it is recognisable for what it is if it ever ends up
    # somewhere out of context.
    gpg --batch --yes --export "$APT_GPG_KEY_ID" > "$outdir/stacked-archive-keyring.gpg"
fi

echo "==> built: $outdir"
find "$outdir" -type f | sort
