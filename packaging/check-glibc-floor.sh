#!/usr/bin/env bash
# Refuse to ship a binary that needs a glibc newer than the floor.
#
# **This exists because one line cost a whole release.** `native/opensles.cpp`
# computed a millibel volume with `std::log10` on a `float`. glibc 2.43 added a
# correctly-rounded `log10f` and made it the *default* binding, so building on a
# 2.43 host emitted `log10f@GLIBC_2.43` -- the only symbol in either binary
# above 2.39 -- and the `.rpm` then refused to install anywhere older with
#
#     nothing provides libm.so.6(GLIBC_2.43)(64bit) needed by cordial-0.10.0
#
# The AppImage had it too, and that one bundles no libc at all, so the format
# whose whole purpose is running anywhere would have run on Fedora 44 and
# nothing else. Nobody noticed because every builder and every developer
# machine was newer than every user.
#
# A version bump in a build container is invisible in a diff, arrives without
# anybody changing a line, and breaks only for people who are not in the room.
# That is exactly the class of thing worth a check rather than a habit.
set -euo pipefail

# The oldest glibc Stacked claims to run on. Raise this deliberately, in a
# commit that says which distribution it drops, never to make this script pass.
FLOOR="${CORDIAL_GLIBC_FLOOR:-2.39}"

fail=0
for bin in "$@"; do
  if [ ! -f "$bin" ]; then
    echo "check-glibc-floor: $bin does not exist" >&2
    exit 2
  fi
  # `objdump -T` lists the versioned symbols this binary imports. The version
  # node names are what rpm turns into `libc.so.6(GLIBC_x.y)` requirements, so
  # reading them here asks the same question the package manager will.
  highest=$(objdump -T "$bin" | grep -oE 'GLIBC_2\.[0-9]+' | sort -uV | tail -1 || true)
  if [ -z "$highest" ]; then
    echo "  $bin: no versioned glibc imports"
    continue
  fi
  want="GLIBC_$FLOOR"
  if [ "$(printf '%s\n%s\n' "$want" "$highest" | sort -V | tail -1)" != "$want" ]; then
    echo "  $bin: needs $highest, floor is $want" >&2
    echo "    symbols responsible:" >&2
    objdump -T "$bin" | grep "$highest" | awk '{print "      " $NF, $(NF-1)}' | sort -u >&2
    fail=1
  else
    echo "  $bin: highest is $highest, within $want"
  fi
done

if [ "$fail" -ne 0 ]; then
  cat >&2 <<'WHY'

A symbol above the floor makes the .rpm uninstallable and the AppImage
unportable. Two ways out, in order of preference:

  1. Stop using the symbol. It is usually one call, and usually a float
     variant of a maths function whose double version has been GLIBC_2.2.5
     since forever -- `log10f` was exactly that.
  2. Build in a container whose glibc is the floor, so the compiler binds to
     the older version node.

Do not raise CORDIAL_GLIBC_FLOOR to make this pass.
WHY
  exit 1
fi
