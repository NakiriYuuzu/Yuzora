#!/usr/bin/env python3
"""What one reload of the Files sidebar costs, with no window.

    scripts/files_bench.py fixtures --dir D
    scripts/files_bench.py run --dir D --example <reload_cost binary> --out a.json
    scripts/files_bench.py compare a.json
    scripts/files_bench.py sub --dir D --example <reload_cost binary> --out a.json --sub d00

`fixtures` builds two git repos under D (small ~5k files, large ~30k) with a few
changed files. `run` drives `reload_cost` (crates/yuzora-files/examples) over
{small, large} x {0, 8 folders open} x {this machine, a simulated one}, and
counts, in a separate pass, the processes a reload starts and the `hc` round trips. Each
case runs the sequence the mod used to (`serial`) and the one it uses now (`parallel`)
alternately, so load on the machine costs both the same.

The simulated machine is a throwaway hc session (`HC_SESSION`, `XDG_*` under D)
that reaches "another" session through `hc machine add --via 'sh -c'`; it needs
no network. `--rtt-ms` adds that much to every `hc` call, as a real link would.
The real core is never touched: the script refuses to stop a core whose pid
matches the one the user's own environment names.

Each metric is reported as median / quartiles / p95 / min / max over the runs. A
difference smaller than the interquartile range of either side is not an improvement:
`compare` says so.
"""

import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

OPEN_8 = ["d00", "d00/s0", "d01", "d01/s0", "d02", "d02/s0", "d03", "d03/s0"]
SIZES = {"small": 20, "large": 120}


def sh(*cmd, cwd=None, env=None, check=True):
    return subprocess.run(cmd, cwd=cwd, env=env, check=check, capture_output=True, text=True)


def make_fixture(path, per_leaf):
    """24 top folders x 10 sub folders x `per_leaf` files, a dozen files at the root."""
    shutil.rmtree(path, ignore_errors=True)
    os.makedirs(path)
    for i in range(12):
        open(os.path.join(path, f"root{i:02}.txt"), "w").write(f"{i}\n")
    for d in range(24):
        for s in range(10):
            leaf = os.path.join(path, f"d{d:02}", f"s{s}")
            os.makedirs(leaf)
            for f in range(per_leaf):
                open(os.path.join(leaf, f"f{f:03}.txt"), "w").write(f"{d}{s}{f}\n")
        for f in range(20):
            open(os.path.join(path, f"d{d:02}", f"top{f:02}.txt"), "w").write(f"{d}{f}\n")
    ident = ["-c", "user.name=bench", "-c", "user.email=bench@example.invalid"]
    sh("git", "init", "-q", cwd=path)
    sh("git", "add", "-A", cwd=path)
    sh("git", *ident, "commit", "-q", "-m", "fixture", cwd=path)
    for i in range(20):
        open(os.path.join(path, "d00", "s0", f"f{i:03}.txt"), "a").write("changed\n")
        open(os.path.join(path, "d01", f"new{i:02}.txt"), "w").write("new\n")


def files_in(path):
    return int(sh("git", "ls-files", cwd=path).stdout.count("\n"))


class Remote:
    """An isolated hc session with one machine, `lab`, that is another session of the same core."""

    def __init__(self, home):
        self.env = dict(os.environ, HC_SESSION=f"yzbench{os.getpid()}", **{
            f"XDG_{k}_HOME": os.path.join(home, "hc", k.lower()) for k in ("CONFIG", "DATA", "STATE")})
        self.env["XDG_RUNTIME_DIR"] = os.path.join(home, "hc", "run")
        for k in ("CONFIG", "DATA", "STATE"):
            os.makedirs(self.env[f"XDG_{k}_HOME"], exist_ok=True)
        os.makedirs(self.env["XDG_RUNTIME_DIR"], exist_ok=True)
        self.real_pid = json.loads(sh("hc", "server", "status").stdout).get("pid")
        mine = json.loads(sh("hc", "server", "status", env=self.env).stdout).get("pid")
        if mine == self.real_pid:
            sys.exit("the isolated hc session is the user's own core: refusing to go on")
        sh("hc", "machine", "add", "--via", "sh -c", "--name", "lab", "--session", f"yzbenchb{os.getpid()}",
           "--temp", "--json", env=self.env)

    def close(self):
        mine = json.loads(sh("hc", "server", "status", env=self.env).stdout).get("pid")
        if mine and mine != self.real_pid:
            sh("hc", "server", "stop", env=self.env, check=False)


def wrappers(dir_, log, rtt_ms):
    """Counting stand-ins for git and sh (PATH), and an hc that waits rtt_ms and counts."""
    os.makedirs(dir_, exist_ok=True)
    real = {p: shutil.which(p) for p in ("git", "sh", "hc")}
    for name in ("git", "sh"):
        path = os.path.join(dir_, name)
        open(path, "w").write(f"#!/bin/sh\necho {name} >> '{log}'\nexec {real[name]} \"$@\"\n")
        os.chmod(path, 0o755)
    hc = os.path.join(dir_, "hc")
    open(hc, "w").write(f"#!/bin/sh\necho hc >> '{log}'\nsleep {rtt_ms / 1000}\nexec {real['hc']} \"$@\"\n")
    os.chmod(hc, 0o755)
    return hc


def reload_cost(example, root, on, open_, only, runs, env, hc=None, sub=None):
    cmd = [example, "--root", root, "--open", ",".join(open_), "-n", str(runs)]
    if sub:
        cmd += ["--sub", sub]
    if only:
        cmd += ["--only", only]
    if on:
        cmd += ["--on", on]
    if hc:
        cmd += ["--hc", hc]
    out = subprocess.run(cmd, env=env, check=True, capture_output=True, text=True).stdout
    return json.loads(out.strip().splitlines()[-1])


def stats(v):
    v = sorted(v)
    at = lambda q: v[min(len(v) - 1, int(round(q * (len(v) - 1))))]
    return {"n": len(v), "median": round(statistics.median(v), 1), "min": v[0], "p25": at(0.25), "p75": at(0.75),
            "p95": at(0.95), "max": v[-1]}


def cmd_fixtures(a):
    os.makedirs(a.dir, exist_ok=True)
    for name, per_leaf in SIZES.items():
        make_fixture(os.path.join(a.dir, name), per_leaf)
        print(f"{name}: {files_in(os.path.join(a.dir, name))} files", flush=True)


def cmd_run(a):
    home = tempfile.mkdtemp(prefix="yzbench-", dir=a.dir)
    base = dict(os.environ, YUZORA_APP_HOME=os.path.join(home, "app"))
    remote = None if a.local_only else Remote(home)
    result = {"rtt_ms": a.rtt_ms, "runs": a.runs, "cases": {}}
    try:
        for size in SIZES:
            root = os.path.join(a.dir, size)
            for open_n, open_ in ((0, []), (8, OPEN_8)):
                for where in ([None] if remote is None else [None, "lab"]):
                    env = dict(remote.env, YUZORA_APP_HOME=base["YUZORA_APP_HOME"]) if where else base
                    hc = wrappers(os.path.join(home, "rtt"), os.path.join(home, "rtt.log"), a.rtt_ms) if where and a.rtt_ms else None
                    timed = reload_cost(a.example, root, where, open_, None, a.runs, env, hc)
                    case = {}
                    for side in ("serial", "parallel"):
                        log = os.path.join(home, "count.log")
                        open(log, "w").close()
                        wbin = os.path.join(home, "count")
                        hcw = wrappers(wbin, log, 0)
                        counting = dict(env, PATH=wbin + os.pathsep + env["PATH"])
                        reload_cost(a.example, root, where, open_, side, 1, counting, hcw if where else None)
                        calls = open(log).read().split()
                        # One warm-up reload and one measured one.
                        count = {k: calls.count(k) // 2 for k in ("hc", "git", "sh")}
                        case[side] = {"t_tree": stats(timed[side]["t_tree"]), "t_full": stats(timed[side]["t_full"]),
                                      "hc_roundtrips": count["hc"], "spawns": sum(count.values())}
                    key = f"{size}/open{open_n}/{where or 'local'}"
                    result["cases"][key] = case
                    for side, c in case.items():
                        print(f"{key:22} {side:8} t_tree {c['t_tree']['median']:>7} [{c['t_tree']['p25']}..{c['t_tree']['p75']}]  "
                              f"t_full {c['t_full']['median']:>7} [{c['t_full']['p25']}..{c['t_full']['p75']}]  "
                              f"hc {c['hc_roundtrips']}  spawns {c['spawns']}", flush=True)
    finally:
        if remote:
            remote.close()
        shutil.rmtree(home, ignore_errors=True)
    json.dump(result, open(a.out, "w"), indent=1)


def cmd_sub(a):
    """The first view of a folder inside a repo: when its rows can be used, and when the repo's top replaces them."""
    home = tempfile.mkdtemp(prefix="yzbench-", dir=a.dir)
    base = dict(os.environ, YUZORA_APP_HOME=os.path.join(home, "app"))
    remote = None if a.local_only else Remote(home)
    result = {"rtt_ms": a.rtt_ms, "runs": a.runs, "sub": a.sub, "cases": {}}
    try:
        for size in SIZES:
            root = os.path.join(a.dir, size)
            for where in ([None] if remote is None else [None, "lab"]):
                env = dict(remote.env, YUZORA_APP_HOME=base["YUZORA_APP_HOME"]) if where else base
                hc = wrappers(os.path.join(home, "rtt"), os.path.join(home, "rtt.log"), a.rtt_ms) if where and a.rtt_ms else None
                timed = reload_cost(a.example, root, where, [], None, a.runs, env, hc, sub=a.sub)
                key = f"{size}/{where or 'local'}"
                result["cases"][key] = {order: {m: stats(timed[order][m]) for m in ("t_tree", "t_full")} for order in ("serial", "previous", "now")}
                for order, c in result["cases"][key].items():
                    print(f"{key:14} {order:8} usable {c['t_tree']['median']:>7} [{c['t_tree']['p25']}..{c['t_tree']['p75']}]  "
                          f"top {c['t_full']['median']:>7} [{c['t_full']['p25']}..{c['t_full']['p75']}]", flush=True)
    finally:
        if remote:
            remote.close()
        shutil.rmtree(home, ignore_errors=True)
    json.dump(result, open(a.out, "w"), indent=1)


def cmd_compare(a):
    data = json.load(open(a.a))
    print(f"{'case':22} {'metric':7} {'serial':>8} {'parallel':>8} {'delta':>8}  verdict")
    for key, case in data["cases"].items():
        o, n = case["serial"], case["parallel"]
        for m in ("t_tree", "t_full"):
            b, x = o[m], n[m]
            delta = x["median"] - b["median"]
            noise = max(b["p75"] - b["p25"], x["p75"] - x["p25"])
            verdict = "within the spread" if abs(delta) <= noise else ("better" if delta < 0 else "worse")
            print(f"{key:22} {m:7} {b['median']:>8} {x['median']:>8} {delta:>+8.1f}  {verdict} (IQR {noise:.1f})")
        for m in ("hc_roundtrips", "spawns"):
            print(f"{key:22} {m:7} {o[m]:>8} {n[m]:>8} {n[m] - o[m]:>+8}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    f = sub.add_parser("fixtures")
    f.add_argument("--dir", required=True)
    r = sub.add_parser("run")
    r.add_argument("--dir", required=True)
    r.add_argument("--example", required=True)
    r.add_argument("--out", required=True)
    r.add_argument("--runs", type=int, default=20)
    r.add_argument("--rtt-ms", type=int, default=0)
    r.add_argument("--local-only", action="store_true")
    s = sub.add_parser("sub")
    s.add_argument("--dir", required=True)
    s.add_argument("--example", required=True)
    s.add_argument("--out", required=True)
    s.add_argument("--sub", required=True)
    s.add_argument("--runs", type=int, default=20)
    s.add_argument("--rtt-ms", type=int, default=0)
    s.add_argument("--local-only", action="store_true")
    c = sub.add_parser("compare")
    c.add_argument("a")
    a = ap.parse_args()
    {"fixtures": cmd_fixtures, "run": cmd_run, "sub": cmd_sub, "compare": cmd_compare}[a.cmd](a)


if __name__ == "__main__":
    main()
