# Controllers work, and the on-screen button glyphs may name the wrong brand

Controller support is on by default. Stacked reads your pad from
`/dev/input/js*` and hands its buttons and sticks to Roblox, and that part is
tested.

**What is not established is which number tells Roblox your controller's
brand.** Roblox ships separate glyph sets for PlayStation, Xbox and a generic
pad, and picks between them with an integer whose meaning is not published
anywhere we can read. Stacked sends a value; it may be the wrong one. If it is,
**you will see the wrong brand of button prompt and every button will still
work.** Sober has the same fault from the same cause — its issues
[#584](https://github.com/vinegarhq/sober/issues/584) and
[#1810](https://github.com/vinegarhq/sober/issues/1810) are exactly this.

Try other values if the glyphs look wrong:

```bash
CORDIAL_GAMEPAD_TYPE=1 stacked      # then 2, 3, ...
```

**If you find the value that draws your controller's own glyphs, please
[open an issue](https://github.com/DamnShabu/stacked/issues) and say which pad
and which number.** That settles it for everyone, and it is the one thing we
cannot work out without a controller in front of the engine. Stacked prints the
value it used at launch, once, the first time it sees a pad.

**In the Flatpak**, the sandbox had no `/dev/input` until the manifest gained
`--device=input`, so a Flatpak installed before that cannot see a pad. That
grant exposes `/dev/input` and nothing else in `/dev`, which still includes the
event nodes of keyboards and mice as far as your own file permissions allow,
because Flatpak has no grant for the pad alone. To give an existing install the
permission yourself:

```bash
flatpak override --user --device=input io.github.damnshabu.Stacked
```

Flatpak older than 1.15.6, which is what Ubuntu 24.04 and Debian 12 ship, does
not know `input` and refuses that command; `--device=all` is the wider grant
those versions have. Checked upstream: `flatpak run --device=input` makes
`/dev/input` appear in the sandbox. **INFERRED:** a pad then works there; no pad
was attached for that check.

Force feedback is absent rather than broken: there is no rumble, deliberately,
because a rumble call that silently does nothing is worse than none.

**If a stick or button does the wrong thing**, there are two things to try
before an issue. Stacked works out which stick and button is which from what
the kernel says your pad has. A pad whose D-pad is four buttons rather than a
hat (the AYN Odin pads) gets its D-pad, a pad with digital triggers is read
correctly, and a four-axis pad (X, Y, Z, RZ) has its right stick on Z and RZ. A
DualShock 4 on the older `hid-sony` driver is recognised by its ids and has its
right stick on Z and RZ too.

If a stick is still on the wrong axis, say which axis is which for this run
with `CORDIAL_GAMEPAD_AXES`, as `linux=android` pairs:

```bash
CORDIAL_GAMEPAD_AXES=z=rz,rz=z stacked         # Z and RZ become the right stick
CORDIAL_GAMEPAD_AXES=rx=ltrigger,ry=rtrigger,z=z,rz=rz stacked
```

The left side is the axis the pad declares (`x y z rx ry rz hat0x hat0y`), the
right is what Roblox is told (`x y z rx ry rz hat_x hat_y ltrigger rtrigger`,
or `none` to drop it). It applies to every pad, and a pair that cannot be read
is named in the log and ignored. If one works, open an issue with the pad's
name and the line, and it can become an entry in the built-in table.

If none of that helps, open an issue with these lines from a terminal (`js1`
and so on for a second pad):

```bash
cat /sys/class/input/js0/device/name
cat /sys/class/input/js0/device/capabilities/abs
cat /sys/class/input/js0/device/capabilities/key
cat /sys/class/input/js0/device/id/{vendor,product,version}
```

Starting with `CORDIAL_TRACE_GAMEPAD=1` also prints the Android code each axis
and button was given.

Sticks, the D-pad hat and triggers are passed in the shape Roblox's own menu
was seen to accept: a stick is one event carrying both its axes, with up as
positive. The left stick and the hat were checked by moving the menu selection;
**INFERRED:** the right stick and the triggers, which no screen reachable
without a signed-in account reads. Why:
[ADR-064](adr/ADR-064-a-gamepad-axis-call-is-a-stick-not-an-axis.md).

**INFERRED:** the Odin 3 and DualShock 4 mappings come from Android's key
layout and SDL's controller database, not from a pad plugged into the machine
that wrote this. The table is `QUIRKS` in
`crates/cordial-runtime/src/android/gamepad.rs`.

```bash
CORDIAL_MONITOR=1 CORDIAL_FULLSCREEN=1 cargo run --release --bin cordial-run -- \
  --lib-dir /path/to/lib/x86_64 --apk /path/to/base.apk \
  --host-libc --game-activity --run 30
```

`cordial-run --help` lists the rest. `stacked config set gamepad false` turns
controller support off entirely, and `CORDIAL_GAMEPAD=0` does the same for one
launch. With the control socket on, `gamepad off` and `gamepad on` switch it
while a game runs: off tells the engine every pad it knows about has been
unplugged, and on lets the next rescan, within two seconds, announce them again.
