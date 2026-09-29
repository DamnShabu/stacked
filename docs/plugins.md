# Installing plugins

## Plugins need Deno, and you install it

Plugins are TypeScript run under [Deno](https://deno.com)
([ADR-008](adr/ADR-008-plugins-are-typescript-on-deno.md)), so there has to
be an interpreter on the machine. **Arch is the only distribution that packages
one**; Fedora and Debian ship none, and Deno's own installer is the usual
route there.

Stacked looks for `deno` on `PATH` first, then in `~/.deno/bin` (where Deno's
installer puts it) and Homebrew's `bin`, and last in
`~/.local/share/cordial/deno/2.9.6/`, where Cordial's Settings window used to
download a pinned copy. **Stacked has no command that downloads Deno**; that
download was a button in the window that no longer exists
([ADR-043](adr/ADR-043-the-launcher-is-a-command-line.md)). A copy Cordial
already fetched is still used.

**Without Deno, a plugin with code is listed, granted and switched on and
still never starts**; the client's output says `deno is not on PATH` when it
tries. A Flatpak cannot see a host `deno` at all, so plugins with code do not
run in one you build yourself unless that sandbox's own data directory
already has the pinned copy.

## Installing somebody else's plugin

There is no plugin store, and this is the honest state of it: there is a
registry format, signature checking and an installer, and no populated registry
to point them at. Until there is, a plugin arrives as a directory or an archive
and you put it in place yourself.

```bash
stacked plugins install thing.tar.zst
```

Stacked unpacks it into place and prints what it is asking for, with the
command that grants each capability. A plugin with code starts switched off
and nothing is granted, so nothing runs until you say so:

```bash
stacked plugins grant thing presence.set
stacked plugins enable thing
stacked plugins                        # what is installed, on or off, and granted
```

Grants and the on/off switch belong to the current profile; add
`--profile NAME` for another. `stacked plugins remove thing` uninstalls it.
Cordial's plugin marketplace browser has no Stacked equivalent, so there is
no way to install from a registry index either.

**The archive is how a plugin travels; a folder is what it is.** A `.tar.zst`
holds the plugin directory's contents, zstd-compressed — zstd for ratio and
speed, tar because zip's Unix mode bits are optional and a plugin arriving
without its execute bit is a confusing failure. **It is not a `.tar.gz`.** If
somebody hands you one of those it is not a plugin archive, whatever is
inside it, and `stacked plugins install` will not take it.

If you are writing a plugin rather than installing one, skip the archive: put
the folder straight into `~/.local/share/cordial/plugins/<plugin-id>/` so that
its `plugin.json` is at `…/<plugin-id>/plugin.json`, or load it where it is
with `stacked config set unpacked_plugins '["/path/to/plugin"]'`. Under a
Flatpak you built yourself the plugins directory is
`~/.var/app/io.github.damnshabu.Stacked/data/cordial/plugins/` instead, since
that is where the sandbox keeps its data.

**No restart needed, since [ADR-038](adr/ADR-038-plugin-hot-swap.md).** A
client already running notices the new directory, the grant you add for it,
and `stacked plugins enable` or `disable` within a second or two, and starts,
stops or restarts exactly the plugin that changed rather than needing a fresh
launch.
That covers installing, updating, removing, enabling, disabling and granting
— everything except a `flags.write` layer, which has always taken effect at
the next launch and still does (ADR-005), because `FFlag`/`FInt`/`FString`
are read once at startup regardless of who is asking to change them.

**A plugin's preferences have no page any more.** Cordial drew one in
Settings for every plugin that declares preferences, which three of the four
shipped plugins do. Stacked has no window to draw it in and no command for
it yet, so a plugin runs with its declared defaults unless you write
`~/.local/share/cordial/profiles/<profile>/plugins/<plugin-id>/preferences.json`
yourself: a flat JSON object of key to value, using the keys in the plugin's
`plugin.json`. A value that does not fit the declaration falls back to the
default.

**Trust the source.** A plugin runs as a real process on your machine. Stacked
gives it no ambient permissions — no file access, no network, no environment, no
subprocess, and every capability it uses is one you approved by name — but that
is a boundary, not a guarantee about intent, and installing something because a
stranger linked it is the same decision it is anywhere else.

Writing one is [`plugins/README.md`](../plugins/README.md), and the capability
model is [ADR-007](adr/ADR-007-host-resources-are-brokered.md).
