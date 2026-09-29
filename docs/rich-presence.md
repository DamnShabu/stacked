# Discord Rich Presence

Stacked ships Cordial's Discord Rich Presence plugin, in
[`plugins/discord-presence/`](../plugins/discord-presence). It is first-party in
the sense that it comes with the project and in no other sense: an ordinary
`plugin.json`, ordinary grants, the same isolation as anything you write
yourself — [ADR-006](adr/ADR-006-plugin-events-and-first-party.md) is
explicit that "built in" and "a plugin" are not opposites, and the project's
own features are built this way so the API has to be good enough for them. It
requests exactly four capabilities, `lifecycle.read`, `presence.set`,
`settings.read` (to read its one preference) and `log`, and holds nothing else.

What it does is small. It subscribes to the client's lifecycle, publishes a
presence when the client launches, and clears it on `shutdown`. A game that
describes itself over BloxstrapRPC replaces the lines it sets.

**It never learns where Discord's socket is, and that is the point.** The
plugin sends a payload — an application id, `details`, `state`, timestamps and
image keys — and the client does the rest: searching `discord-ipc-0` through `-9`
and the nested path Discord's own Flatpak uses, performing the handshake, and
writing the frames. The payload is a closed struct that refuses any field
Discord does not define, so nothing a plugin invents crosses the wire, and
`details` and `state` are refused past Discord's own 128-character limit — the
author hears that from the call rather than from Discord quietly dropping the
whole activity. A plugin cannot read Discord's state and cannot send anything
else down the connection.

That is [ADR-007](adr/ADR-007-host-resources-are-brokered.md) rather than
a detail of this one plugin. A Flatpak permission is app-wide and permanent
while a capability is per-plugin and revocable, so if installing a plugin could
add a permission, uninstalling it could not take one away. The client holds the
permission and performs the effect; the plugin sends a payload.

## Turning it on

Plugins are discovered under `~/.local/share/cordial/plugins/`, one directory
each, so installing this one is a copy — and the same `XDG_DATA_HOME` remap
described for `flags.json` in [`docs/fastflags.md`](fastflags.md) applies
inside the Flatpak:

```bash
cp -r plugins/discord-presence ~/.local/share/cordial/plugins/
```

Installing is not approving. Grants are default deny and belong to the profile,
so the plugin gets what you grant it and nothing else:

```bash
stacked plugins grant discord-presence lifecycle.read
stacked plugins grant discord-presence presence.set
stacked plugins grant discord-presence settings.read
stacked plugins grant discord-presence log
stacked plugins enable discord-presence
```

Those write `~/.local/share/cordial/profiles/<profile>/plugin-grants.json`,
which you can also edit by hand. A plugin with no grants is reported at launch
and not started, and a capability that was requested but withheld is named —
so an author can tell "not allowed" from "broken". `stacked plugins` lists
what is installed, what each one requests and what it has been granted.

## What it does not do yet

**Discord shows "Playing Cordial", not Stacked.** The default application id in
`main.ts` is upstream Cordial's registered Discord application, and Stacked has
none of its own. The plugin's `client_id` preference takes another
application's id, and with no Settings page to set it in, that means writing
`{ "client_id": "<digits>" }` to
`~/.local/share/cordial/profiles/<profile>/plugins/discord-presence/preferences.json`.

**The lifecycle push carries no payload**, so unless the game describes itself
over BloxstrapRPC the activity names only the application, with an elapsed
timer, rather than the experience.

**This section used to say nothing reaches Discord in an actual session, and
the code no longer agrees.** It said the client's plugin host,
`crates/cordial-runtime/src/plugin_host.rs`, answered everything but
`settings.*`, `flags.*` and `log.write` with `not implemented yet`, and that
nothing outside a test pushed a lifecycle event. That host now answers
`presence.set`, `presence.clear` and `lifecycle.subscribe`, and `load.rs`
publishes `client.launch` and `client.shutdown`. That is read from the
source, not observed in a session, so it is `INFERRED` that presence now
arrives. The broker, the payload validation and Discord's framing are covered
end to end by `crates/cordial-plugins/tests/discord_presence_plugin.rs`
against a stand-in Unix socket.
