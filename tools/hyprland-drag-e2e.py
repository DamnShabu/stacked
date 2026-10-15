#!/usr/bin/env python3
"""A right-button camera drag on a real Hyprland, nested, with the lock confirmed.

Issues #56 and #105: Hyprland releases a held button when a pointer lock
activates under it and never reports the physical release, so a drag that took
the lock either ended before it began (0.26.0, 0.27.0) or, with the lock taken
away again (0.28.0), left the cursor drifting. Nothing here could start
Hyprland until it was given a host that offers `wl_compositor` and
`xdg_wm_base` at version 6: sway and weston offer 5, and Hyprland's own
`aquamarine` refuses them. KWin's virtual backend offers 6, so the stack is

    kwin_wayland --virtual   ->   Hyprland (wayland backend)   ->   Cordial

all inside the `cordial` distrobox, all under a private `XDG_RUNTIME_DIR`, and
none of it on the desktop the person running this is sitting at. The runtime
directory is what keeps it that way: with `WAYLAND_DISPLAY` unset Hyprland
falls back to the first socket it finds, and an earlier attempt opened a window
on somebody's real session that way. This script exports `WAYLAND_DISPLAY`
explicitly for every child and refuses to start if the private directory
already holds a compositor.

The virtual pointer is `zwlr_virtual_pointer_v1` into the *nested* Hyprland,
which is the one form of injection AGENTS.md allows. It must exist before
Cordial starts, because Cordial reads the seat's capabilities once.

What this measures, and the control. Each right drag is run and the readbacks
are `cordial_pointerlock` (`requested`, `confirmed`, `held_for_lock`) and the
client's own trace of what reached the engine (`nativePassMouseMove` with a
nonzero `dx` while the button is down; `nativePassMouseButton down=false`
exactly once). The control is `CORDIAL_DRAG_LOCK=0`, 0.28.0's behaviour:
`confirmed` stays false throughout and no relative motion is locked.

Usage:  tools/hyprland-drag-e2e.py [--binary PATH] [--drags N] [--keep]
        tools/hyprland-drag-e2e.py --control
Requires:  tools/build-wl-holders.sh output in $CORDIAL_HOLDER_BIN, the
           `cordial` distrobox with Hyprland and kwin_wayland installed.
"""

import argparse, os, re, shutil, socket, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HOLDERS = os.environ.get("CORDIAL_HOLDER_BIN", os.path.expanduser("~/.cache/cordial-scratch/holders"))
# Short on purpose: Hyprland's IPC socket path is
#   <runtime>/hypr/<40 hex>_<time>_<n>/.socket2.sock
# and sockaddr_un holds 107 bytes.
RT = os.environ.get("CORDIAL_HYPR_E2E_RT", "/tmp/cordial-he")
OUT = os.environ.get("CORDIAL_HYPR_E2E_OUT", os.path.expanduser("~/.cache/cordial-scratch/hypr-e2e"))
BOX = "cordial"
BUTTON_SECONDARY = 2

HYPR_CONF = """monitor=,1280x800@60,0x0,1
misc {
    disable_hyprland_logo = true
    disable_splash_rendering = true
}
animations {
    enabled = false
}
decoration {
    blur { enabled = false }
    shadow { enabled = false }
}
general {
    border_size = 0
    gaps_in = 0
    gaps_out = 0
}
"""


def in_box_bg(cmd, log):
    return subprocess.Popen(
        ["distrobox", "enter", BOX, "--", "bash", "-c", f"exec {cmd} > {log} 2>&1"],
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def in_box(cmd):
    return subprocess.run(["distrobox", "enter", BOX, "--", "bash", "-c", cmd],
                          capture_output=True, text=True)


class Devctl:
    def __init__(self, path):
        self.path = path

    def send(self, line):
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(10)
        s.connect(self.path)
        s.sendall((line + "\n").encode())
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = s.recv(4096)
            if not chunk:
                break
            buf += chunk
        s.close()
        return buf.decode(errors="replace").strip()

    def lock(self):
        return dict(kv.split("=", 1) for kv in self.send("pointerlock").split() if "=" in kv)


class Holder:
    def __init__(self, display, args=()):
        self.p = subprocess.Popen(
            ["distrobox", "enter", BOX, "--", "bash", "-c",
             f"export XDG_RUNTIME_DIR={RT} WAYLAND_DISPLAY={display}; unset DISPLAY; "
             f"exec {HOLDERS}/wl-pointer-holder {' '.join(args)}"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        deadline = time.time() + 20
        while time.time() < deadline:
            if self.p.poll() is not None:
                raise RuntimeError(f"wl-pointer-holder exited: {self.p.stderr.read()}")
            if self.p.stderr.readline().startswith("ready"):
                return
        raise RuntimeError("wl-pointer-holder never became ready")

    def cmd(self, line, settle=0.25):
        self.p.stdin.write(line + "\n")
        self.p.stdin.flush()
        self.p.stdout.readline()
        time.sleep(settle)

    def close(self):
        try:
            self.p.stdin.write("quit\n")
            self.p.stdin.flush()
            self.p.wait(timeout=5)
        except Exception:
            self.p.kill()


class Case:
    def __init__(self):
        self.rows = []

    def check(self, name, got, want):
        ok = got == want
        self.rows.append((ok, name, f"{got!r} (want {want!r})"))
        print(f"  {'PASS' if ok else 'FAIL'}  {name}: {got!r} (want {want!r})")
        return ok

    def note(self, name, value):
        self.rows.append((None, name, str(value)))
        print(f"  ....  {name}: {value}")

    def failed(self):
        return [r for r in self.rows if r[0] is False]


def start_stack():
    """KWin (virtual) then Hyprland inside it. Returns the Hyprland display path."""
    if os.path.exists(RT) and os.listdir(RT):
        sys.exit(f"FAIL: {RT} is not empty; something else may be using it")
    os.makedirs(RT, mode=0o700, exist_ok=True)
    os.makedirs(OUT, exist_ok=True)
    conf = os.path.join(OUT, "hypr.conf")
    with open(conf, "w") as fh:
        fh.write(HYPR_CONF)
    # KWin gets its own session bus: left on the real one it would register
    # org.kde.KWin there, and KDE applications would believe it.
    in_box_bg(f"env -u WAYLAND_DISPLAY -u DISPLAY -u DBUS_SESSION_BUS_ADDRESS XDG_RUNTIME_DIR={RT} "
              f"KWIN_WAYLAND_NO_PERMISSION_CHECKS=1 dbus-run-session -- kwin_wayland --virtual "
              f"--socket kw --width 1600 --height 1000 --no-lockscreen", f"{OUT}/kwin.log")
    deadline = time.time() + 20
    while time.time() < deadline and not os.path.exists(f"{RT}/kw"):
        time.sleep(0.3)
    if not os.path.exists(f"{RT}/kw"):
        sys.exit(f"FAIL: kwin never created its socket; see {OUT}/kwin.log")
    in_box_bg(f"env -u DISPLAY -u DBUS_SESSION_BUS_ADDRESS -u HYPRLAND_INSTANCE_SIGNATURE "
              f"XDG_RUNTIME_DIR={RT} WAYLAND_DISPLAY=kw Hyprland -c {conf}", f"{OUT}/hypr.log")
    deadline = time.time() + 20
    while time.time() < deadline and not os.path.exists(f"{RT}/wayland-1"):
        time.sleep(0.3)
    if not os.path.exists(f"{RT}/wayland-1"):
        sys.exit(f"FAIL: Hyprland never created its socket; see {OUT}/hypr.log")
    sigs = os.listdir(f"{RT}/hypr")
    return f"{RT}/wayland-1", sigs[0]


def stop_stack():
    for name, marker in (("Hyprland", f"{RT}"), ("kwin_wayland", "--socket kw")):
        for pid in subprocess.run(["pidof", name], capture_output=True, text=True).stdout.split():
            try:
                argv = open(f"/proc/{pid}/cmdline").read().replace("\0", " ")
            except OSError:
                continue
            # Only ours: a Hyprland we started names our config, KWin our socket.
            if (name == "Hyprland" and "hypr.conf" in argv and OUT in argv) or \
               (name == "kwin_wayland" and marker in argv and "--virtual" in argv):
                os.kill(int(pid), 15)
    time.sleep(1)
    shutil.rmtree(RT, ignore_errors=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", default=os.path.join(ROOT, "target/release/cordial-run"))
    ap.add_argument("--profile", default="HyprDrag")
    ap.add_argument("--drags", type=int, default=5)
    ap.add_argument("--control", action="store_true",
                    help="run with CORDIAL_DRAG_LOCK=0 (0.28.0): the lock must not be taken")
    ap.add_argument("--env", action="append", default=[], metavar="K=V",
                    help="extra environment for the client, for running an older binary as the control")
    ap.add_argument("--keep", action="store_true")
    args = ap.parse_args()

    lib = os.path.expanduser("~/.cache/cordial/lib/x86_64")
    apk = os.path.expanduser("~/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/"
                             "com.roblox.client/base.apk")
    for path in (args.binary, lib, apk, f"{HOLDERS}/wl-pointer-holder"):
        if not os.path.exists(path):
            sys.exit(f"FAIL: missing {path}")

    case = Case()
    client = ptr = None
    scratch = os.path.join(OUT, "xdg")
    log_path = os.path.join(OUT, "client.log")
    try:
        display, sig = start_stack()
        print(f"== nested Hyprland on {display} (signature {sig[:12]}...)")
        ptr = Holder(display, ["1280", "800"])
        env = dict(os.environ)
        for k in ("DISPLAY", "GNOME_SETUP_DISPLAY"):
            env.pop(k, None)
        for sub in ("data", "cache", "config"):
            os.makedirs(f"{scratch}/{sub}", exist_ok=True)
        env.update(WAYLAND_DISPLAY=display, GDK_BACKEND="wayland", XDG_CURRENT_DESKTOP="Hyprland",
                   HYPRLAND_INSTANCE_SIGNATURE=sig, CORDIAL_DEV_CONTROL="1", CORDIAL_TRACE_MOUSE="1",
                   XDG_DATA_HOME=f"{scratch}/data", XDG_CACHE_HOME=f"{scratch}/cache",
                   XDG_CONFIG_HOME=f"{scratch}/config")
        if args.control:
            env["CORDIAL_DRAG_LOCK"] = "0"
        env.update(kv.split("=", 1) for kv in args.env)
        log = open(log_path, "w")
        client = subprocess.Popen(
            [args.binary, "--lib-dir", lib, "--apk", apk, "--host-libc", "--game-activity",
             "--run", "0", "--profile", args.profile],
            env=env, stdout=log, stderr=subprocess.STDOUT)
        deadline = time.time() + 180
        while time.time() < deadline:
            if client.poll() is not None:
                sys.exit(f"FAIL: client exited {client.returncode}; see {log_path}")
            if re.search(r"app ready: (\w+)", open(log_path, errors="replace").read()):
                break
            time.sleep(1)
        else:
            sys.exit(f"FAIL: the app shell never became ready; see {log_path}")
        time.sleep(8)
        dev = Devctl(f"{scratch}/data/cordial/profiles/{args.profile}/devctl.sock")
        run_cases(case, dev, ptr, log_path, args)
    finally:
        if ptr:
            ptr.close()
        if client and client.poll() is None:
            client.terminate()
            try:
                client.wait(timeout=15)
            except Exception:
                client.kill()
        if not args.keep:
            stop_stack()
            shutil.rmtree(scratch, ignore_errors=True)

    bad = case.failed()
    print()
    if bad:
        print(f"FAIL: {len(bad)} assertion(s) failed")
        for _, name, detail in bad:
            print(f"  - {name}: {detail}")
        return 1
    print("PASS: every assertion held")
    return 0


def log_text(path):
    return open(path, errors="replace").read()


def run_cases(case, dev, ptr, log_path, args):
    ptr.cmd("move 640 460")
    time.sleep(1)
    before = dev.lock()
    case.check("the pointer is over the canvas", before.get("on_canvas"), "true")
    case.check("nothing requested before any input", before.get("requested"), "false")
    case.note("frame watch (the way a release is found)", before.get("frame_watch"))
    if args.control:
        case.check("the frame watch is off with CORDIAL_DRAG_LOCK=0", before.get("frame_watch"), "false")
    else:
        case.check("a frame-watching pointer exists on Hyprland", before.get("frame_watch"), "true")

    print("\n-- a LEFT drag must not take the pointer")
    ptr.cmd("down left")
    case.check("no request during a left drag", dev.lock().get("requested"), "false")
    ptr.cmd("up left")

    for n in range(1, args.drags + 1):
        print(f"\n-- right drag {n}")
        mark = len(log_text(log_path))
        ptr.cmd("down right", 0.5)
        during = dev.lock()
        if args.control:
            case.check("no lock requested during the drag (0.28.0)", during.get("requested"), "false")
        else:
            case.check("the lock was requested", during.get("requested"), "true")
            case.check("and Hyprland confirmed it", during.get("confirmed"), "true")
            case.check("the engine still believes the button is down",
                       str(int(during.get("buttons", "0")) & BUTTON_SECONDARY), str(BUTTON_SECONDARY))
        for _ in range(4):
            ptr.cmd("rel 30 5", 0.1)
        ptr.cmd("up right", 0.6)
        after = dev.lock()
        text = log_text(log_path)[mark:]
        moves = [m for m in re.findall(r"nativePassMouseMove\(x=[\d.]+, y=[\d.]+, dx=(-?[\d.]+), dy=(-?[\d.]+)\)", text)
                 if float(m[0]) != 0]
        ups = len(re.findall(r"nativePassMouseButton\(.*down=false, android=2", text))
        downs = len(re.findall(r"nativePassMouseButton\(.*down=true, android=2", text))
        if not args.control:
            case.check("camera motion reached the engine under the lock", len(moves) >= 4, True)
        case.check("the engine saw one press", downs, 1)
        case.check("and exactly one release", ups, 1)
        case.check("the button is up", str(int(after.get("buttons", "0")) & BUTTON_SECONDARY), "0")
        case.check("the lock is given back", after.get("requested"), "false")
        case.check("and nothing is still held for it", after.get("held_for_lock"), "0")
        # Hyprland hands pointer focus back to the canvas on the next movement,
        # not on the lock's release, so a real hand's nudge is part of the drag.
        ptr.cmd("rel 3 0", 0.4)
        case.check("the canvas has the pointer again after a nudge",
                   dev.lock().get("on_canvas"), "true")

    if not args.control:
        extra_gestures(case, dev, ptr, log_path)

    print("\n-- the engine's own request (shift lock) still takes the lock")
    dev.send("fakeenginelock true")
    time.sleep(0.8)
    eng = dev.lock()
    case.check("requested", eng.get("requested"), "true")
    case.check("confirmed", eng.get("confirmed"), "true")
    dev.send("fakeenginelock false")
    time.sleep(0.5)
    case.check("and released again", dev.lock().get("requested"), "false")
    dev.send("fakeenginelock clear")


def engine_buttons(text):
    """(presses, releases) per Android button, as the engine was told."""
    out = {}
    for down, android in re.findall(r"nativePassMouseButton\(.*down=(true|false), android=(\d+)", text):
        d = out.setdefault(int(android), [0, 0])
        d[0 if down == "true" else 1] += 1
    return out


def extra_gestures(case, dev, ptr, log_path):
    """The shapes a real hand makes that a clean press-move-release does not."""
    print("\n-- a quick right click, five times: one press and one release each, nothing stuck")
    mark = len(log_text(log_path))
    for _ in range(5):
        ptr.cmd("click right", 0.5)
        ptr.cmd("rel 2 0", 0.3)
    got = engine_buttons(log_text(log_path)[mark:]).get(BUTTON_SECONDARY, [0, 0])
    case.check("five presses reached the engine", got[0], 5)
    case.check("and five releases", got[1], 5)
    after = dev.lock()
    case.check("nothing held for the lock afterwards", after.get("held_for_lock"), "0")
    case.check("and the lock is not left requested", after.get("requested"), "false")

    print("\n-- a drag held still for three seconds is not ended by the silence")
    mark = len(log_text(log_path))
    ptr.cmd("down right", 0.5)
    ptr.cmd("rel 20 0", 0.1)
    time.sleep(3)
    still = dev.lock()
    case.check("still locked after three quiet seconds", still.get("confirmed"), "true")
    case.check("still held", still.get("held_for_lock"), "2")
    ptr.cmd("rel 20 0", 0.1)
    ptr.cmd("up right", 0.6)
    got = engine_buttons(log_text(log_path)[mark:]).get(BUTTON_SECONDARY, [0, 0])
    case.check("one press and one release", got, [1, 1])
    ptr.cmd("rel 3 0", 0.4)

    print("\n-- the left button pressed and released during a right drag")
    mark = len(log_text(log_path))
    ptr.cmd("down right", 0.5)
    ptr.cmd("click left", 0.4)
    ptr.cmd("rel 10 0", 0.1)
    case.check("the right drag survived a left click", dev.lock().get("held_for_lock"), "2")
    ptr.cmd("up right", 0.6)
    seen = engine_buttons(log_text(log_path)[mark:])
    case.check("the engine saw the left click whole", seen.get(1, [0, 0]), [1, 1])
    case.check("and the right drag whole", seen.get(BUTTON_SECONDARY, [0, 0]), [1, 1])
    ptr.cmd("rel 3 0", 0.4)

    print("\n-- a middle-button drag takes the lock the same way")
    mark = len(log_text(log_path))
    ptr.cmd("down middle", 0.5)
    mid = dev.lock()
    case.check("middle drag locked", mid.get("confirmed"), "true")
    ptr.cmd("rel 20 0", 0.1)
    ptr.cmd("up middle", 0.6)
    seen = engine_buttons(log_text(log_path)[mark:])
    case.check("one middle press and one release", seen.get(4, [0, 0]), [1, 1])
    case.check("lock given back", dev.lock().get("requested"), "false")
    ptr.cmd("rel 3 0", 0.4)

    print("\n-- twenty quick drags in a row")
    mark = len(log_text(log_path))
    for _ in range(20):
        ptr.cmd("down right", 0.25)
        ptr.cmd("rel 15 2", 0.05)
        ptr.cmd("up right", 0.25)
        ptr.cmd("rel 2 0", 0.15)
    got = engine_buttons(log_text(log_path)[mark:]).get(BUTTON_SECONDARY, [0, 0])
    case.check("twenty presses and twenty releases", got, [20, 20])
    case.check("nothing is left held", dev.lock().get("held_for_lock"), "0")


if __name__ == "__main__":
    sys.exit(main())
