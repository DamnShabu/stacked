# ADR-021: Everything is a plugin; code is a property, not a category

**Status:** proposed
**Related:** [ADR-003](ADR-003-plugin-isolation.md), [ADR-007](ADR-007-host-resources-are-brokered.md), [ADR-010](ADR-010-plugin-asset-overlays.md), [ADR-013](ADR-013-per-profile-configuration.md), [ADR-014](ADR-014-plugin-registry-and-unpacking.md), [ADR-020](ADR-020-declarative-plugin-preferences.md)

## Decision

There is one installable thing in Cordial and it is a plugin. A texture pack is
a plugin. A flag preset is a plugin. A Discord presence integration is a plugin.
There is no second import path, no "mods" folder beside the plugins folder, no
separate list in Settings, and no `type` key in `plugin.json` saying which sort
of thing this is.

**Whether a plugin contains code is a property read off its manifest, not a
category it declares.** A manifest with no `entry` has nothing to run; a manifest
with no `capabilities` can affect nothing outside its own directory. A plugin
that is both is *data* — assets, flags, preferences — and there is nothing to
sandbox because there is nothing to execute. `entry` therefore becomes optional,
and its absence is the whole signal, in exactly the way ADR-020 made the
`preferences` declaration the whole signal that a plugin has a settings page.

This decision covers five things that turned out to be one thing:

1. asset overlays resolved by interception, on both routes the engine uses;
2. orphan detection, in two flavours that mean different things;
3. precedence displayed rather than implied;
4. consent at install, gated on the plugin containing code at all;
5. hot reload as a development switch, off by default.

## What was measured first, and what it changed

Three questions were settled by observation before any of the above was
designed. Two of them killed a hazard outright and one of them refuted a premise
this ADR was originally briefed with.

### The APK is never mapped, so an intercept is not defeated by offsets

The hazard is real in general: if the client `mmap`s `base.apk` once and reads
each asset as bytes at an offset inside that single mapping, then interposing on
`open` buys nothing, because no asset ever gets an fd of its own.

It does not happen here. `/proc/<pid>/maps` and `/proc/<pid>/fd` were sampled
every 400 ms across **two complete cold launches** of `cordial-run` (`--profile
CordialTest --host-libc --game-activity --join-url roblox://experiences/start`),
136 samples, with `libroblox.so` present in the mapping list in **every one** of
them so it is certain the engine was loaded throughout:

```
apk fds: 0    asset-dir fds: 0    apk maps: 0    cordial/assets maps: 0
```

Zero mappings of `base.apk`. Zero file descriptors on it. Zero mappings of
`~/.cache/cordial/assets`, the extracted content tree. A mmap-by-offset scheme
requires a mapping that persists for the life of the reads; there was never one
to find.

This is passive `/proc` reading of another agent's running client, not a launch
and not a debugger attach. It is a **negative result sampled discretely** and
therefore cannot exclude a mapping created and destroyed inside a 400 ms window;
what it does exclude, conclusively, is the long-lived whole-archive mapping the
hazard depends on. Marked `INFERRED` only in that narrow sense.

The `AAssetManager` route needs no measurement at all, because that route is
*ours*: `android/asset.rs` reads the zip entry with the `zip` crate, decompresses
into a `Vec`, leaks it and hands the engine an interior pointer.
`AAsset_openFileDescriptor` hands back a sealed `memfd`. The engine cannot map
the APK through an API whose implementation never opens it.

### The engine imports no `openat`, so the dirfd hazard does not exist here

The second classic intercept bug — a relative path resolved against a directory
fd, bypassing a resolver that only understands absolute paths — is answered by
the dynamic symbol table rather than by argument. Every path-taking or
size-reporting libc symbol `libroblox.so` imports:

```
access  fdopen  fopen  fstat  ftruncate  lstat  mmap  open  opendir  readlink  realpath  stat
```

No `openat`. No `fstatat`, no `statx`, no `open64`, no `__xstat`. There is no
dirfd-relative call in the engine's import list to bypass anything, so
canonicalisation has one entry point by construction rather than by discipline.

This is a fact about **this build** and it must be re-checked when the build
moves. `docs/analysis/undefined-symbols.tsv` already generates the stub table
from exactly this list, so the check costs one `comm`, and the machinery in
AGENTS.md's "Missing symbols" section is the machinery for it.

### `fstat` needs no resolver, and this is a real result rather than an omission

`fstat` **is** imported and is **not** in `native/system_paths.cpp`'s intercept
table. The received wisdom is that this is the bug that eats the week: intercept
`open` but not the size call, and the client allocates for the original file's
size and reads a larger overlay into it.

That failure needs the size and the bytes to come from different files. It
cannot happen through `fstat`, because `fstat` takes an **fd** — the fd our
`open` already returned, pointing at the overlay file itself. It reports the
overlay's size because it is looking at the overlay. Leaving `fstat` alone is
not an oversight to fix; intercepting it would mean intercepting a call that is
already correct.

The hazard is entirely in the **path-taking** size and existence calls, and
those are `stat`, `lstat` and `access` — all three of which are already in the
intercept table, alongside `open`, `fopen`, `opendir`, `realpath` and
`readlink`. So the invariant has a precise statement:

> **Every path-taking call in `cordial_system_symbols`'s table consults one
> resolver, or none of them do.** `fstat` is exempt because it is not
> path-taking. `mmap` is exempt for the same reason.

Eight functions, one table, one resolver, and the table is the checklist. A
symbol added to that table without being routed through the resolver is the
regression to guard against, and the test for it compares the two lists rather
than trusting a reviewer.

### Bloxstrap mods are only partly portable, and the brief overstated it

The premise handed to this work was that the Android APK's tree is
`assets/content/{textures,sounds,fonts,sky,models,avatar,…}`, that a Bloxstrap
mod mirrors the Windows client's `content/…`, and that the two are therefore the
same subtree under a different prefix. Checked against the actual APK on this
host — 1835 entries under `assets/` — that is true of the prefix and **not true
of the contents**.

The tree is bigger than the premise:

| subtree | entries |
|---|---|
| `content/` | 1223 (`textures` 987, `fonts` 118, `avatar` 56, `configs` 34, `sky` 10, `sounds` 10, …) |
| `ExtraContent/` | 552 (`textures/ui` 485, `LuaPackages/Packages` 53, `models`, `places`, `translations`) |
| `android/` | 45 (`textures` 42, `fonts`, `terrain`, `shared_compression_dictionaries`) |
| `fonts/`, `shaders/`, `ssl/`, `dexopt/`, `com/` | 12 |

A mod built against Windows `content/…` addresses at most the first row. The 485
UI textures under `ExtraContent/textures/ui` are invisible to it, and
`ExtraContent` is not a Windows concept.

Filenames diverge inside the matching directories too. Probing the canonical
Bloxstrap mod targets against the real archive:

| Bloxstrap path | on Android |
|---|---|
| `content/textures/Cursors/KeyboardMouse/ArrowCursor.png` | **present** |
| `content/textures/Cursors/KeyboardMouse/ArrowFarCursor.png` | **present** |
| `content/fonts/SourceSansPro-Regular.ttf` | **present** |
| `content/sounds/action_jump.mp3` | **present** |
| `content/sounds/ouch.ogg` | **absent** — Android ships `content/sounds/oof.ogg` |
| `content/sky/sky512_bk.tex` | **absent** — Android ships `content/sky/*.dds` and `moon.jpg`/`sun.jpg` |
| `content/textures/ui/Lua/Graphic/PlayerlistExpansionArrow.png` | **absent** |

So cursor mods and font mods drop in unchanged, and the single most famous
Bloxstrap mod in existence — replacing the death sound — **silently does
nothing**, because it ships `ouch.ogg` and this build reads `oof.ogg`.

That is the finding that makes orphan detection load-bearing rather than a
nicety. Without it, "Bloxstrap mods work in Cordial" is a claim that is
two-thirds true and fails in the case users will try first, with no error
anywhere. **Cordial must not advertise Bloxstrap compatibility without shipping
the report that says which files landed.** The two are one feature.

## The instrument comes first

`CORDIAL_TRACE_ASSETS=1` already printed one line per lookup to stderr. It is
promoted here to the primary instrument, because one cold launch plus one game
join yields the **ground-truth list of every asset path this build actually
asks for** — which is the missing half of the Windows-to-Android mapping table
above, and is derived rather than guessed.

```
CORDIAL_TRACE_ASSETS=1 XDG_DATA_HOME=~/.cache/cordial-agent-x just client --run 60
sort -u ~/.cache/cordial-agent-x/cordial/asset-trace.log
```

The recorder is separate from the stderr tracing and always on, because it costs
one hash insert per *distinct* name and answers questions the log cannot: it is
a set, it survives to the end of the run, and it is what both orphan signals are
computed against. AGENTS.md's rule about paying for nothing is satisfied by it
being a set rather than a log — a name already seen costs a lookup and no
allocation.

This ordering is deliberate and is this project's standing rule. Every ad-hoc
score taken here before its instrument was checked against a control turned out
to be constant across all runs.

## Mechanism

### One resolver, two routes

**Route 1, `AAssetManager`** — Cordial's own code, already the mechanism ADR-010
describes, and the route that carries textures, sounds, fonts and models. This
is covered, in Rust, in `android/asset.rs`.

**Route 2, the real filesystem** — the engine is handed
`setAssetFolder <cache>/assets/content` and reads through libc. This is the
route ADR-010 explicitly left out of scope, and it is what serves anything the
engine resolves by path rather than by asset name — `ssl/cacert.pem` being the
one already known to matter.

The two routes share **one resolver and one index**. They must, or an overlay
applies to a texture and not to the same texture reached the other way, which is
a bug nobody would guess from the symptom. The resolver is Rust, in
`android/asset.rs`; the libc route reaches it through a C ABI entry point that
`native/system_paths.cpp` calls from the eight path-taking functions already in
its table.

A path arriving on route 2 is turned into an asset-relative name by stripping
the extracted-assets prefix, and only then looked up. A path outside that prefix
is not an asset and is forwarded untouched — the overlay must never become a
general filesystem redirect, which is the same line ADR-007 draws between an
effect and a channel.

### Writes are refused, and this is decided now rather than discovered later

`O_WRONLY`, `O_RDWR`, `O_CREAT`, `O_TRUNC` or `O_APPEND` against a path that
resolves to an overlay file **do not get the overlay**. They are forwarded to
the real path underneath, untouched.

The three candidate behaviours were: deny the open, copy-on-write into the
instance root, or pass through. Pass through wins because the overlay is
*read-only by definition* — ADR-010's entire claim is that nothing is ever
written into the APK or into anything extracted from it, and handing a writable
fd to a plugin's file would make an overlay a place the engine can scribble,
which is neither non-destructive nor something the plugin author consented to.
Denying would break any engine write to a path that merely happens to collide
with an overlay name. Copy-on-write needs a manifest of what was copied and
becomes exactly the half-restored-tree problem ADR-010 declined to build.

So: reads resolve to the overlay, writes go to the original. Stated in the
resolver, tested, and written here so nobody has to rediscover it from a
corrupted cache.

### The index, built once

Resolution today canonicalises and stats every layer on every lookup. That is a
stat storm on the hottest path in asset loading, and it gets worse with each
overlay installed.

Instead: each root is walked **once**, producing a map from asset-relative name
to the winning layer, with the layers merged in precedence order at build time
so a lookup is one hash probe. The index is held behind an `Arc` and swapped
wholesale; a lookup clones the `Arc` and never blocks on a writer. A name absent
from the map is absent from every overlay — that is the negative answer, and it
costs the same probe, so there is no separate negative cache to keep coherent.

Precedence is unchanged from ADR-010 and from `flags.rs`: **the user's root
beats every plugin's, and among plugins the last registered wins.** Building the
index in that order means the map already holds the winner and the resolver has
no ordering logic in it at all.

### A shipped overlay directory needs no capability

This fell out of implementation and is the piece that makes "a texture pack is
a plugin" true rather than aspirational, so it belongs in the decision rather
than in a comment.

A plugin's `overlay/` directory is registered by Cordial when the plugin is
installed and enabled. It needs no `assets.override` grant, and this is not a
hole — it is the precedent `flags.rs` already set. `flags::collect` reads every
enabled plugin's own `flags.json` **with no capability check at all**, because a
static file a plugin ships is not a request a process is making. It is what the
plugin *is*, and installing and enabling it is the consent.

`assets.override` gates something genuinely different: a **running** plugin
asking Cordial to register a directory of its choosing at runtime. That is a
request from a process and is brokered like every other one, per ADR-007.

Confusing the two would mean a texture pack that cannot work without a
permission prompt for a process it does not have — and would put us back to
needing a second import path for the thing that has no code, which is the
decision this ADR exists to avoid. It also means the consent rules hold
together: a data-only plugin declares no capabilities, so it gets no prompt,
so the prompt keeps its meaning.

The precedence rule is `flags::collect`'s too, including its refusal to let a
user plugin shadow a first-party id: system root first, user root second,
sorted within each. Registration order is precedence order among plugins, so
the shadow report is quoting a fact rather than a filesystem accident.

### Two orphan signals, which mean different things

**Stale** — an overlay path with no counterpart anywhere in the APK's asset
tree. Computed by diffing the index against the archive's entry list; needs no
running client and is the check to run at install time and after a Roblox
update.

> `retro-ui: 7 files no longer match anything in client 2.7xx`

**Unrequested** — an overlay path that does exist in the APK but that the engine
never asked for during a session. Computed by diffing the index against the
recorder's set.

These are not the same claim and must not be reported as one. Stale means *this
file can never apply*: the name is wrong, or the build removed it — this is the
signal that catches `ouch.ogg`. Unrequested means *this file did not apply
today*: possibly because the feature it decorates was never opened, possibly
because the session was short. **Stale is a defect; unrequested is an
observation**, and the report says so in those terms rather than presenting a
count.

Unrequested carries an honest caveat that is stated wherever it is shown: a
60-second launch that never joins an experience will call almost everything
unrequested. It is a strong signal after a real session and a misleading one
after a smoke test.

### Shadowing is shown, never implied

When two layers provide the same name the loser is recorded next to the winner,
and the report names both:

```
user:my-fonts wins over plugin:retro-ui   content/fonts/SourceSansPro-Regular.ttf
```

This falls out of building the index in precedence order for free, and it is the
answer to "why did this file not change" — which is otherwise indistinguishable
from "the overlay is broken", and is the question that will be asked most.

### Hot reload: a development switch, off by default

Interception makes reload cheap: rebuild the index and swap the `Arc`. Nothing
is remounted and nothing is copied.

It is off by default and enabled with `CORDIAL_OVERLAY_WATCH=1`, for the reason
this codebase has a standing rule about: **watching a directory tree costs
wakeups**, and a texture pack that nobody is editing must not cost a single one.
The default reload is explicit — a call, from the Settings page or the
development MCP — and the watcher is what an author turns on while iterating.

Two caveats that are honest limitations and are documented rather than papered
over:

**A reload affects the next load, not what is on screen.** `Manager::read`
caches decompressed bytes for the life of the process and hands the engine an
interior pointer that must stay valid; ADR-010 already records this. Swapping
the index does not retroactively change what a held pointer points at. A reload
therefore applies to assets loaded after it, which for most of the interesting
ones means the next launch. Saying "reloaded" and showing the old texture is
precisely the stub-that-lies failure AGENTS.md forbids, so the reload reports
how many *cached* names it could not affect.

**The watcher is a development instrument and is not load-bearing.** If it
misses an event the fix is to press reload. Nothing in Cordial's correctness
depends on it firing.

### What the implementation settled that the design did not

**The stale check runs without a client.** `cordial-run --check-overlays` sets
the APK, registers the overlay stack, diffs it against the archive's entry list
and exits before the engine is loaded. That matters more than convenience: it
is the only orphan signal that can be produced without playing, and it is the
one that is a defect rather than an observation.

**The build is named by scanning, and says nothing rather than guessing.** "7
files no longer match anything" is not a useful sentence without a version
beside it, because the user's next question is always "since when". The version
is read by scanning the APK's binary `AndroidManifest.xml` string pool for a
token shaped like a Roblox version — three dotted numbers with a three-digit
middle — rather than by writing an AXML parser for one field. The narrow shape
is deliberate: a looser pattern picks an AndroidX library version out of the
same pool and reports it as the client's, and a confidently wrong version is
worse than none. An ambiguous scan falls back to naming the archive.

**The watcher polls.** An inotify watch must be re-armed per directory as
subdirectories appear, and getting that wrong presents as a reload that
silently stops working. A signature over the roots — file count and newest
mtime — taken twice a second while an author is editing is small enough not to
matter and simple enough to be obviously right. It misses an edit that restores
the file's timestamp, and the answer to that is the explicit reload, which is
the default anyway.

## Consent, and why the wording is the whole job

A prompt saying *"this plugin wants to run code, allow?"* is worse than no
prompt. It appears for everything, it is answered yes by everyone, and it
trains the user to dismiss the one that mattered. ADR-007 already supplies the
better vocabulary: capabilities are **named effects**, and an effect is
something a person can actually judge.

Three rules.

**1. A plugin with no code gets no prompt at all.** No `entry`, nothing to run;
no capabilities, nothing it could reach. A texture pack installs silently, the
same way copying a file into a folder does, because there is nothing to ask
about and asking anyway is what makes the third prompt meaningless. This is the
direct payoff of "code is a property, not a category" — the prompt is gated on
reading the manifest, not on which import button was pressed.

**2. Code starts disabled regardless.** The prompt is not the gate; the toggle
is. Consent and enablement are separate acts, and `enablement.rs` already argues
this in the other direction — that disabling must not cost the approvals. The
same separation run forwards means approving what a plugin *may* do does not
start it doing it. A user who clicks through a prompt has still not started
anything.

This is a **change to the current install path**, and it is a correction rather
than an addition. `plan_confirmation_text` today tells the user that installing
"grants each plugin below exactly what it requests", and the handler then does
so. Combined with `enablement`'s "absence means enabled", a marketplace install
today produces a plugin that is granted and running the moment the dialog
closes. That is one act, not two, and it contradicts both this ADR and the
spirit of ADR-003's default deny. An installed plugin with code is written into
`plugin-enabled.json` as `false`, explicitly, and the user turns it on.

**3. `flags.write` is spelled out honestly.** ADR-020 records what it actually
is, and the wording it deserves follows from that record rather than from the
capability's name:

> **Change how Cordial itself renders and behaves.** Sets Roblox FastFlags, and
> also Cordial's own settings including the graphics backend
> (`CordialGraphicsBackend`) and present mode (`CordialPresentMode`). Takes
> effect at the next launch. Your own choices in Settings still win.

"Change some Roblox settings" would be technically true and materially
misleading, which is the standard `webview_policy.rs` already holds a URL to.
Every capability gets a sentence of this kind — what it *does*, in the second
person, with the honest edge of it included and its limit stated. The sentences
live beside the `Capability` enum so a variant cannot be added without one, in
exactly the way `name`/`parse`/`all` are already held together by a test.

`assets.override` gets the sentence ADR-010's "what is still refused" section
earned: replacing a mesh is not a cosmetic change, and the user is the one
deciding.

### Built-in plugins: the question is due when one is switched on (2026-10-05)

Upstream Cordial found its Settings window putting one permission dialog per
built-in over whatever tab was open, `fps-flex` included although it ships off,
and moved the question to the moment it is due: a built-in that is switched on
and has something requested but not granted. Stacked has no dialog to stack
(ADR-043): consent is `stacked plugins grant`, one capability at a time. What
carries over is the timing. `stacked plugins enable ID` names the capabilities
that plugin still lacks on the profile, with the command for each, and says
nothing when everything is granted or nothing was requested. Nothing is granted
by it; default deny is unchanged.

## Consequences

**Accepted:** `entry` becomes optional in `plugin.json`, and a manifest with
neither `entry` nor `capabilities` is a valid, complete plugin. Every existing
manifest is unaffected, because an absent optional key is exactly what
`#[serde(default)]` already does for `capabilities`, `version` and
`preferences`.

**Accepted:** a plugin can grow code later without changing what it is. A
texture pack that adds an `entry` in version 2 becomes a plugin with code, gets
a consent prompt at that upgrade, and starts disabled. The identity, the
directory, the id and the user's other settings all survive, because none of
them ever encoded the distinction.

**Accepted:** one list in Settings, which will be long. A user with thirty
texture packs and two integrations sees thirty-two rows. That is the honest
presentation of what they installed, and filtering is a UI affordance to add
later rather than a reason for a second list — two lists is how the two import
paths grow back.

**Accepted:** the libc route makes Cordial's resolver load-bearing for
`ssl/cacert.pem`. A resolver bug there does not produce a wrong texture, it
produces a TLS failure three layers from anything mentioning certificates —
which is a failure this project has already paid for once. The prefix check
therefore fails *closed* to the original path: anything the resolver is not
certain about is forwarded untouched.

**Not yet wired, and stated rather than implied:** the filesystem route's
resolver, its write rule and its C ABI entry point (`cordial_overlay_resolve`)
are implemented and tested, and `native/system_paths.cpp` does not call them
yet. Its table holds `stat`, `lstat`, `access`, `opendir`, `realpath`,
`readlink`, `fopen`, `statvfs` and `open`, and all of them must be routed
together — the invariant above is that they consult one resolver or none of
them do. Until that change lands, only the `AAssetManager` route is live, which
is what covers the textures, sounds, models and fonts people ask to override.

**Rejected: overlayfs.** The assets are zip entries inside `base.apk`, so there
is nothing on a filesystem to overlay without extracting the whole archive
first; and Flatpak cannot mount overlayfs unprivileged in any case. Interception
also gives the trace, the orphan signals and the shadow report, none of which a
mount can provide.

**Rejected: a `type: "assets"` manifest key.** It is the category this ADR
exists to not have. Two facts that can disagree eventually do, and here the
disagreement would be a data-only plugin that declares itself code, or an
`entry` that never runs because a key says it should not.

## What would change this

Evidence that the engine reaches assets by a route neither covered here — a
mapping of the archive taken and released inside a sampling window, or a build
that starts importing `openat` — would not change the decision but would change
the mechanism, and the mechanism section would need rewriting rather than
patching.

Evidence that people look for a texture pack somewhere other than the plugin
list would be an argument about presentation, not about whether the two things
are one kind.
