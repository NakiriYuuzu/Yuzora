"""Part A of the update probe: a lower bound for the swap, without Yuzora.

A program inside folder X (`Yuzora\\`) puts a staged copy in place of its own folder the
way src/update.rs does (remove X.old, rename X to X.old, rename X.new to X) and reopens
itself with the same PowerShell line, three rounds in a row. winprobe.rs is that program.
If this fails, the real app fails; if it passes, nothing is promised about the real app
(part B looks at that).

  A1  nothing but the program itself is in X; it was started with another folder as cwd
  A2  as A1, started with X as its cwd (what the Start menu shortcut does)
  A3  as A1, and a second program that lives on keeps X as its cwd (what herdr-core would)

usage: python lower_bound.py --probe winprobe.exe --out <dir> --work <dir>
"""
import argparse
import json
import os
import shutil
import subprocess
import time

import common
from common import FAIL, INFO, NOT_MEASURED, PASS, PROBE_ERROR, Run, Tracker, WinProbe, wait_for

ROUNDS = 3
TERMINAL = {"done", "swap_failed", "error"}


def events(path):
    out = []
    for line in common.read_text(path).splitlines():
        try:
            out.append(json.loads(line))
        except ValueError:
            pass
    return out


def step_text(event):
    result = event.get("result") or event.get("copy") or event.get("remove_old") or {}
    if result.get("ok") is False:
        return "FAILED: %s" % result.get("error")
    return "ok"


def swap_outcome(evs, round_number):
    """What the process of that round did, from its events: (verdict, text)."""
    mine = [e for e in evs if e.get("ev") == "start" and e.get("round") == round_number]
    if not mine:
        return NOT_MEASURED, "round %d never started (the previous round did not reopen the program)" % round_number
    pid = mine[0]["pid"]
    own = {e["ev"]: e for e in evs if e.get("pid") == pid}
    steps = []
    for key, label in [("stage", "stage copy"), ("swap_remove_old", "remove X.old"), ("swap_root_to_old", "rename X -> X.old"), ("swap_new_to_root", "rename X.new -> X"), ("relaunch", "powershell relaunch")]:
        if key in own:
            steps.append("%s %s" % (label, step_text(own[key])))
    if "swap_failed" in own:
        return FAIL, "; ".join(steps)
    if "relaunch" in own and (own["relaunch"].get("result") or {}).get("ok") is False:
        return FAIL, "; ".join(steps)
    if "swap_new_to_root" in own and (own["swap_new_to_root"].get("result") or {}).get("ok"):
        return PASS, "; ".join(steps)
    if "final_cleanup" in own:
        return INFO, "last round: remove X.old %s" % step_text({"result": own["final_cleanup"].get("remove_old")})
    return FAIL, "stopped early: " + ("; ".join(steps) or "no step logged")


def run_scenario(run, probe, tracker, work, sid, label, cwd_inside, foreign, plain, reader_ok):
    base = os.path.join(work, sid)
    shutil.rmtree(base, ignore_errors=True)
    root = os.path.join(base, "Yuzora")
    elsewhere = os.path.join(base, "elsewhere")
    os.makedirs(root)
    os.makedirs(elsewhere)
    shutil.copy(probe.exe, os.path.join(root, "winprobe.exe"))
    with open(os.path.join(root, "data.txt"), "w") as f:
        f.write("x")
    log = os.path.join(run.out, sid, "chain.jsonl")
    os.makedirs(os.path.dirname(log), exist_ok=True)

    holder = None
    if foreign:
        holder = subprocess.Popen(["ping", "-n", "900", "127.0.0.1"], cwd=root, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        tracker.add(holder.pid)
        time.sleep(1)
        info = probe.procinfo(holder.pid, "%s/foreign-holder-procinfo.json" % sid)
        run.log("%s: foreign holder pid %d cwd=%s" % (sid, holder.pid, (info.get("peb") or {}).get("cwd_final")))

    env = dict(os.environ, PROBE_LOG=log, PROBE_ROUNDS=str(ROUNDS), PROBE_ROUND="1")
    if plain:
        env["PROBE_RELAUNCH"] = "plain"
    first = subprocess.Popen([os.path.join(root, "winprobe.exe")], cwd=root if cwd_inside else elsewhere, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    tracker.add(first.pid)

    def finished():
        evs = events(log)
        # A program that was reopened logs "start" within seconds; 30 s of nothing after "relaunch" means it never came.
        stalled = evs and evs[-1].get("ev") == "relaunch" and time.time() * 1000 - evs[-1].get("ms", 0) > 30000
        return (any(e.get("ev") in TERMINAL for e in evs) or stalled) and evs
    evs = wait_for(finished, 90) or events(log)
    tracker.add(*[e["pid"] for e in evs if "pid" in e])
    tracker.add(*probe.descendants(first.pid))
    ev_name = "%s/chain.jsonl" % sid

    starts = [e for e in evs if e.get("ev") == "start"]
    for number, ordinal in [(1, "first_update"), (2, "second_update")]:
        verdict, text = swap_outcome(evs, number)
        run.result("%s.%s" % (sid, ordinal), verdict, "%s: %s" % (label, text), [ev_name])

    cwd_notes = []
    for e in starts:
        peb = e.get("peb") or {}
        cwd_notes.append("round %d: cwd_std=%s handle=%s dos=%s parent=%s" % (e["round"], e.get("cwd_std"), peb.get("cwd_final"), peb.get("cwd_dos"), e.get("parent_name")))
    inside_old = [e["round"] for e in starts if e["round"] > 1 and ".old" in str((e.get("peb") or {}).get("cwd_final", "")).lower()]
    verdict = INFO if reader_ok else NOT_MEASURED
    note = "reopened programs: cwd on the OLD folder in rounds %s. " % (inside_old or "none") if reader_ok else "cwd reader failed its self-check; "
    run.result("%s.reopened_cwd" % sid, verdict, note + " | ".join(cwd_notes), [ev_name])
    if not starts or starts[-1]["round"] < ROUNDS:
        last = [e for e in evs if e.get("ev") in TERMINAL]
        run.result("%s.reached_round_%d" % (sid, ROUNDS), FAIL, "%s stopped at round %d (%s)" % (label, starts[-1]["round"] if starts else 0, ("ended with " + last[-1]["ev"]) if last else ("nothing more was logged after %s" % (evs[-1]["ev"] if evs else "the start"))), [ev_name])
    else:
        final = [e for e in evs if e.get("ev") == "final_cleanup"]
        removable = (final[0].get("remove_old") or {}) if final else {}
        run.result("%s.reached_round_%d" % (sid, ROUNDS), PASS, "%s: all %d rounds ran; last process could remove X.old: %s" % (label, ROUNDS, removable.get("ok")), [ev_name])
    run.evidence("%s/after.json" % sid, json.dumps({"root": common.inventory(root), "old": common.inventory(root + ".old"), "new": common.inventory(root + ".new")}, indent=2))
    tracker.kill_all()


def control(run, work):
    """Nothing is running: a plain folder rename. If this fails the runner, not Windows, is the problem."""
    src = os.path.join(work, "control", "X")
    os.makedirs(src, exist_ok=True)
    with open(os.path.join(src, "f.txt"), "w") as f:
        f.write("x")
    try:
        os.rename(src, src + ".old")
        run.result("A0.control_rename_idle_folder", PASS, "an idle folder renames", [])
    except OSError as e:
        run.result("A0.control_rename_idle_folder", PROBE_ERROR, "an idle folder did not rename: %s" % e, [])


def reader_selfcheck(run, probe, tracker, work):
    """The cwd reader against a process whose cwd is known."""
    known = os.path.join(work, "reader-check")
    os.makedirs(known, exist_ok=True)
    proc = subprocess.Popen([probe.exe, "hold", "30"], cwd=known, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    tracker.add(proc.pid)
    time.sleep(1.5)
    info = probe.procinfo(proc.pid, "A0/reader-selfcheck.json")
    peb = info.get("peb") or {}
    got = str(peb.get("cwd_final") or "")
    ok = bool(got) and os.path.normcase(os.path.realpath(got.replace("\\\\?\\", ""))).rstrip("\\") == os.path.normcase(os.path.realpath(known)).rstrip("\\")
    if ok:
        run.result("A0.cwd_reader_selfcheck", PASS, "the cwd handle of a known process reads back as %s (image %s)" % (got, peb.get("image")), ["A0/reader-selfcheck.json"])
    else:
        run.result("A0.cwd_reader_selfcheck", PROBE_ERROR, "expected cwd %s, read cwd_final=%s dos=%s" % (known, got, peb.get("cwd_dos")), ["A0/reader-selfcheck.json"])
    tracker.kill_all()
    return ok


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--work", required=True)
    args = parser.parse_args()
    run = Run(args.out)
    probe = WinProbe(run, args.probe)
    tracker = Tracker(run, probe)
    work = os.path.abspath(args.work)
    os.makedirs(work, exist_ok=True)
    run.log("lower bound: winprobe=%s work=%s" % (probe.exe, work))
    try:
        run.evidence("env/winprobe-ver.json", json.dumps(probe.version()))
        run.guard("A0.control_rename_idle_folder", lambda: control(run, work))
        reader_ok = bool(run.guard("A0.cwd_reader_selfcheck", lambda: reader_selfcheck(run, probe, tracker, work)))
        # Each twice: reopened with the path the app uses (\\?\ prefix), and without it, so that a
        # reopen that fails on the prefix does not hide what the later swaps do.
        for plain in (False, True):
            for sid, label, inside, foreign in [
                ("A1.exe_only", "the program alone in X, cwd elsewhere", False, False),
                ("A2.own_cwd_inside", "the program alone in X, cwd = X", True, False),
                ("A3.foreign_cwd_inside", "cwd elsewhere, another live program has X as its cwd", False, True),
            ]:
                name = sid + (".plainpath" if plain else "")
                run.guard(name, lambda: run_scenario(run, probe, tracker, work, name, label + (" (reopened without the \\\\?\\ prefix)" if plain else ""), inside, foreign, plain, reader_ok))
    finally:
        tracker.kill_all()


if __name__ == "__main__":
    main()
