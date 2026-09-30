# ADR-037: The build store's three writers share one lock, and an entry now proves its own bytes

**Status:** accepted, implemented
**Date:** 2026-09-24
**Extends:** [ADR-033](ADR-033-roblox-versions-are-a-keyed-store.md)
**Related:** [ADR-012](ADR-012-profiles-and-instances.md), [ADR-025](ADR-025-fetching-from-a-third-party-mirror.md)

## What this is not

This is not the ADR that makes Cordial's Roblox builds shared instead of
per-profile. **That one is [ADR-033](ADR-033-roblox-versions-are-a-keyed-store.md)
and it shipped on 2026-09-13.** A profile has never held its own copy of an
engine or an APK -- `profiles/<name>/roblox-version` is one line of text naming
a version, and the bytes live once, under `~/.cache/cordial/builds/<version>/`,
shared by every profile on the machine. Measured on the machine this was
written on: `~/.cache/cordial` is 535 MB total, `build/x86_64` (the build in
use) reports 271 MB and `builds/2.738.0.1397` (the same build, kept as a store
entry) reports 384 MB, and the two do not add up to more than the whole because
`base.apk` is one inode with two names -- `store::keep_archives`'s hard link,
working as documented, not a copy. Anyone assessing "is this pnpm-style shared
storage or npm-style per-project duplication" should read ADR-033, not this one.

What is genuinely missing, found while confirming the above, is two things ADR-033
left open or got only partly right:

## Decision

**One lock, not two, and a third path that had none at all.** Three places in
this codebase mutate `~/.cache/cordial/builds/`:

| Caller | Guarded by, before this ADR |
|---|---|
| `provider::obtain_and_install` (an auto-update) | `build_dir().join(".installing")` |
| `provider::obtain_into_store` (the Version page's download) | `store::root().join(".downloading")` |
| `cordial-shell::install::key_into_store` (keying a build Sober or the user already supplied) | **nothing** |

The first two are different lock files on different directories and do not
serialise against each other. The third takes no lock at all. All three end up
calling `store::adopt_current`, and the first two also call `store::prune_in`,
against the one `store::root()` directory every profile shares. A build landing
through the update path while a Version-page download of a different build is
mid-rename, or while a first launch is keying whatever Sober already extracted,
is three processes -- or three code paths in one process across two windows --
free to interleave writes to entries and to the pruning that deletes them, with
nothing stopping it.

This had not caused a reported bug, and the reason is almost certainly ADR-025's
own math: an auto-update and a Version-page download are ordinarily fetching
*different* versions, so they usually land in different directories and the
race is invisible. "Usually different" is not a guarantee, and the shell's path
having no lock at all is not something ADR-033 argued for — it is something
nobody added while writing the other two.

**Fixed by moving the lock inside the primitives, not by fixing the three call
sites.** `store::adopt_current`, `store::prune_in` and `store::remove_in` — the
functions that actually create, rename or delete an entry — now take
`store::root().join(".store.lock")` themselves, for the duration of the call.
`install::file_into_store`, which is the Version page's download and does not
go through `adopt_current` at all, takes the same lock directly. Every one of
the three callers above reaches the store exclusively through one of these four
functions, so none of them had to change, and a fourth caller written later
gets the same guarantee for free rather than having to remember to ask for it —
the same shape of mistake ADR-012 records about the profile lock being "half
implemented" because taking it was left to each caller instead of built into
what everybody already calls.

**Blocking, not refused.** `provider::exclusive` — the lock behind
`.installing` and `.downloading` — refuses instantly and says so, because what
it guards is a fetch the user is watching a progress bar for, and queuing
silently behind someone else's download would look like Cordial hanging.
`store::lock` guards a handful of renames with no network in it, typically
under a second even for a 100 MB engine being hashed (see below), so a second
caller waits rather than being told to try again. The alternative — refusing a
first launch's "key what Sober extracted" step because a Version-page download
happened to be mid-rename — would fail an ordinary launch for a reason nobody
watching it could act on. Waiting a fraction of a second is the honest answer;
refusing is not free just because it is simpler to write.

**Measured**: a test acquires the lock, spawns a thread that calls
`store::prune_in` against the same root, sleeps 200 ms, then releases the lock.
The thread's `prune_in` call is measured taking at least 180 ms — it blocked
for the lock rather than running concurrently with the holder. See
`concurrent_mutations_serialize_on_the_store_lock` in `crates/cordial-update/src/store.rs`.

### A content hash, recorded once, trusted after that

ADR-033's entries are addressed by a version string scanned out of the engine
with a digits-and-dots heuristic. That is the right *name* for a picker — see
ADR-033's own reasoning for why the directory stays version-named rather than
becoming a hash — but it is not what the task that produced this ADR asked for
literally: a store "keyed by content (the APK's signature-verified hash, and
version)". Today nothing records what was actually verified. An entry cannot
be checked against tampering after the fact, and two entries that turned out to
hold byte-identical engines — plausible, since ADR-025 measured Google Play's
split bundle and APKPure's monolithic APK carrying the same signed build in
containers of very different shapes — have no way to be recognised as the same
thing.

**So every entry now carries `.content-sha256`**, a streamed SHA-256 of its own
`libroblox.so`, written by `store::ensure_content_hash`. It hashes the engine
itself rather than the container APK, for the reason the mirror measurement
above gives directly: the container's bytes differ between distributors for
the same signed build (150 MB against 229 MB, ADR-025), so a hash of the
archive would not recognise two copies of one engine as the same thing, which
is the one thing this is for.

**One function, not three call sites.** `adopt_current` is where every one of
this codebase's three install paths already converges to key a build — see
the table above — so `ensure_content_hash` is called from inside it, not from
each caller. `file_into_store` is the one path that does not go through
`adopt_current`, so it calls `ensure_content_hash` itself, once, after an entry
exists at its final name.

**Idempotent, and cheap on the ordinary path.** `ensure_content_hash` reads
`.content-sha256` first and returns it unhashed if it parses; only a build with
none yet pays for a pass over the engine. *Corrected 2026-09-30:* this said
`adopt_current` runs on every launch, which is true of the call and not of the
hashing: once the single-slot path is a link, `adopt_current` returns at its
first check and never reaches `ensure_content_hash`. The hash is taken when a
build is keyed, which is once per build. Verified by a test that writes a hash,
corrupts the engine bytes on disk, and calls `ensure_content_hash` again: the
stale hash comes back unchanged, proving the second call never re-read the
file.

**`None` is the honest answer for an entry from before this shipped**, in the
same spirit as `Entry::loaded_by`. Nothing re-hashes every existing entry on
upgrade; the next time each one is keyed, it gets one. *Corrected 2026-09-30:*
this said that for the current build that was the very next launch. It is not:
an entry the single-slot path already links to is not keyed again, so an entry
from before this shipped has no hash until it is extracted or installed anew.
Nothing reads the hash yet (see below), so nothing depends on it being there.

**`store::find_by_content_hash` is the lookup this buys**, and it is left
unwired. Using it to recognise that a freshly downloaded build is byte-identical
to an entry already kept under a different label, and linking rather than
duplicating, is a real improvement and a real amount of additional surface —
touching both install paths' success handling — that this change does not
make. Recorded as open, the way ADR-033 left its own rotation and per-launch
questions open, rather than silently deferred.

## What was already right and is only being written down

**Migration is idempotent**, and was before this ADR: `adopt_current` run twice
returns `Some` once and `None` the second time, tested since ADR-033. This
ADR's own tests add nothing new here beyond confirming the new lock does not
change that.

**The Flatpak data root needs no special case.** `cache_root()` reads
`$XDG_CACHE_HOME`, and Flatpak sets that variable to the sandboxed
`~/.var/app/<id>/cache` for every app it runs, unconditionally — there is
nothing here to detect or branch on. Confirmed on the machine this was written
on: `~/.var/app/io.github.luohoa97.Cordial/cache/cordial` exists and is a
sibling of the host installation's own `~/.cache/cordial`, with no shared
content between them, which is correct — a Flatpak's sandbox and a host
checkout are different machines from the store's point of view, and each gets
its own.

**Garbage collection was already explicit and already refuses a referenced
entry.** `store::prune_in` runs automatically after every install, bounded by
`store::KEEP`, and never removes the current build or anything any profile has
pinned -- both are in its `protect` list, checked before anything is deleted.
`store::remove_in` is the explicit path, reached from the Version page's remove
button, and refuses the same two things by name rather than silently declining.
Nothing here needed to change; a `gc_unreferenced` sweep that ignores the count
bound was considered and rejected as solving a problem nobody has reported —
`prune_in` already reclaims space automatically, and `remove_in` already lets
somebody reclaim more, explicitly, whenever they choose to.

## What would change this

`store::find_by_content_hash` being wired into an install path, if somebody
measures that two distributors' archives for the same version routinely
produce byte-identical engines and decides the disk saved is worth the extra
branch in `adopt_current` and `file_into_store`. Nothing here blocks it; it is
just not done.
