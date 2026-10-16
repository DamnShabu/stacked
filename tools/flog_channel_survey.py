#!/usr/bin/env python3
"""Which FLog/DFLog channels take a number and which take a severity name (#13).

`flog_channel_shapes.py` reads the shape off Roblox's own settings document,
which names about 140 channels. The engine defines roughly 740. This tool covers
the rest by measurement: give every channel a number in one run and a severity
name in another, and a channel whose lines appear in only one of them has told
you which it takes.

That works because the wrong shape silences a channel instead of erroring
(`FLogNativeDM`: 29 lines absent, 0 with `"100"`, 30 with `"Verbose"`). Run the
control first. If `FLogNativeDM` does not behave like that on the build in
front of you, the scores below it mean nothing.

    # 1. The channel names the engine can log under, from its own format strings.
    flog_channel_survey.py inventory --binary libroblox.so --settings clientsettings.json

    # 2. One flags.json per shape, covering every channel in step 1.
    flog_channel_survey.py flags --shape number   --binary libroblox.so > number.json
    flog_channel_survey.py flags --shape severity --binary libroblox.so > severity.json

    # 3. Three signed-out launches of the same build, same length: no flags,
    #    then number.json, then severity.json, as the profile's flags.json.
    #    Keep each run's stdout.

    # 4. Score.
    #    The engine's own log is <profile>/data/files/appData/logs/*_last.log.
    flog_channel_survey.py score --baseline base.log --number number7.log number6.log \\
        --severity severity.log --binary libroblox.so --settings clientsettings.json

**What a score is not.** A channel the run never reached has no lines under any
shape and is `unscored`; that is the usual case for most of them, and says
nothing about the channel. `either` means the channel kept talking under both
shapes. `severity-only` means a bare number silenced it and a severity name did
not. `number-only` would mean the reverse and has not been seen. None of this is
evidence about a channel the signed-out landing page does not touch.

Number runs: pass more than one `--number` log (the survey on 2.738.0.1397 used 7
and 6) so a number outside one channel's range does not read as silencing.
"""

import argparse
import json
import mmap
import re
import sys
from collections import Counter
from pathlib import Path

# Channel names exist in the binary as the prefix of its log format strings,
# `[FLog::NativeDM] ...`. Reading those is reading names, not behaviour: the
# same tags are in every log the engine writes.
TAG = re.compile(rb"\[(DFLog|FLog)::([A-Za-z0-9_]+)\]")
LOG_TAG = re.compile(r"\[(DFLog|FLog)::([A-Za-z0-9_]+)\]")


def channels_in_binary(path: Path) -> set[str]:
    with path.open("rb") as fh, mmap.mmap(fh.fileno(), 0, access=mmap.ACCESS_READ) as mm:
        return {(m.group(1) + m.group(2)).decode() for m in TAG.finditer(mm)}


def settings_shapes(path: Path | None) -> dict[str, str]:
    """Shapes Roblox's own settings document uses, via the sibling tool."""
    if path is None:
        return {}
    sys.path.insert(0, str(Path(__file__).parent))
    import flog_channel_shapes as shapes

    doc = shapes.load_object(path)
    return {
        k: shapes.value_shape(shapes.observed_text(v), k)
        for k, v in doc.items()
        if k.startswith(("FLog", "DFLog"))
    }


def count_lines(log: Path) -> Counter:
    """Lines carrying each channel's tag, as `FLogName`/`DFLogName`."""
    counts: Counter = Counter()
    with log.open(errors="replace") as fh:
        for line in fh:
            seen = {m.group(1) + m.group(2) for m in LOG_TAG.finditer(line)}
            counts.update(seen)
    return counts


def logs(lines: int, baseline: int) -> bool:
    """Whether a run left the channel talking.

    More than nothing, and not less than it said with no override at all: a
    channel that logged 30 lines by default and 8 under an override has been
    cut down, whatever else it did. Where the default is already loud this can
    only say "not silenced", not "raised", which is all the verdict claims.
    """
    return lines > 0 and lines >= baseline


def verdict(number: int, severity: int, baseline: int) -> str:
    n, s = logs(number, baseline), logs(severity, baseline)
    if n and s:
        return "either"
    if s:
        return "severity-only"
    if n:
        return "number-only"
    return "silenced-by-both" if baseline else "unscored"


def agrees(settings_shape: str, scored: str) -> str:
    """Whether a measured verdict contradicts the shape the settings use."""
    """Whether a measured verdict contradicts the shape the settings use.

    `either` contradicts nothing. `severity-only` against a settings value that
    is a number is the case worth reading: the settings document names a number
    and a number did not make the channel log, so the settings value cannot be
    taken as the shape the channel needs.
    """
    base = settings_shape.split("-")[0]
    if base not in ("number", "severity") or scored not in ("severity-only", "number-only"):
        return "-"
    return "agrees" if scored.startswith(base) else "DISAGREES"


def flags_for(channels, shape: str, number: str, severity: str) -> dict:
    value = number if shape == "number" else severity
    return {c: value for c in sorted(channels)}


def inventory(channels, settings) -> list[tuple[str, str, str]]:
    rows = []
    for c in sorted(set(channels) | set(settings)):
        rows.append((c, settings.get(c, "unknown"), "yes" if c in channels else "no"))
    return rows


def score(channels, settings, base: Counter, nums, sev: Counter):
    """`nums` is one Counter per number run; a number counts if any run logged."""
    rows = []
    for c in sorted(set(channels) | set(settings)):
        # The run that did best for the channel, so a number that is merely
        # outside a channel's range (6 against 7) cannot read as silencing.
        best = max((n[c] for n in nums), key=lambda x: (x > 0 and x >= base[c], x))
        v = verdict(best, sev[c], base[c])
        rows.append((c, settings.get(c, "unknown"), base[c], best, sev[c], v, agrees(settings.get(c, "unknown"), v)))
    return rows


def summary(rows) -> str:
    c = Counter(r[5] for r in rows)
    bad = [r[0] for r in rows if r[6] == "DISAGREES"]
    parts = [f"{len(rows)} channels"] + [
        f"{k}: {c[k]}" for k in ("either", "severity-only", "number-only", "silenced-by-both", "unscored")
    ]
    out = "; ".join(parts)
    if bad:
        out += f"\nmeasured shape disagrees with the settings document for: {', '.join(bad)}"
    return out


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)

    def common(sp):
        sp.add_argument("--binary", type=Path, help="libroblox.so")
        sp.add_argument("--settings", type=Path, help="cached clientsettings.json")

    inv = sub.add_parser("inventory", help="every channel, with the settings shape where one exists")
    common(inv)
    fl = sub.add_parser("flags", help="a flags.json giving every channel one shape")
    common(fl)
    fl.add_argument("--shape", choices=("number", "severity"), required=True)
    fl.add_argument("--number", default="7", help="the number to use (default 7, as in the issue's example)")
    fl.add_argument("--severity", default="Verbose", help="the severity to use (default Verbose)")
    sc = sub.add_parser("score", help="classify channels from three logs")
    common(sc)
    sc.add_argument("--baseline", type=Path, required=True)
    sc.add_argument("--number", type=Path, nargs="+", required=True,
                    help="one or more logs from runs where every channel was given a bare number")
    sc.add_argument("--names-out", type=Path,
                    help="write the severity-only channel names here, one a line")
    sc.add_argument("--severity", type=Path, required=True)

    a = p.parse_args(argv)
    channels = channels_in_binary(a.binary) if a.binary else set()
    try:
        settings = settings_shapes(a.settings)
    except (OSError, ValueError) as exc:
        p.error(str(exc))

    if a.cmd == "inventory":
        print("channel\tsettings_shape\tin_binary")
        for row in inventory(channels, settings):
            print("\t".join(row))
    elif a.cmd == "flags":
        if not channels:
            p.error("--binary is required to know which channels exist")
        json.dump(flags_for(channels, a.shape, a.number, a.severity), sys.stdout, indent=1)
        print()
    else:
        rows = score(channels, settings, count_lines(a.baseline), [count_lines(n) for n in a.number],
                     count_lines(a.severity))
        if a.names_out:
            a.names_out.write_text("".join(f"{r[0]}\n" for r in rows if r[5] == "severity-only"))
        print("channel\tsettings_shape\tbaseline_lines\tnumber_lines\tseverity_lines\tverdict\tvs_settings")
        for r in rows:
            print("\t".join(str(x) for x in r))
        print(summary(rows), file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
