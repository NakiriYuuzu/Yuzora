"""Shared pieces of the Windows update probe: result records, raw evidence files, running
programs with a timeout, the winprobe helper, and the list of processes the probe started
(the only ones it ever stops).

A result has one of five states, and the first two are the product's:
  pass          the behaviour an update needs was seen
  fail          the product did the wrong thing (the evidence says what)
  info          something measured that is not a verdict (a path, a count, a state)
  not-measured  could not look (and why): the question is still open
  probe-error   the probe itself broke; says nothing about the product
"""
import json
import os
import signal
import subprocess
import tempfile
import time
import traceback

PASS, FAIL, INFO, NOT_MEASURED, PROBE_ERROR = "pass", "fail", "info", "not-measured", "probe-error"


def now():
    return time.strftime("%H:%M:%S")


class Run:
    """One probe job's output folder: results.jsonl, probe.log and raw evidence files."""

    def __init__(self, out):
        self.out = os.path.abspath(out)
        os.makedirs(self.out, exist_ok=True)
        self.results_path = os.path.join(self.out, "results.jsonl")
        self.log_path = os.path.join(self.out, "probe.log")
        self.started = time.time()

    def log(self, message):
        line = "[%s +%4ds] %s" % (now(), time.time() - self.started, message)
        print(line, flush=True)
        with open(self.log_path, "a", encoding="utf-8") as f:
            f.write(line + "\n")

    def evidence(self, name, content):
        """Writes `content` (text or bytes) under the output folder; returns the relative name."""
        path = os.path.join(self.out, name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        if isinstance(content, bytes):
            with open(path, "wb") as f:
                f.write(content)
        else:
            with open(path, "w", encoding="utf-8", errors="replace") as f:
                f.write(content)
        return name

    def result(self, rid, status, detail, evidence=()):
        record = {"id": rid, "status": status, "detail": detail, "evidence": list(evidence)}
        with open(self.results_path, "a", encoding="utf-8") as f:
            f.write(json.dumps(record, ensure_ascii=False) + "\n")
        self.log("%-13s %s: %s" % (status.upper(), rid, detail))

    def guard(self, rid, fn):
        """Runs fn(); an exception is the probe's fault, not the product's."""
        try:
            return fn()
        except Exception:
            trace = traceback.format_exc()
            name = self.evidence("probe-errors/%s.txt" % rid.replace("/", "_"), trace)
            self.result(rid, PROBE_ERROR, trace.strip().splitlines()[-1], [name])
            return None

    def sh(self, args, timeout, cwd=None, env=None, name=None):
        """Runs a program to completion or to the timeout; keeps its raw output as evidence.
        Returns an object with rc (None after a timeout), out and err."""
        started = time.time()
        timed_out = False
        # Files, not pipes: a program that leaves a background process behind (an installer
        # starting what it installed) would hold a pipe open and the read would never end.
        with tempfile.TemporaryFile() as out_file, tempfile.TemporaryFile() as err_file:
            try:
                rc = subprocess.run([str(a) for a in args], stdout=out_file, stderr=err_file, timeout=timeout, cwd=cwd, env=env, stdin=subprocess.DEVNULL).returncode
            except subprocess.TimeoutExpired:
                rc, timed_out = None, True
            except OSError as e:
                rc = -1
                err_file.write(("could not start: %s" % e).encode())
            out_file.seek(0)
            err_file.seek(0)
            out, err = out_file.read(), err_file.read()
        text_out, text_err = out.decode("utf-8", "replace"), err.decode("utf-8", "replace")
        result = Shell(rc, text_out, text_err, timed_out)
        if name:
            self.evidence(name, "$ %s\ncwd: %s\nrc: %s%s  (%.1fs)\n--- stdout\n%s\n--- stderr\n%s\n" % (
                " ".join('"%s"' % a if " " in str(a) else str(a) for a in args), cwd, rc, " TIMED OUT" if timed_out else "", time.time() - started, text_out, text_err))
        return result


class Shell:
    def __init__(self, rc, out, err, timed_out):
        self.rc, self.out, self.err, self.timed_out = rc, out, err, timed_out

    def json(self):
        """The output as one JSON document, else its last line as one (the helper prints one line)."""
        for text in (self.out.strip(), (self.out.strip().splitlines() or [""])[-1]):
            try:
                return json.loads(text)
            except ValueError:
                pass
        return None


def wait_for(condition, timeout, interval=0.5):
    """The first truthy value of condition() within `timeout` seconds, else None."""
    end = time.time() + timeout
    while True:
        value = condition()
        if value:
            return value
        if time.time() >= end:
            return None
        time.sleep(interval)


class WinProbe:
    """winprobe.exe (winprobe.rs): processes, their cwd, their windows."""

    def __init__(self, run, exe):
        self.run, self.exe = run, os.path.abspath(exe)

    def ps(self):
        shell = self.run.sh([self.exe, "ps"], 30)
        rows = []
        for line in shell.out.splitlines():
            try:
                rows.append(json.loads(line))
            except ValueError:
                pass
        return rows

    def named(self, name):
        return [p for p in self.ps() if p["name"].lower() == name.lower()]

    def alive(self, pid):
        return any(p["pid"] == pid for p in self.ps())

    def procinfo(self, pid, evidence_name=None):
        shell = self.run.sh([self.exe, "procinfo", pid], 30, name=evidence_name)
        return shell.json() or {"pid": pid, "error": "no answer: rc=%s %s" % (shell.rc, shell.err.strip())}

    def windows(self, pid):
        return self.run.sh([self.exe, "windows", pid], 30).json() or []

    def close(self, pid):
        """WM_CLOSE to the process's visible windows: what ending a window app politely does."""
        return self.run.sh([self.exe, "close", pid], 30).json() or {"windows": 0, "posted": 0}

    def version(self):
        return self.run.sh([self.exe, "ver"], 30, name="env/winprobe-ver.txt").json()

    def descendants(self, pid, rows=None):
        rows = rows if rows is not None else self.ps()
        found, frontier = set(), {pid}
        while frontier:
            frontier = {p["pid"] for p in rows if p["ppid"] in frontier and p["pid"] not in found and p["pid"] != pid}
            found |= frontier
        return found


class Tracker:
    """The processes the probe started or saw start; the only ones it stops."""

    def __init__(self, run, probe):
        self.run, self.probe, self.pids = run, probe, set()

    def add(self, *pids):
        self.pids.update(p for p in pids if p)

    # A process id can be reused once its owner is gone: only programs of these names are ever stopped.
    STOPPABLE = {"winprobe.exe", "yuzora.exe", "cmd.exe", "ping.exe", "hc.exe", "herdr-core.exe"}

    def kill_all(self):
        alive = {p["pid"]: p["name"].lower() for p in self.probe.ps()}
        for pid in sorted(self.pids & set(alive)):
            if alive[pid] not in self.STOPPABLE:
                continue
            try:
                os.kill(pid, signal.SIGTERM)  # TerminateProcess on Windows
                self.run.log("stopped pid %d" % pid)
            except OSError as e:
                self.run.log("could not stop pid %d: %s" % (pid, e))
        self.pids.clear()


def tail(text, lines=12):
    return "\n".join(text.strip().splitlines()[-lines:])


def inventory(path):
    """Every file under path with its size, for before/after comparisons."""
    entries = []
    if os.path.isdir(path):
        for base, dirs, files in os.walk(path):
            for name in sorted(files):
                full = os.path.join(base, name)
                try:
                    entries.append({"path": os.path.relpath(full, path), "size": os.path.getsize(full)})
                except OSError:
                    entries.append({"path": os.path.relpath(full, path), "size": None})
    return {"exists": os.path.isdir(path), "files": sorted(entries, key=lambda e: e["path"])}


def read_text(path, default=""):
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            return f.read()
    except OSError:
        return default
