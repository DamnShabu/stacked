# Plugins

A plugin is a directory containing `plugin.json` and its entry module. Stacked
discovers them under `~/.local/share/cordial/plugins/`.

Stacked runs Cordial's plugin system unchanged, so a plugin written for one
runs on the other, and where this document says "Cordial" it means that
system: the host your plugin talks to.

```json
{
  "id": "flag-inspector",
  "name": "Flag Inspector",
  "version": "1.0.0",
  "entry": "main.ts",
  "capabilities": ["flags.read", "log"],
  "dependencies": { "some-other-plugin": "^1.2.0" }
}
```

`capabilities` is what the plugin **requests**. What it actually gets is what you
approved, in the profile you approved it in, one capability at a time:

```bash
stacked plugins grant flag-inspector flags.read
stacked plugins grant flag-inspector log
```

That writes `~/.local/share/cordial/profiles/<profile>/plugin-grants.json`:

```json
{ "flag-inspector": ["flags.read", "log"] }
```

Default deny — a plugin missing from that file gets nothing, and a capability it
asked for but you did not grant is refused at the point of use, by name, so the
author can tell "not allowed" from "broken".

**Grants are per profile, and that is deliberate.** Approving a plugin in a
profile you made to try something out does not approve it in the profile you
actually play on. The plugin is installed once for the machine; what it is
allowed to do is decided per account. An existing
`~/.config/cordial/plugin-grants.json` from before this changed is moved into the
first profile that looks for one, and every other profile starts at default deny
— see [ADR-013](../docs/adr/ADR-013-per-profile-configuration.md).

**None of this needs a restart to reach a running client**
([ADR-038](../docs/adr/ADR-038-plugin-hot-swap.md)). Installing, updating,
removing, enabling, disabling or granting a plugin in this profile is
noticed within a second or two and starts, stops or restarts exactly that
plugin. An update that changes what your manifest requests never lets a
restarted process keep a capability the new manifest stopped asking for,
whatever the grants file still says — see `intersect_for_restart` in
`crates/cordial-plugins/src/reconcile.rs`. The one thing that still waits for
the next launch is a `flags.write` layer, because `FFlag`/`FInt`/`FString`
are read once at startup regardless (ADR-005) — see "Flags have two
lifetimes" below.

## Versions and dependencies

`version` is a semantic version, `major.minor.patch`. It is optional — a plugin
without one still loads, so nothing written before this existed stopped working
— but a plugin cannot be published or depended upon without it.

`dependencies` names **other Cordial plugins**, by id. It is not npm's field
under another roof: if you also need a JavaScript package, that is your
`deno.json`'s business and Cordial neither reads nor validates it. One key
cannot honestly mean both, and you may well need both.

A requirement is written in one of exactly two forms, and a bare version is
refused:

| | |
|---|---|
| `"=1.2.0"` | that version and nothing else |
| `"^1.2.0"` | anything compatible with it |

`"1.2.0"` on its own means *exactly that version* in npm and *`^1.2.0`* in
Cargo. Rather than pick one and be wrong for half of you, Cordial refuses it and
the error names both forms. `>=`, `~`, `*` and comma-separated lists are refused
too — every operator in the language is one you have to understand before you
can tell what an install will do.

A dependency is installed before the plugin that needs it, and **started**
before it too. That matters: `events.subscribe` only matches types somebody has
already declared, so a plugin that subscribes to another's events needs that
other one running first. It is the same order, from the same graph, on purpose.

Cordial refuses, by name, on a missing dependency, a requirement nothing
satisfies, a cycle, and two dependents needing incompatible versions of one
plugin — there is one directory per plugin id, so exactly one version can win.

## Publishing: an archive and an index entry

A distribution archive is a `.tar.zst` of your plugin directory's **contents**,
with `plugin.json` at its root:

```bash
tar --zstd -cf flag-inspector-1.0.0.tar.zst -C plugins/flag-inspector .
sha256sum flag-inspector-1.0.0.tar.zst
```

An index is one static JSON file listing what is available — see
[`index.example.json`](index.example.json), whose URLs and hashes are
deliberately fake. It is meant to be served straight out of a git repository, so
that what is on offer is a diff somebody can read, anyone who dislikes it can
fork it, and hosting it costs nothing.

**Who runs an index, and who decides what goes in one, is not settled.** Nothing
in Cordial names a URL and nothing assumes there is only one. A curated list and
an index you host yourself are the same file in the same format; where two of
them publish the same version of the same plugin pointing at different bytes,
Cordial refuses rather than picking, because picking is the decision nobody has
made yet.

**Index signing is not implemented.** The intended scheme is a detached minisign
signature beside the index, checked before the JSON is parsed. Until that
exists, an index is exactly as trustworthy as wherever you got it. The content
hash on each entry protects the *download* against a mirror serving different
bytes; it cannot protect against a tampered index, because a tampered index
carries a matching hash.

What the index says a plugin requests is checked against the archive when it is
unpacked. An entry advertising `log` cannot ship a manifest asking for
`assets.override` — that refuses the install, because the capabilities you
approved were the ones you were shown.

## Installing does not make a plugin safe

Everything above answers one question: *are these the bytes that were
published, unpacked without escaping the directory they were meant for?* None of
it answers whether those bytes deserve your trust. A hash proves provenance, not
intent.

**The capabilities are still the boundary, and they are still yours to grant.**
Installing a plugin that depends on another does not grant the other one
anything: Cordial refuses a plan where any plugin in it — including one that
arrived only as somebody else's dependency — asks for something you have not
granted it by name. Being listed in an index is not review, and should not be
read as any.

## What a plugin cannot do

It runs as a separate Deno process started with **no permissions**: no file,
network, environment or subprocess access. That is a second, independent layer
under Cordial's own capability broker, so a plugin cannot reach the machine even
if the broker had a hole in it.

There is no script execution against Roblox and no memory access. Those are
absent from the API rather than disabled — see
[ADR-001](../docs/adr/ADR-001-in-process-hooking.md) and
[ADR-003](../docs/adr/ADR-003-plugin-isolation.md).

A plugin granted `assets.override` **may** provide files that resolve in place
of Roblox's own for the same name — a non-destructive overlay, never a write
into the APK or anything extracted from it. See
[ADR-010](../docs/adr/ADR-010-plugin-asset-overlays.md) for what that does and
does not permit: it covers cosmetic substitution, and it explicitly does not
protect you from a plugin that overlays a gameplay-affecting asset such as a
collision mesh. That is the same trust decision you already make in approving
the capability at all.

## Host resources are brokered, never handed over

A plugin never receives a socket, a D-Bus connection or a file descriptor, and
installing one can never widen Cordial's Flatpak permissions. Where a capability
needs a host resource, Cordial holds the permission and performs the effect.

Discord Rich Presence is the example: `presence.set` takes a presence payload.
Cordial owns the connection to Discord's IPC socket — your plugin never learns
where it is, cannot read Discord's state, and cannot send anything else down it.

The reasoning is in [ADR-007](../docs/adr/ADR-007-host-resources-are-brokered.md),
and the short version is that a Flatpak permission is app-wide and permanent
while a capability is per-plugin and revocable. If installing a plugin could add
a permission, uninstalling it would not take the permission away.

This also means a resource Cordial does not already broker needs a change to
Cordial rather than to your manifest. Slower on purpose: the manifest is the one
place anyone can read the whole sandbox, and it should stay true.

To keep that rare, the common effects are already brokered — `presence.set`,
`notify.send` and `url.open` (`http`/`https` only). If you need one that is not
here, open an issue: a broker is a payload type and an effect, so adding one is a
small change rather than a redesign. If a proposed broker *cannot* be small, that
usually means the capability is too broad and wants splitting.

## Settings: Cordial keeps them, your plugin never touches the file

A plugin runs with no file access, so it has nowhere of its own to remember
anything. `settings.read` and `settings.write` give it one without giving it a
path: Cordial owns `<profile>/plugins/<your-id>/settings.json` and your plugin
exchanges a JSON document.

The document arrives unasked, in the handshake, so the usual case costs no round
trip:

```ts
// A push has no id; the handshake is the one with this event name.
if (msg.id === undefined && msg.event === "cordial/init") {
  const settings = msg.payload.settings;   // your document, {} if you have none,
                                           // null if you were not granted settings.read
}

await call("settings.set", { settings: { panel: "flags", opened: 4 } });
const mine = await call("settings.get");   // takes no arguments — see below
```

`settings.set` replaces the whole document rather than merging, so you can remove
a key you have stopped using. It must be a JSON object, and it is capped at a
megabyte: this is configuration, not a data store.

**Scoped to your own id, structurally.** Neither call takes a plugin id, because
Cordial already knows which process is on the other end of the pipe. Naming
another plugin in your params is not an error and is not honoured — you get your
own document. There is no field to set to somebody else's name, which is the same
defence `events.declare` uses for namespaces.

They are two capabilities, not one, for the reason `events.declare` and
`events.publish` are two: a plugin that only reads its configuration should not
have to be trusted to rewrite it.

## Preferences: you declare them, Cordial draws them

The section above is your plugin's own scratch document. This is a different
thing: **settings a person sets**, whose answers Cordial owns.

Declare them in `plugin.json`. There is no capability and no other manifest
key to set. **Under Cordial they became a page in its Settings window, and
Stacked has no page for them.** It has no window of its own
([ADR-043](../docs/adr/ADR-043-the-launcher-is-a-command-line.md)) and no
command for preferences yet, so a user sets them by writing
`<profile>/plugins/<your-id>/preferences.json` by hand, a flat object of your
keys. Your plugin reads them exactly as below either way, and until someone
writes that file it gets your defaults.

```json
{
  "id": "example",
  "entry": "main.ts",
  "capabilities": ["settings.read"],
  "preferences": [
    { "key": "loud", "type": "bool", "title": "Be loud",
      "description": "Shown under the title.", "default": false },
    { "key": "level", "type": "int", "title": "Level", "default": 3,
      "minimum": 1, "maximum": 10, "step": 1, "group": "Tuning" },
    { "key": "mode", "type": "choice", "title": "Mode", "default": "slow",
      "group": "Tuning",
      "options": [ { "value": "slow", "label": "Slow" },
                   { "value": "fast", "label": "Fast" } ] },
    { "key": "note", "type": "text", "title": "Note", "default": "" }
  ]
}
```

| `type` | the row Cordial draws | its own keys |
|---|---|---|
| `bool` | a switch | `default` |
| `int` | a spin box | `default`, `minimum`, `maximum`, `step` |
| `choice` | a drop-down | `default`, `options` of `{value,label}` |
| `text` | an entry | `default` |

Every field takes `key`, `title`, and optionally `description` and `group`.
Fields sharing a `group` become one group on Cordial's page, in the order the
groups first appear; ungrouped fields come first.

Reading them is the same shape as settings, and needs the same `settings.read`:

```ts
if (msg.id === undefined && msg.event === "cordial/init") {
  const prefs = msg.payload.preferences;   // every declared key, always
}
const prefs = (await call("preferences.get")).result;
```

**The document you get is always complete and always valid.** Every key you
declared is present, and every value fits the declaration you wrote — so no
`?? default` and no range checks in your code. A value saved against an older
version of your manifest that no longer fits falls back to the current default,
and Cordial says so in its log rather than silently.

**There is no `preferences.set`, and there is not going to be one.** These
answers are the user's. A plugin that could rewrite them could set them to
whatever it liked and have the page show the result back as though the user had
chosen it. Your own state goes in `settings.json`, which is yours to replace.

**Your words are drawn as text, never as markup.** Titles, descriptions, group
names and option labels are shown literally; a control character anywhere in
them refuses the whole plugin at install rather than being stripped quietly.

**Why you cannot draw the page yourself.** GNOME Shell extensions can, because
they run inside the shell's own process. Your plugin does not: it is a separate
sandboxed process with no display and no toolkit, and a plugin able to draw in
the game window could draw something indistinguishable from Roblox's own
sign-in screen. See
[ADR-020](../docs/adr/ADR-020-declarative-plugin-preferences.md), which also
lists what the declarative form gives up and what is planned to be added to it.

## Plugins may declare their own events

`events.declare` registers an event type under your plugin's own namespace —
you provide a bare name, Cordial prefixes it with your plugin id, so
`flag-manager` declaring `profile-changed` gets `flag-manager/profile-changed`
back, never a bare `profile-changed`. `events.publish` broadcasts on a type
you declared; `events.subscribe` receives events, including ones other
plugins declared.

These are three separate capabilities on purpose (ADR-006). Declaring and
publishing are split because a plugin that could publish on any string it
liked could impersonate another plugin's events, and a subscriber would have
no way to tell — declaring first makes a type's origin a fact the registry
checks, not a claim a plugin makes about itself. Subscribing is broader than
publishing: a plugin that only reacts to something should not have to be
trusted to speak.

Subscribing filters at the point you subscribe, not on every event that
arrives — which means you can only subscribe to a type someone has already
declared. If you depend on another plugin's events, that dependency has to
have started first, the same ordering [ADR-006](../docs/adr/ADR-006-plugin-events-and-first-party.md)
already describes for first-party plugins.

Core event types are never available to publish on — only Cordial can speak
for what Cordial did.

## Flags have two lifetimes

`flags.write` contributes flags that take effect at the **next launch**. That
one works, and it is the one to use.

`flags.write.dynamic` is meant for the `DFFlag`/`DFInt`/`DFString` families,
which are the only ones that can change while the client runs — the static
families are read once at startup and cannot be changed live at all. **It is not
implemented, and it is not waiting to be.** `flags.setDynamic` falls to the
host's catch-all and comes back `flags.setDynamic is not implemented yet`, every
time, on purpose: a live write into the running engine's own `DFFlag` table
needs in-process access to the Roblox process, which
[ADR-001](../docs/adr/ADR-001-in-process-hooking.md) and
[ADR-003](../docs/adr/ADR-003-plugin-isolation.md) rule out permanently. It is a
capability whose effect has nowhere to live, not a gap somebody will fill.

They are separate capabilities so that an API call cannot silently do nothing,
and that is still why the refusal above is an error rather than a success. See
[ADR-005](../docs/adr/ADR-005-flag-service.md).

## Examples

[`flag-inspector/`](flag-inspector) reports which flag overrides are in effect
and where each came from, then deliberately attempts a write it was not granted
so the refusal is visible.

[`discord-presence/`](discord-presence) is first-party — it ships with
Stacked, as it did with Cordial — and is still an ordinary plugin: same manifest, same grants, same
isolation ([ADR-006](../docs/adr/ADR-006-plugin-events-and-first-party.md)
is explicit that "built in" and "a plugin" are not opposites). It listens for
client lifecycle events and keeps Discord Rich Presence in step with them,
through `presence.set`/`presence.clear`. Its own source says plainly that the
lifecycle push does not yet carry which game is running, because that needs
work in `cordial-runtime` this plugin does not touch, and that its default
Discord application id is Cordial's.
