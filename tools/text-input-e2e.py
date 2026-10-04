#!/usr/bin/env python3
"""End-to-end test of Cordial's Roblox text editor, with real keys.

Why this exists rather than `tools/text-entry-check.sh`, which already tests
typing: that script asserts on the *path* -- a spec arrived, the editor was
placed somewhere with a size, the client survived -- and leaves "did the right
characters end up in the box" to a human looking at a screenshot. Every failure
this project has actually shipped in the text editor would have passed it. A
guard that inserted every character twice, a select-all that silently destroyed
the selection, a click that put the caret in the wrong place: all of them leave
a live client with a correctly placed editor.

Three instruments here are different from that script's, and each replaces one
that could not see a failure:

  * **sway, not cage.** cage advertises no `zwp_text_input_manager_v3` and its
    seat never gains a keyboard. Cordial reads seat capabilities once, at
    `open()`, so under cage it never binds its own `wl_keyboard` -- and the
    guard that stops GDK and Cordial both inserting the same character never
    runs. `docs/NEXT.md` labelled that guard `INFERRED` for exactly this
    reason.

  * **A virtual keyboard that is held open from before the client starts**, so
    the seat looks like a real one for the whole run and that guard is under
    test. `wlrctl` creates its keyboard and exits in the same breath, which is
    a race it sometimes wins.

  * **The `textbox` devctl verb**, which reports what the field actually
    contains. There was no readback at all before: the editor's change signal
    prints nothing, the `text ->` trace prints a byte count, and
    `cordial_screenshot` photographs the engine's swapchain, which cannot see a
    GTK widget.

Input is driven inside a nested compositor on its own `WAYLAND_DISPLAY`, which
is the one form AGENTS.md permits -- never at the developer's session.

Usage:  tools/text-input-e2e.py [--profile NAME] [--keep]
"""

import argparse, json, os, re, shutil, socket, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HOLDERS = os.environ.get("CORDIAL_HOLDER_BIN", "/tmp/cordial-wl-holders")
BOX = "cordial"                       # the distrobox with sway, grim and the holders
OUT = os.environ.get("CORDIAL_E2E_OUT", "/tmp/cordial-text-e2e")


def sh(argv, **kw):
    return subprocess.run(argv, capture_output=True, text=True, **kw)


def in_box(cmd):
    """Run a shell command inside the container that has the compositor."""
    return sh(["distrobox", "enter", BOX, "--", "bash", "-lc", cmd])


class Devctl:
    """The one-line-in, one-line-out development control socket."""

    def __init__(self, path):
        self.path = path

    def send(self, line):
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(20)
        s.connect(self.path)
        s.sendall((line + "\n").encode())
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = s.recv(65536)
            if not chunk:
                break
            buf += chunk
        s.close()
        reply = buf.decode().strip()
        if reply.startswith("err "):
            raise RuntimeError(f"devctl {line!r}: {reply}")
        return reply[3:].strip() if reply.startswith("ok") else reply

    def textbox(self):
        """Parse the `textbox` reply into a dict; `text` stays a real string."""
        line = self.send("textbox")
        head, _, text = line.partition(" text=")
        # Quoted values, not just bare ones: `family` reports what fontconfig
        # calls the face and half of Roblox's shipped families have a space in
        # them ("Builder Sans", "Press Start 2P"). A plain `head.split()` cut
        # those in half and silently kept the first word.
        d = {k: json.loads(v) if v.startswith('"') else v
             for k, v in re.findall(r'(\w+)=("[^"]*"|\S*)', head)}
        # `redacted` emits Rust's `{:?}`, which for these strings is JSON.
        try:
            d["text"] = json.loads(text) if text.startswith('"') else None
        except Exception:
            d["text"] = None
        d["raw_text"] = text
        for k in ("gen", "rev", "chars", "bytes", "caret"):
            if k in d:
                d[k] = int(d[k])
        return d


class Holder:
    """A persistent virtual input device, driven a line at a time."""

    def __init__(self, binary, display, args=()):
        self.p = subprocess.Popen(
            ["distrobox", "enter", BOX, "--", "bash", "-lc",
             f"export WAYLAND_DISPLAY={display}; exec {binary} {' '.join(args)}"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        # The holder prints "ready" once the device exists. Waiting for it is
        # the difference between a keyboard the seat has and one it is about
        # to have; every key sent before it is silently dropped.
        deadline = time.time() + 20
        while time.time() < deadline:
            if self.p.poll() is not None:
                raise RuntimeError(f"{binary} exited: {self.p.stderr.read()}")
            line = self.p.stderr.readline()
            if line.startswith("ready"):
                return
        raise RuntimeError(f"{binary} never became ready")

    def cmd(self, line, settle=0.15):
        self.p.stdin.write(line + "\n")
        self.p.stdin.flush()
        self.p.stdout.readline()          # "ok" per command, so we never sleep blind
        time.sleep(settle)

    def close(self):
        try:
            self.p.stdin.write("quit\n")
            self.p.stdin.flush()
        except Exception:
            pass
        try:
            self.p.wait(timeout=5)
        except Exception:
            self.p.kill()


class Case:
    """One assertion, recorded whether it passed or not.

    Nothing here retries and nothing relaxes on failure. The whole value of
    this file is that a result which needed a second chance is a failure.
    """

    def __init__(self):
        self.results = []

    def check(self, name, got, want):
        ok = got == want
        self.results.append((ok, name, got, want))
        print(f"  {'ok  ' if ok else 'FAIL'}  {name}: got {got!r}"
              + ("" if ok else f", want {want!r}"), flush=True)
        return ok

    def note(self, name, value):
        print(f"        {name}: {value}", flush=True)

    @property
    def failed(self):
        return [r for r in self.results if not r[0]]


SWAY_CFG = """xwayland disable
output HEADLESS-1 mode {w}x{h}
default_border none
default_floating_border none
focus_follows_mouse no
{extra}
exec sh -c 'printf %s "$WAYLAND_DISPLAY" > {stamp}'
"""


def start_sway(width, height):
    """A headless sway, which reports the display it bound rather than being guessed at.

    sway picks its own socket name -- `WAYLAND_DISPLAY` on its command line
    names the display it would *connect* to, not the one it serves -- so it is
    asked from an exec block. Guessing cost two runs.
    """
    stamp, cfg = "/tmp/cordial-e2e-display", "/tmp/cordial-e2e-sway.cfg"
    for f in (stamp, cfg):
        if os.path.exists(f):
            os.unlink(f)
    with open(cfg, "w") as fh:
        fh.write(SWAY_CFG.format(w=width, h=height, stamp=stamp,
                                 extra=os.environ.get("CORDIAL_E2E_SWAY_EXTRA", "")))
    # Detached from this process's stdout. Inheriting it means the compositor
    # holds the pipe open after the script exits, and anything reading the
    # script through a pipe -- `| tail`, a harness, an agent -- waits forever
    # for an EOF that only arrives when the compositor dies. One whole run's
    # results were lost that way before this line existed.
    subprocess.Popen(
        ["distrobox", "enter", BOX, "--", "bash", "-lc",
         f"exec env -u WAYLAND_DISPLAY -u DISPLAY WLR_BACKENDS=headless "
         f"WLR_LIBINPUT_NO_DEVICES=1 WLR_HEADLESS_OUTPUTS=1 "
         f"sway -c {cfg} > /tmp/cordial-e2e-sway.log 2>&1"],
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.time() + 25
    while time.time() < deadline:
        if os.path.exists(stamp) and os.path.getsize(stamp):
            return sway_pid(cfg), open(stamp).read().strip()
        time.sleep(0.2)
    raise RuntimeError("sway never reported a display; see /tmp/cordial-e2e-sway.log")


def sway_pid(cfg):
    """The compositor's own pid, so cleanup kills it rather than its launcher.

    Killing the `distrobox enter` wrapper leaves sway running, and a stale
    compositor holding a display is indistinguishable from a fresh one until
    the seat comes back with no keyboard on it.
    """
    for pid in in_box("pidof sway").stdout.split():
        argv = in_box(f"tr '\\0' ' ' < /proc/{pid}/cmdline").stdout
        if cfg in argv:
            return int(pid)
    return None


def profile_holder(profile):
    """The pid holding this profile's lock, or None.

    Never `pgrep -f`: the pattern matches the shell running it, and that has
    killed this session's own process five times in one day. `pidof` matches
    the executable, which the engine's thread rename does not touch.
    """
    out = sh(["pidof", "cordial-run"]).stdout.split()
    for pid in out:
        try:
            argv = open(f"/proc/{pid}/cmdline").read().split("\0")
        except OSError:
            continue
        if "--profile" in argv and argv[argv.index("--profile") + 1] == profile:
            return int(pid)
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--profile", default="default",
                    help="a signed-in profile; must not be one another client holds")
    ap.add_argument("--keep", action="store_true", help="leave the client and compositor up")
    ap.add_argument("--only-engine-text", action="store_true",
                    help="run only the Enter and engine-text cases (sections 15-17), which "
                         "use a synthetic focused box and need no real one")
    ap.add_argument("--control", action="store_true",
                    help="start the client with CORDIAL_EDITOR_GTK_ENTER=1, which gives GTK's "
                         "own newline on Enter back; section 15 then expects it, so the same "
                         "assertion shows what swallowing Return changed")
    ap.add_argument("--width", type=int, default=1280)
    ap.add_argument("--height", type=int, default=800)
    args = ap.parse_args()

    os.makedirs(OUT, exist_ok=True)
    log_path = os.path.join(OUT, "client.log")
    # Overridable for the same reason CORDIAL_APK/CORDIAL_LIB_DIR already are:
    # AGENTS.md's build split means a host that builds in the toolbox
    # container has no `target/release` at all, and building bare cargo into
    # one to satisfy this default would be the two-CARGO_TARGET_DIRs mistake
    # that file warns about, not a fix.
    binary = os.environ.get("CORDIAL_E2E_BINARY", os.path.join(ROOT, "target/release/cordial-run"))
    apk = os.environ.get(
        "CORDIAL_APK",
        os.path.expanduser("~/.var/app/org.vinegarhq.Sober/data/sober/packages/"
                           "x86_64/com.roblox.client/base.apk"))
    lib = os.environ.get("CORDIAL_LIB_DIR", os.path.expanduser("~/.cache/cordial/lib/x86_64"))

    for path, what in ((binary, "build -- cargo build --release first"), (apk, "APK"), (lib, "library directory")):
        if not os.path.exists(path):
            sys.exit(f"FAIL: no {what} at {path}")
    for tool in ("wl-keyboard-holder", "wl-pointer-holder"):
        if not os.path.exists(os.path.join(HOLDERS, tool)):
            sys.exit(f"FAIL: no {tool} -- run tools/build-wl-holders.sh in the container")
    held = profile_holder(args.profile)
    if held:
        sys.exit(f"FAIL: profile {args.profile!r} is already open in pid {held}. "
                 f"Pick another with --profile; do not kill a client someone is using.")

    sway = client = kbd = ptr = None
    case = Case()
    try:
        sway, display = start_sway(args.width, args.height)
        print(f"== nested sway on {display}, {args.width}x{args.height}")

        # Both devices before the client, so the seat advertises a keyboard and
        # a pointer at the moment Cordial reads its capabilities. This is the
        # whole reason the double-insert guard has never been exercised.
        kbd = Holder(f"{HOLDERS}/wl-keyboard-holder", display)
        ptr = Holder(f"{HOLDERS}/wl-pointer-holder", display, [str(args.width), str(args.height)])
        caps = in_box(f"SWAYSOCK=$(ls -t $XDG_RUNTIME_DIR/sway-ipc.*.sock | head -1) "
                      f"swaymsg -t get_seats -r").stdout
        print(f"== seat: {caps.strip()[:200]}")
        if '"capabilities":3' not in caps.replace(" ", ""):
            sys.exit("FAIL: the seat does not advertise both a keyboard and a pointer; "
                     "every reading below would be taken with Cordial's key path switched off")

        env = dict(os.environ)
        env.update(
            WAYLAND_DISPLAY=display,
            GDK_BACKEND="wayland",
            CORDIAL_DEV_CONTROL="1",
            CORDIAL_TRACE_TEXT="1",
            # The readback. Safe here and nowhere else: this harness types its
            # own known strings into a search box and never touches a sign-in
            # field. It also puts the clipboard in the log, so the log stays in
            # /tmp and is not something to paste into an issue.
            CORDIAL_TRACE_TEXT_SHOW_PASSWORDS="1",
        )
        if args.control:
            env["CORDIAL_EDITOR_GTK_ENTER"] = "1"
        log = open(log_path, "w")
        client = subprocess.Popen(
            [binary, "--lib-dir", lib, "--apk", apk, "--host-libc",
             "--game-activity", "--run", "0", "--profile", args.profile],
            env=env, stdout=log, stderr=subprocess.STDOUT)
        print(f"== client pid {client.pid}, log {log_path}")

        deadline = time.time() + 120
        ready = None
        while time.time() < deadline:
            if client.poll() is not None:
                sys.exit(f"FAIL: the client exited with {client.returncode} before it was ready; "
                         f"see {log_path}")
            text = open(log_path, errors="replace").read()
            # Specifically Home or Landing. The shell announces several routers
            # before either -- `PlatformAccountRouter` arrives first and is not
            # a screen with a search box on it -- and taking the first `app
            # ready` cost a whole run clicking at a page that had not loaded.
            m = re.search(r"app ready: (Home|Landing)", text)
            if m:
                ready = m.group(1)
                break
            time.sleep(1)
        if not ready:
            sys.exit(f"FAIL: the app shell never became ready; see {log_path}")
        print(f"== app ready: {ready}")
        # Ready is not laid out, and nothing in the log marks that.
        time.sleep(14)

        # XDG_DATA_HOME-aware: AGENTS.md tells agents to redirect it so a
        # throwaway run does not collide with someone else's `default`
        # profile, and a hardcoded `~/.local/share` here would silently watch
        # the wrong socket (or none) for exactly the run that most needs
        # isolation -- confirmed the hard way, `ConnectionRefusedError`
        # against the real home directory while the client's actual socket
        # sat under the redirected one.
        data_home = os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share")
        dev = Devctl(os.path.join(data_home, "cordial", "profiles", args.profile, "devctl.sock"))
        print(f"== {dev.send('info')}")

        # **Is this client actually running?** About a third of launches hit the
        # startup freeze -- the engine reaches Home, presents a frame or two and
        # stops (docs/NEXT.md §0). Every assertion below would then fail, and
        # they would fail describing the text editor, which is not what broke.
        # A test that blames the wrong thing is worse than one that refuses.
        def presents():
            return int(re.search(r"presents=(\d+)", dev.send("info")).group(1))
        first = presents()
        time.sleep(6)
        if presents() == first and first < 10:
            sys.exit(f"SKIPPED: this launch hit the startup freeze -- presents stuck at "
                     f"{first}. That is docs/NEXT.md §0 and not a text-entry result. "
                     f"Run it again; it is about a third of launches.")
        if args.only_engine_text:
            run_enter_and_engine_text(case, dev, kbd, log_path, args.control)
        else:
            run_cases(case, dev, kbd, ptr, log_path, args, display)
    finally:
        if not args.keep:
            for h in (kbd, ptr):
                if h:
                    h.close()
            if client and client.poll() is None:
                client.terminate()
                try:
                    client.wait(timeout=15)
                except Exception:
                    client.kill()
            if sway:
                in_box(f"kill {sway}")

    print()
    if case.failed:
        print(f"BROKEN -- {len(case.failed)} of {len(case.results)} assertions failed:")
        for _, name, got, want in case.failed:
            print(f"  {name}: got {got!r}, want {want!r}")
        sys.exit(1)
    print(f"PERFECT -- {len(case.results)} assertions, none failed.")


TEXT_KEYS = set(range(2, 14)) | set(range(16, 28)) | set(range(30, 42)) | set(range(44, 54)) | {57}


def settle(dev, want_rev=None, tries=40):
    """Wait for the mirror to catch up with the widget, then read it.

    The `textbox` verb reports the buffer `adopt_editor_text` mirrors from the
    widget, which is one signal-hop behind it. Polling the revision counter is
    how that hop is waited out; sleeping a fixed amount instead is how a
    harness starts reporting the previous keystroke's answer.
    """
    last = None
    for _ in range(tries):
        last = dev.textbox()
        if want_rev is None or last["rev"] != want_rev:
            return last
        time.sleep(0.05)
    return last


def focus_box(case, dev, tries=5, geometry_timeout=4.0):
    """Click the home search bar until a TextBox takes focus and says where it is.

    Focus and geometry are two events, not one, and the gap between them is the
    whole reason this waits rather than reading once. Clicking the search bar
    opens a modal, and the engine focuses that modal before it has laid out --
    `showKeyboard` volunteers `x=0 y=0 w=0 h=0` and the real numbers only arrive
    from `nativeGetTextBoxInfo` about a second later. Reading at the instant of
    focus gets the zeroes every time, and then every coordinate computed from
    them is nonsense; that misread three assertions in the first full run.

    The returned box is the first one with a usable size. How long that took is
    reported, because a second of it is a second the user is looking at a box
    with no editor on it.
    """
    for attempt in range(1, tries + 1):
        dev.send("click 674 28")
        for _ in range(20):
            time.sleep(0.25)
            if dev.textbox()["focus"] != "none":
                break
        else:
            print(f"        attempt {attempt} focused nothing, retrying", flush=True)
            continue
        start = time.time()
        while time.time() - start < geometry_timeout:
            box = dev.textbox()
            if box["focus"] == "none":
                break                       # it blurred again; click once more
            if box["x"] != "none" and float(box["w"]) > 0 and float(box["h"]) > 0:
                box["geometry_wait"] = round(time.time() - start, 2)
                return box
            time.sleep(0.1)
    box = dev.textbox()
    box["geometry_wait"] = None
    return box


def wait_for(dev, pred, timeout=3.0):
    """Poll the `textbox` readback until `pred` holds, and return the last reading.

    The pump applies an engine push and seeds the widget on its own tick, so
    asserting immediately reads the state before the thing under test has
    happened. Returns the last reading either way; the caller's assertion is
    what fails, not this.
    """
    last = None
    end = time.time() + timeout
    while time.time() < end:
        last = dev.textbox()
        if pred(last):
            return last
        time.sleep(0.05)
    return last


def fake_box(dev, text=""):
    """Focus a synthetic multi-line box and wait for the editor to be on it.

    **Every reading taken in a box made here is SYNTHETIC.** `fakefocus` is
    the engine-side focus path with nothing behind it: it shows what the GTK
    editor does with a `multiline=1` spec, and says nothing about what a real
    chat box sends. A fresh blur first, so no case inherits the previous one's
    text, caret or echo history.
    """
    dev.send("fakeblur")
    time.sleep(0.6)
    dev.send(f"fakefocus 1 300 300 400 120 {text}")
    box = wait_for(dev, lambda b: b["focus"] != "none" and b["x"] != "none"
                   and float(b["w"]) > 0, timeout=5.0)
    # Placed is not focused: GTK takes keyboard focus a pump tick later, and a
    # key sent before it lands on nothing.
    time.sleep(1.0)
    return box


def run_enter_and_engine_text(case, dev, kbd, log_path, control):
    """Sections 15-17: Enter in a multi-line editor, engine-to-editor text, and the single-line editor.

    Everything here runs in a SYNTHETIC multi-line box (`fakefocus`) and the
    engine's push is synthetic too (`enginetext`, the same entry point
    `onLuaTextBoxChangedCallback` uses). That is as far as a signed-out
    session reaches: a real chat box sending and clearing needs an account.
    """
    def type_(s):
        kbd.cmd(f"type {s}")

    def key(k):
        kbd.cmd(f"key {k}")

    def log():
        return open(log_path, errors="replace").read()

    enters = lambda: re.findall(r"pass_key_event down=true code=28 mods=(0x[0-9a-f]+)", log())

    print("\n-- 15. Return in a multi-line editor (SYNTHETIC box)"
          + ("  [CONTROL: CORDIAL_EDITOR_GTK_ENTER=1]" if control else ""))
    fake_box(dev)
    type_("ok")
    s = wait_for(dev, lambda b: b["chars"] == 2)
    case.check("typed 'ok' into the multi-line editor", s["text"], "ok")
    before = len(enters())
    key("Return")
    time.sleep(0.8)
    s = dev.textbox()
    want_text, want_chars = ("ok\n", 3) if control else ("ok", 2)
    case.check("text after Return" + (" (control: GTK inserts its own newline)" if control else ""),
               s["text"], want_text)
    case.check("characters after Return", s["chars"], want_chars)
    got = enters()
    case.check("the engine was handed the raw Return", len(got) - before, 1)
    case.check("a bare Return carries no modifiers", got[-1] if got else None, "0x0")

    fake_box(dev)
    type_("ok")
    wait_for(dev, lambda b: b["chars"] == 2)
    before = len(enters())
    key("shift+Return")
    time.sleep(0.8)
    s = dev.textbox()
    case.check("text after Shift+Return" + (" (control)" if control else ""),
               s["text"], want_text)
    got = enters()
    case.check("the engine was handed Shift+Return as well", len(got) - before, 1)
    case.note("modifiers the engine received with Shift+Return", got[-1] if got else None)
    case.check("Shift+Return reached the engine with a modifier set",
               bool(got) and got[-1] != "0x0", True)

    print("\n-- 16. the engine's text reaches the editor (SYNTHETIC push)")
    # The pre-fix symptom, with no push: the engine cleared its box and the
    # editor was never told. Typing on builds on the stale text.
    fake_box(dev)
    type_("ok")
    wait_for(dev, lambda b: b["chars"] == 2)
    key("Return")
    time.sleep(0.6)
    type_("x")
    s = wait_for(dev, lambda b: "x" in (b["text"] or ""))
    case.note("CONTROL (no engine push, the old behaviour)", f"text after 'ok', Return, 'x': {s['text']!r}")
    case.check("control: without a push the editor still holds the sent text",
               (s["text"] or "").replace("\n", "") , "okx")

    fake_box(dev)
    type_("ok")
    wait_for(dev, lambda b: b["chars"] == 2)
    key("Return")
    time.sleep(0.6)
    rev = dev.textbox()["rev"]
    dev.send("enginetext ")
    s = wait_for(dev, lambda b: b["rev"] != rev and b["chars"] == 0)
    case.check("an engine-side clear empties the editor mirror", (s["text"], s["chars"], s["caret"]),
               ("", 0, 0))
    type_("x")
    s = wait_for(dev, lambda b: b["chars"] >= 1)
    case.check("the next keystroke builds on the engine's text, not the stale one",
               s["text"], "x")

    fake_box(dev)
    dev.send("enginetext hello")
    s = wait_for(dev, lambda b: b["chars"] == 5)
    case.check("engine text arrives in the box", (s["text"], s["caret"]), ("hello", 5))
    type_("Z")
    s = wait_for(dev, lambda b: b["chars"] == 6)
    case.check("the widget really holds the engine's text (typing builds on it)",
               s["text"], "helloZ")

    # A stale echo: "ab" was typed, so "a" is the engine reflecting the first
    # keystroke. Applying it would put the box back a character.
    fake_box(dev)
    type_("ab")
    wait_for(dev, lambda b: b["chars"] == 2)
    rev = dev.textbox()["rev"]
    dev.send("enginetext a")
    time.sleep(0.8)
    s = dev.textbox()
    case.check("an echo of an earlier keystroke is ignored", (s["text"], s["rev"]), ("ab", rev))
    # Same push once the entry has expired: now it is the engine's own edit.
    time.sleep(1.3)
    dev.send("enginetext a")
    s = wait_for(dev, lambda b: b["rev"] != rev)
    case.check("the same text after the echo window is applied", s["text"], "a")

    # Caret: kept when the new text only adds to the end of what was there and
    # the caret was not at the end.
    fake_box(dev)
    type_("ab")
    wait_for(dev, lambda b: b["chars"] == 2)
    key("Left")
    wait_for(dev, lambda b: b["caret"] == 1)
    dev.send("enginetext abc")
    s = wait_for(dev, lambda b: b["chars"] == 3)
    case.check("a pure extension keeps a mid-text caret", (s["text"], s["caret"]), ("abc", 1))
    type_("Q")
    s = wait_for(dev, lambda b: b["chars"] == 4)
    case.check("the widget's caret was placed there too", s["text"], "aQbc")

    fake_box(dev)
    type_("abc")
    wait_for(dev, lambda b: b["chars"] == 3)
    key("Left")
    dev.send("enginetext xy")
    s = wait_for(dev, lambda b: b["text"] == "xy")
    case.check("a rewrite puts the caret at the end", (s["text"], s["caret"]), ("xy", 2))
    type_("!")
    s = wait_for(dev, lambda b: b["chars"] == 3)
    case.check("and the widget agrees", s["text"], "xy!")
    print("\n-- 17. the single-line editor is unchanged (SYNTHETIC box, multiline=0)")
    # Sections 3-8 and 10 of this file, replayed in a synthetic single-line box.
    # They are the cases that assert the double-insert guard, caret placement and
    # selection on the `gtk::Text` widget this change did not mean to touch;
    # section 1's focus needs the signed-in Home search bar and cannot run on a
    # signed-out profile, so these stand in for 3-8 and 10 there.
    dev.send("fakeblur")
    time.sleep(0.6)
    dev.send("fakefocus 0 300 300 400 40")
    wait_for(dev, lambda b: b["focus"] != "none" and b["x"] != "none"
             and float(b["w"]) > 0, timeout=5.0)
    time.sleep(1.0)
    type_("hello")
    s = wait_for(dev, lambda b: b["chars"] == 5)
    case.check("single-line: text and caret after typing 'hello'", (s["text"], s["caret"]), ("hello", 5))
    key("BackSpace")
    s = wait_for(dev, lambda b: b["chars"] == 4)
    case.check("single-line: backspace", (s["text"], s["caret"]), ("hell", 4))
    key("Home")
    type_("X")
    s = wait_for(dev, lambda b: b["chars"] == 5)
    case.check("single-line: insertion at the caret", (s["text"], s["caret"]), ("Xhell", 1))
    key("End")
    type_("!")
    s = wait_for(dev, lambda b: b["chars"] == 6)
    case.check("single-line: append", s["text"], "Xhell!")
    key("shift+Left"); key("shift+Left")
    type_("?")
    s = wait_for(dev, lambda b: b["text"] == "Xhel?")
    case.check("single-line: a selection is replaced", (s["text"], s["caret"]), ("Xhel?", 5))
    key("ctrl+a")
    type_("abcdefgh")
    s = wait_for(dev, lambda b: b["text"] == "abcdefgh")
    case.check("single-line: select all, overtype", (s["text"], s["caret"]), ("abcdefgh", 8))
    key("ctrl+a"); key("ctrl+c"); key("End"); key("ctrl+v")
    s = wait_for(dev, lambda b: b["chars"] == 16, timeout=4.0)
    case.check("single-line: paste appends the copy once", s["text"], "abcdefghabcdefgh")
    before = len(enters())
    key("Return")
    time.sleep(0.8)
    s = dev.textbox()
    case.check("single-line: Return inserts nothing (gtk::Text has no newline to insert)",
               s["text"], "abcdefghabcdefgh")
    case.check("single-line: the engine still got the raw Return", len(enters()) - before, 1)
    # And the engine's push reaches this widget as well.
    dev.send("enginetext cleared")
    s = wait_for(dev, lambda b: b["text"] == "cleared")
    case.check("single-line: an engine push replaces the text", (s["text"], s["caret"]), ("cleared", 7))
    type_("!")
    s = wait_for(dev, lambda b: b["chars"] == 8)
    case.check("single-line: typing builds on the pushed text", s["text"], "cleared!")

    dev.send("fakeblur")
    time.sleep(0.6)


def run_cases(case, dev, kbd, ptr, log_path, args, display):
    print("\n-- 1. a click focuses a TextBox and the editor gets real geometry")
    box = focus_box(case, dev)
    case.check("a TextBox has focus", box["focus"] != "none", True)
    if box["focus"] == "none":
        shot = os.path.join(OUT, "no-focus.png")
        in_box(f"WAYLAND_DISPLAY={display} grim {shot}")
        try:
            dev.send(f"screenshot {os.path.join(OUT, 'no-focus-swapchain.png')}")
        except Exception as e:
            case.note("swapchain capture", f"refused: {e}")
        case.note("what was on screen", shot)
        return
    case.note("geometry", f"x={box['x']} y={box['y']} w={box['w']} h={box['h']} "
                          f"after {box.get('geometry_wait')}s")
    case.check("the box has a real width", box["x"] != "none" and float(box["w"]) > 0, True)
    case.check("the box has a real height", box["x"] != "none" and float(box["h"]) > 0, True)
    log = open(log_path, errors="replace").read()
    placed = re.findall(r"text editor placed from ([a-z ]+) x=", log)
    case.check("the editor ends up on the box", placed[-1] if placed else None,
               "engine geometry")
    # The editor must never be *seen* anywhere else on the way there. A focus
    # whose geometry has not arrived yet used to drop the editor into a bar at
    # the bottom of the window for about a second and then jump it back up,
    # which is visible and is a large part of what "the text field looks wrong"
    # means.
    case.check("the editor never flashed in the fallback bar",
               placed.count("fallback bar"), 0)

    # Clicking the search bar opens a modal, and the modal is the box that
    # focuses before it has laid out. Watching the placement source across that
    # transition is the only way to see the flinch this fix is about: by the
    # time anything settles, the editor is on the modal and looks fine.
    sources, deadline = [], time.time() + 4
    while time.time() < deadline:
        sources.append(dev.textbox().get("placed"))
        time.sleep(0.1)
    seen = sorted(set(s for s in sources if s))
    case.note("placement sources across the modal opening", " ".join(seen) or "none")
    case.check("the editor was never dropped into the placed bar",
               [s for s in seen if s == "fallback"], [])

    def type_(s):
        kbd.cmd(f"type {s}")

    def key(k):
        kbd.cmd(f"key {k}")

    def state():
        return settle(dev)

    print("\n-- 2. a clean start")
    key("ctrl+a"); key("BackSpace")
    s = state()
    case.check("the box empties", (s["text"], s["chars"], s["caret"]), ("", 0, 0))

    print("\n-- 3. typing five keys inserts five characters (the double-insert guard)")
    type_("hello")
    s = state()
    case.check("text after typing 'hello'", s["text"], "hello")
    case.check("caret after typing 'hello'", s["caret"], 5)

    print("\n-- 4. backspace")
    key("BackSpace")
    s = state()
    case.check("text after backspace", s["text"], "hell")
    case.check("caret after backspace", s["caret"], 4)

    print("\n-- 5. Home, and insertion at the caret rather than the end")
    key("Home")
    case.check("caret after Home", state()["caret"], 0)
    type_("X")
    s = state()
    case.check("text after inserting at the start", s["text"], "Xhell")
    case.check("caret after inserting at the start", s["caret"], 1)

    print("\n-- 6. End")
    key("End")
    case.check("caret after End", state()["caret"], 5)
    type_("!")
    s = state()
    case.check("text after appending", s["text"], "Xhell!")

    print("\n-- 7. shift-arrow selection, replaced by the next character")
    key("shift+Left"); key("shift+Left")
    type_("?")
    s = state()
    case.check("selection was replaced, not appended", s["text"], "Xhel?")
    case.check("caret after replacing a selection", s["caret"], 5)

    print("\n-- 8. select all, then overtype")
    key("ctrl+a")
    type_("abcdefgh")
    s = state()
    case.check("text after select-all and overtype", s["text"], "abcdefgh")
    case.check("caret after select-all and overtype", s["caret"], 8)

    print("\n-- 9. clicking positions the caret (a real click, through the compositor)")
    extent = re.search(r"extent=(\d+)x(\d+)", dev.send("info"))
    sw, shh = int(extent.group(1)), int(extent.group(2))
    header = args.height - shh
    case.note("surface", f"{sw}x{shh} inside a {args.width}x{args.height} output, "
                         f"header {header}px")
    box = settle(dev)
    if box["x"] == "none" or float(box["w"]) <= 0:
        case.check("the box still reports where it is, for the click test",
                   False, True)
        return
    bx, by, bw, bh = (float(box[k]) for k in ("x", "y", "w", "h"))
    row = int(by + bh / 2) + header

    def click_at(surface_x):
        ptr.cmd(f"move {int(surface_x)} {row}")
        ptr.cmd("click")
        time.sleep(0.6)
        return dev.textbox()

    left = click_at(bx + 2)
    case.check("clicking the left edge puts the caret at the start", left["caret"], 0)
    case.check("clicking inside the box keeps it focused", left["focus"] != "none", True)
    right = click_at(bx + bw - 4)
    case.check("clicking past the last character puts the caret at the end",
               right["caret"], 8)
    case.check("clicking did not change the text", right["text"], "abcdefgh")

    print("\n-- 9b. a double-click selects a word, and the next key replaces it")
    key("ctrl+a")
    type_("alpha beta")
    s2 = state()
    case.check("a string with a space in it survives", s2["text"], "alpha beta")
    # Land on the first word. A quarter of the way in is inside "alpha" for any
    # plausible font, and the assertion below does not depend on which
    # character it lands on -- only on the word boundary GTK finds from it.
    ptr.cmd(f"move {int(bx + bw * 0.08)} {row}")
    ptr.cmd("click")
    ptr.cmd("click", settle=0.05)
    time.sleep(0.5)
    type_("Z")
    s2 = state()
    case.check("double-click selected the word, and typing replaced just that word",
               s2["text"], "Z beta")

    print("\n-- 9c. Delete removes forwards, and cut removes the selection")
    key("ctrl+a")
    type_("abcdef")
    key("Home")
    key("Delete")
    case.check("Delete removes the character after the caret", state()["text"], "bcdef")
    key("ctrl+a"); key("ctrl+x")
    time.sleep(0.5)
    case.check("cut empties the field", state()["text"], "")

    print("\n-- 10. copy and paste")
    type_("abcdefgh")
    key("ctrl+a"); key("ctrl+c"); key("End")
    key("ctrl+v")
    time.sleep(1.0)
    s = state()
    case.check("paste appends the copied text exactly once", s["text"], "abcdefghabcdefgh")

    print("\n-- 11. Escape gives the box up")
    key("Escape")
    time.sleep(1.0)
    case.check("Escape blurs the TextBox", dev.textbox()["focus"], "none")

    print("\n-- 12. focusing again starts clean and still takes keys")
    box2 = focus_box(case, dev)
    case.check("the box takes focus a second time", box2["focus"] != "none", True)
    if box2["focus"] != "none":
        key("ctrl+a")
        type_("second")
        s = state()
        case.check("typing works after a refocus", s["text"], "second")

    print("\n-- 12c. the editor follows the window when it resizes")
    # **The case docs/NEXT.md says was never tested.** `editor_rect` is in
    # surface coordinates and the input region is punched from it, and the
    # region is rebuilt on placement and stacking changes -- not on a
    # configure. So the question is whether anything re-places the editor after
    # the surface changes size, or whether the hole is left where the box used
    # to be.
    #
    # It is not obviously broken: `sync_text_overlay` runs off the pump about
    # twenty times a second, re-reads the engine's geometry every 100ms, and
    # rebuilds when what it is about to draw differs from what it drew. If the
    # engine re-lays-out on resize, that should self-heal within a tick or two.
    # This asserts it rather than assuming it either way.
    #
    # Sober #1026 is the reason to care: same engine, same kind of desktop, and
    # users report the textbox dying *only* in fullscreen, only on Wayland, one
    # of them pinning it to the surface being exactly the output size.
    if box2["focus"] != "none":
        before = dev.textbox()
        case.note("editor rect before resize",
                  f"x={before.get('x')} y={before.get('y')} "
                  f"w={before.get('w')} h={before.get('h')}")
        new_w, new_h = int(args.width) - 120, int(args.height) - 80
        r = in_box(f"SWAYSOCK=$(ls -t $XDG_RUNTIME_DIR/sway-ipc.*.sock | head -1) "
                   f"swaymsg output HEADLESS-1 mode {new_w}x{new_h}")
        # **Prove the resize happened before asserting anything about it.**
        # The first version of this case did not, and passed while reporting a
        # byte-identical rectangle either side -- which is exactly what a
        # swaymsg that silently did nothing would produce. An instrument that
        # cannot fail is not evidence, and this suite exists because that
        # mistake has already been made here more than once.
        modes = in_box(f"SWAYSOCK=$(ls -t $XDG_RUNTIME_DIR/sway-ipc.*.sock | head -1) "
                       f"swaymsg -t get_outputs -r").stdout
        case.note("swaymsg said", (r.stdout or r.stderr).strip()[:120])
        got_mode = ""
        try:
            for o in json.loads(modes):
                if o.get("name") == "HEADLESS-1":
                    cm = o.get("current_mode") or {}
                    got_mode = f"{cm.get('width')}x{cm.get('height')}"
        except Exception as e:
            case.note("could not read the output mode", str(e))
        case.check("the output really did change size",
                   got_mode, f"{new_w}x{new_h}")
        # Generous: the engine has to notice the configure, re-lay-out, and
        # publish new geometry, and only then does the 100ms poll pick it up.
        time.sleep(3.0)
        after = dev.textbox()
        case.note("editor rect after resize",
                  f"x={after.get('x')} y={after.get('y')} "
                  f"w={after.get('w')} h={after.get('h')} placed={after.get('placed')}")

        # Three separate assertions, because they fail independently and the
        # third passing while the first fails is exactly the "the path worked"
        # illusion this suite exists to avoid.
        case.check("the box still has focus after a resize",
                   after.get("focus") != "none", True)
        case.check("the editor is still placed from engine geometry",
                   after.get("placed"), "engine")
        # And the one that matters: can you still type into it. A rectangle in
        # the right place proves nothing if keys no longer land.
        key("ctrl+a")
        type_("resized")
        s = state()
        case.check("typing still reaches the box after a resize", s["text"], "resized")

        in_box(f"SWAYSOCK=$(ls -t $XDG_RUNTIME_DIR/sway-ipc.*.sock | head -1) "
               f"swaymsg output HEADLESS-1 mode {args.width}x{args.height}")
        time.sleep(2.0)

    run_enter_and_engine_text(case, dev, kbd, log_path, args.control)

    print("\n-- 12b. with no box focused, keys reach the game again")
    # Everything after this mark is deliberately sent with nothing focused, so
    # step 13 stops reading here rather than counting this on purpose.
    focused_phase = len(open(log_path, errors="replace").read())
    key("Escape")
    time.sleep(1.0)
    before = len(re.findall(r"pass_key_event down=\w+ code=17 ",
                            open(log_path, errors="replace").read()))
    key("w")
    time.sleep(0.6)
    after = len(re.findall(r"pass_key_event down=\w+ code=17 ",
                           open(log_path, errors="replace").read()))
    # The suppression is only correct if it stops. A guard stuck on is the
    # failure that looks like "movement is broken" and never like a text bug.
    case.check("'w' reaches the game once the box has been given up", after > before, True)

    print("\n-- 13. no character reached the game while a box had focus")
    log = open(log_path, errors="replace").read()[:focused_phase]
    suppressed = len(re.findall(r"pass_key_event suppressed", log))
    forwarded = [int(m) for m in re.findall(r"pass_key_event down=\w+ code=(\d+)", log)]
    leaked = sorted({c for c in forwarded if c in TEXT_KEYS})
    case.note("keys", f"{suppressed} suppressed, {len(forwarded)} forwarded to the game")
    case.check("text keys were suppressed at all", suppressed > 0, True)
    case.check("no text key reached the game's key handler", leaked, [])

    print("\n-- 14. the client is still alive")
    case.check("the client survived the whole run", dev.send("ping"), "")


if __name__ == "__main__":
    main()
