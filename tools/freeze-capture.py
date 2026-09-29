#!/usr/bin/env python3
"""Photograph a wedged cordial-run without disturbing it. Usage:

    tools/freeze-capture.py PID OUTDIR [PROFILE]

Why this exists rather than the MCP: `cordial_backtrace` gives the stacks, but
nothing in the MCP reads the *state the stacks are waiting on*, and the startup
freeze (docs/NEXT.md, 2026-09-29) turned out to be a question about state. Two
rounds of NEXT entries reasoned from a handful of syscall lines and one
`ss -tnp`; this takes the rest in one pass, reading `/proc` only until the final
gdb step so the specimen is unchanged while it is measured.

Written to files under OUTDIR:

  threads.txt   comm, state, CPU over 4 s, wchan and three syscall samples per thread
  pollfds.txt   the fd array each thread inside poll/ppoll is waiting on. A 1 ms
                poll loop rewrites its array constantly, so this takes up to 300
                snapshots and keeps the ones whose fds are all live descriptors.
  futex.txt     the words around every futex a thread is blocked on
  wrappers.txt  every live Cordial condition-variable wrapper (bionic::pthread),
                its glibc backing object decoded, and any thread whose futex sits
                inside one. `waiters > 0` beside pending `g_signals` on a cond
                is a lost wakeup; healthy clients show neither (92-94 wrappers,
                no duplicates, none anomalous, on 2026-09-29).
  sockets.txt   `ss -tnpei` rows for this pid (tcp_info: bytes, last-rx times),
                the socket fds, and which epoll instance registers which fd
  devctl.txt    `info` and `loopers` from the control socket, if it is up
  gdb.txt       `thread apply all bt 30` with unresolved frames mapped to
                module+offset through /proc/PID/maps

Reads process memory through /proc/PID/mem, which needs the same uid and
`ptrace_scope` 0 (as on this host). It does not signal, write to or stop the
process until gdb attaches at the end. Do not run it against a client whose
profile is somebody else's.

The measurement it was built to take and has not yet taken: every capture so
far of a frozen client predates it. Its healthy-client output was checked on
eight clients; its frozen-client output has never been seen.
"""
import os, re, struct, subprocess, sys, time, collections

pid = int(sys.argv[1]); out = sys.argv[2]
profile = sys.argv[3] if len(sys.argv) > 3 else "CordialTest"
os.makedirs(out, exist_ok=True)
P = f"/proc/{pid}"
READY = 0xC0D1A1FF

def w(name, text):
    open(f"{out}/{name}", "w").write(text)

def read(path):
    try:
        return open(path).read()
    except OSError as e:
        return f"<{e}>"

# ---------------------------------------------------------------- maps
regions = []
for line in read(f"{P}/maps").splitlines():
    m = re.match(r"([0-9a-f]+)-([0-9a-f]+) (\S+) ([0-9a-f]+) \S+ \d+\s*(.*)", line)
    if m:
        regions.append((int(m[1], 16), int(m[2], 16), m[3], int(m[4], 16), m[5]))

def where(a):
    for lo, hi, perm, off, name in regions:
        if lo <= a < hi:
            base = f"{name or 'anon'} {perm}"
            if name:
                return f"{base} +{a - lo + off:#x}"
            return f"{base} region {lo:#x}-{hi:#x} (+{a - lo:#x})"
    return "unmapped"

mem = open(f"{P}/mem", "rb", 0)
def rd(a, n):
    try:
        mem.seek(a)
        return mem.read(n)
    except (OSError, ValueError, OverflowError):
        return None

# ---------------------------------------------------------------- threads
tids = sorted(int(t) for t in os.listdir(f"{P}/task"))
def stat_cpu(t):
    s = read(f"{P}/task/{t}/stat")
    try:
        f = s[s.rindex(")") + 2:].split()
        return f[0], int(f[11]), int(f[12])
    except Exception:
        return "?", 0, 0

samples = collections.defaultdict(list)
cpu0 = {t: stat_cpu(t) for t in tids}
for i in range(3):
    for t in tids:
        samples[t].append(read(f"{P}/task/{t}/syscall").strip())
    time.sleep(0.4)
time.sleep(4.0)
cpu1 = {t: stat_cpu(t) for t in tids}
hz = os.sysconf("SC_CLK_TCK")
lines = []
for t in tids:
    comm = read(f"{P}/task/{t}/comm").strip()
    wch = read(f"{P}/task/{t}/wchan").strip()
    st, u0, s0 = cpu0[t]; _, u1, s1 = cpu1[t]
    busy = ((u1 - u0) + (s1 - s0)) / hz / 4.0 * 100
    lines.append(f"tid={t} comm={comm!r} state={st} cpu={busy:.1f}% wchan={wch}")
    for s in samples[t]:
        lines.append(f"    {s}")
w("threads.txt", "\n".join(lines) + "\n")

# ---------------------------------------------------------------- fd table / sockets / epoll
fds = {}
for fd in os.listdir(f"{P}/fd"):
    try:
        fds[int(fd)] = os.readlink(f"{P}/fd/{fd}")
    except OSError:
        pass
ss = subprocess.run(["ss", "-tnpei"], capture_output=True, text=True).stdout
ss_rows = []
cur = None
for l in ss.splitlines():
    if l.startswith(("State", "Netid")):
        continue
    if l[:1].isspace():
        if cur is not None:
            cur += " " + l.strip()
    else:
        if cur:
            ss_rows.append(cur)
        cur = l.strip()
if cur:
    ss_rows.append(cur)
mine = [r for r in ss_rows if f"pid={pid}," in r]
ino_state = {}
for r in mine:
    mi = re.search(r"ino:(\d+)", r)
    if mi:
        ino_state[int(mi[1])] = r
sock_lines = ["== ss rows for this pid (state recv-q send-q local peer, timers) =="]
for r in mine:
    sock_lines.append(re.sub(r"\s+cgroup:\S+", "", r))
sock_lines.append("\n== fd table (sockets and epoll/event fds only) ==")
epolls = {}
for fd, tgt in sorted(fds.items()):
    if tgt.startswith("socket:"):
        ino = int(re.search(r"\[(\d+)\]", tgt)[1])
        row = ino_state.get(ino, "(not in ss -t: unix/udp/netlink?)")
        sock_lines.append(f"fd={fd} {tgt} -> {row[:110]}")
    elif "eventpoll" in tgt:
        info = read(f"{P}/fdinfo/{fd}")
        tfd = re.findall(r"tfd:\s+(\d+) events:\s+([0-9a-f]+) data:\s+([0-9a-f]+)", info)
        epolls[fd] = tfd
        sock_lines.append(f"fd={fd} {tgt} registers {len(tfd)} fds")
sock_lines.append("\n== epoll instances: registered fds, mask, target ==")
for efd, tfd in epolls.items():
    sock_lines.append(f"epoll fd={efd}:")
    for tf, ev, data in tfd:
        tgt = fds.get(int(tf), "?")
        extra = ""
        if tgt.startswith("socket:"):
            ino = int(re.search(r"\[(\d+)\]", tgt)[1])
            r = ino_state.get(ino)
            if r:
                extra = " " + " ".join(r.split()[:5])
        sock_lines.append(f"    fd={tf} events={ev} {tgt}{extra}")
w("sockets.txt", "\n".join(sock_lines) + "\n")

# ---------------------------------------------------------------- pollfds / futex words
poll_lines = []
futex_lines = []
futex_addrs = {}
for t in tids:
    for s in samples[t][-1:]:
        p = s.split()
        if not p or p[0] == "running":
            continue
        try:
            nr = int(p[0]); args = [int(x, 16) for x in p[1:7]]
        except Exception:
            continue
        comm = read(f"{P}/task/{t}/comm").strip()
        if nr in (7, 271):  # poll, ppoll
            # A 1 ms poll loop rewrites the array between our reads, so take many
            # snapshots and keep the ones whose fds are all live descriptors.
            seen = collections.Counter()
            for _ in range(300):
                sc = read(f"{P}/task/{t}/syscall").split()
                if not sc or sc[0] not in ("7", "271"):
                    continue
                a0, a1 = int(sc[1], 16), int(sc[2], 16)
                raw = rd(a0, 8 * a1) if 0 < a1 < 64 else None
                if not raw:
                    continue
                ent = []; ok = True
                for i in range(a1):
                    fd, ev, rev = struct.unpack_from("<iHH", raw, 8 * i)
                    if fd not in fds:
                        ok = False; break
                    ent.append(f"fd={fd}({fds[fd]}) events={ev:#x}")
                if ok:
                    tmo = int(sc[3], 16) if sc[0] == "7" else "ppoll-timespec"
                    seen[f"nfds={a1} timeout={tmo}: " + "; ".join(ent)] += 1
                time.sleep(0.002)
            if not seen:
                poll_lines.append(f"tid={t} {comm!r} nr={nr}: no self-consistent snapshot in 300 tries")
            for k, c in seen.most_common(4):
                poll_lines.append(f"tid={t} {comm!r} nr={nr} seen {c}x {k}")
        if nr == 202:
            a = args[0]
            futex_addrs[t] = (a, args[1], args[2])
            blob = rd(a - 0x60, 0xC0)
            hexs = ""
            if blob:
                for i in range(0, len(blob), 16):
                    words = struct.unpack_from("<4I", blob, i)
                    mark = "  <-- futex word" if a - 0x60 + i <= a < a - 0x60 + i + 16 else ""
                    hexs += f"      {a - 0x60 + i:#x}: " + " ".join(f"{x:08x}" for x in words) + mark + "\n"
            futex_lines.append(f"tid={t} {comm!r} futex addr={a:#x} op={args[1]:#x} val={args[2]:#x} [{where(a)}]\n{hexs}")
w("pollfds.txt", "\n".join(poll_lines) + "\n")
w("futex.txt", "\n".join(futex_lines) + "\n")

# ---------------------------------------------------------------- cond wrapper scan
pat = struct.pack("<I", READY)
found = []
scanned = 0
t0 = time.time()
for lo, hi, perm, off, name in regions:
    if not perm.startswith("rw") or name.startswith(("/dev/", "[vvar", "[vsyscall")):
        continue
    if "dri" in name or "nvidia" in name or hi - lo > (1 << 31):
        continue
    a = lo
    while a < hi:
        n = min(hi - a, 64 << 20)
        buf = rd(a, n)
        if buf is None:
            a += n
            continue
        scanned += len(buf)
        i = buf.find(pat)
        while i != -1:
            if i % 4 == 0 and i + 12 <= len(buf):
                lo32, hi32 = struct.unpack_from("<II", buf, i + 4)
                ptr = lo32 | (hi32 << 32)
                if 0x7f0000000000 <= ptr < 0x800000000000:
                    found.append((a + i, ptr))
            i = buf.find(pat, i + 1)
        a += n
scan_s = time.time() - t0
wl = [f"scanned {scanned/1e6:.0f} MB of rw regions in {scan_s:.1f}s; {len(found)} READY-marker candidates"]
backing_count = collections.Counter(p for _, p in found)
dups = {p: c for p, c in backing_count.items() if c > 1}
wl.append(f"backings referenced more than once (copied cond?): {len(dups)}")
for p, c in dups.items():
    wl.append(f"    backing {p:#x} referenced {c}x by " + ", ".join(f"{a:#x}" for a, q in found if q == p))
suspicious = []
for wa, ptr in found:
    raw = rd(ptr, 48)
    if not raw or len(raw) < 48:
        wl.append(f"wrapper {wa:#x} [{where(wa)}] -> backing {ptr:#x} UNREADABLE")
        continue
    wseq, g1s, r0, r1, s0, s1, orig, wrefs, sig0, sig1 = struct.unpack("<QQIIIIIIII", raw)
    waiters = wrefs >> 3
    tag = ""
    if waiters or sig0 >> 1 or sig1 >> 1:
        tag = "  <== waiters/signals non-zero"
        suspicious.append(wa)
    wl.append(f"wrapper {wa:#x} [{where(wa)}] -> backing {ptr:#x} wseq={wseq:#x} g1_start={g1s:#x} "
              f"g_refs=({r0},{r1}) g_size=({s0},{s1}) orig={orig:#x} wrefs={wrefs:#x}(waiters={waiters}) "
              f"g_signals=({sig0:#x},{sig1:#x}){tag}")
wl.append("")
for t, (a, op, val) in futex_addrs.items():
    hit = [(wa, p) for wa, p in found if p <= a < p + 128]
    if hit:
        wl.append(f"tid={t} futex {a:#x} is inside backing of wrapper(s): " + ", ".join(f"{wa:#x}->{p:#x}(+{a-p:#x})" for wa, p in hit))
    else:
        wl.append(f"tid={t} futex {a:#x} op={op:#x} val={val:#x} is NOT inside any Cordial cond backing")
w("wrappers.txt", "\n".join(wl) + "\n")

# ---------------------------------------------------------------- devctl
try:
    import socket as _s
    data_home = os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share"))
    sockpath = f"{data_home}/cordial/profiles/{profile}/devctl.sock"
    dv = []
    for cmd in ("info", "loopers"):
        c = _s.socket(_s.AF_UNIX, _s.SOCK_STREAM); c.settimeout(5)
        c.connect(sockpath); c.sendall((cmd + "\n").encode())
        buf = b""
        try:
            while not buf.endswith(b"\n"):
                d = c.recv(65536)
                if not d:
                    break
                buf += d
        except Exception as e:
            buf += f"<{e}>".encode()
        dv.append(f"== {cmd}\n{buf.decode(errors='replace')}")
        c.close()
    w("devctl.txt", "\n".join(dv))
except Exception as e:
    w("devctl.txt", f"<{e}>\n")

# ---------------------------------------------------------------- gdb
gdbcmds = f"{out}/gdb.cmds"
open(gdbcmds, "w").write("set pagination off\nset language c\nthread apply all bt 30\ninfo threads\n")
r = subprocess.run(["/home/linuxbrew/.linuxbrew/bin/gdb", "-p", str(pid), "-batch", "-x", gdbcmds],
                   capture_output=True, text=True, timeout=120)
def mapaddr(m):
    a = int(m.group(2), 16)
    wh = where(a)
    return f"{m.group(1)}{m.group(2)} in ?? ()  [{wh}]"
txt = re.sub(r"(#\d+\s+)(0x[0-9a-f]+) in \?\? \(\)", mapaddr, r.stdout)
w("gdb.txt", txt + "\n=== stderr ===\n" + r.stderr[-2000:])
print("capture written to", out)
