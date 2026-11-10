"""Parts B and C of the update probe: the real yuzora.exe, installed by the real NSIS
installer (/S) into %LOCALAPPDATA%\\Programs\\Yuzora, updating itself.

Two flavors of the same source (the workflow builds each):

  release  scripts/yuzora/package as shipped (HERDR_APP_RELEASE). Its updater trusts only
           the release key, so the probe cannot make it find an update. What it can reach
           is `apply_on_startup`, the crash-recovery path: files staged the way `stage`
           leaves them (Yuzora.new, Yuzora.new.json), then the app is started. That runs
           the real `swap` and the real PowerShell `relaunch_expected`.
  testkey  the release profile with debug assertions and without HERDR_APP_RELEASE, which
           is what makes update.rs `keys` trust YUZORA_UPDATE_TEST_KEY. Everything runs
           for real against a local stand-in for GitHub: check, download, signature,
           checksum, unpack, stage, then swap when the window closes (or, in chain C1,
           `yuzora do app.update`, the Restart button). D2 does the second update from the
           program PowerShell reopened, which needs no herdr-core.

  C2-C5    (testkey, herdr-core installed) an old app's core left alive in the install folder, and the
           installer run again over a working install; see chain_old_core and chain_overlay.

Newer versions come from the built exe itself with its version string replaced by one of
the same length (release_server.py); each is checked with `--version` before use.
Launching is like the Start menu shortcut: cwd = the folder the shortcut says, an ordinary
process. What each step needs and which parts are real is in the report that goes with
this probe; every result points at its raw output.

usage: python observe.py --flavor release|testkey --dist <app/dist> --probe winprobe.exe
                         --out <dir> --work <dir> [--budget seconds]
"""
import argparse
import glob
import json
import os
import re
import shutil
import subprocess
import tarfile
import threading
import time

import common
import release_server as rs
from common import FAIL, INFO, NOT_MEASURED, PASS, PROBE_ERROR, Run, Tracker, WinProbe, inventory, read_text, strip_ansi, tail, wait_for

PROGRAMS = os.path.join(os.environ.get("LOCALAPPDATA", ""), "Programs")
INSTALL = os.path.join(PROGRAMS, "Yuzora")
UNINSTALL = os.path.join(PROGRAMS, "Yuzora-uninstall")
EXE = os.path.join(INSTALL, "yuzora.exe")
CORE_INSTALLER = "irm https://releases.herdr.dev/core/install.ps1 | iex"


def beside(suffix):
    return os.path.join(PROGRAMS, "Yuzora" + suffix)


def norm(path):
    """A comparable form: no verbatim prefix, long names, one case."""
    p = str(path or "")
    if p.startswith("\\\\?\\"):
        p = p[4:]
    return os.path.normcase(os.path.realpath(p)).rstrip("\\")


def version_word(text):
    """`herdr-app 1.2.3` -> 1.2.3"""
    parts = (text or "").strip().split()
    return parts[-1] if len(parts) == 2 and parts[0] == "herdr-app" else None


class Ctx:
    def __init__(self, run, probe, flavor, dist, work, budget):
        self.run, self.probe, self.flavor, self.dist = run, probe, flavor, dist
        self.work = os.path.abspath(work)
        self.tracker = Tracker(run, probe)
        self.deadline = time.time() + budget
        self.elsewhere = os.path.join(self.work, "cwd-elsewhere")
        self.key = rs.TestKey()
        self.server = None
        self.archives = {}
        self.cli = None
        self.core = None  # path of herdr-core.exe once ensure_core installed it
        self.core_tried = False
        self.shots = 0

    def fresh_server(self, *versions):
        """A chain's own server state: nothing offered but `versions` (oldest first, so the last is the newest).
        Without this an earlier chain's newest release is served first and the app stages that one."""
        self.server.reset()
        for version in versions:
            self.offer(version)
        self.run.log("the stand-in server was reset, it offers %s" % (", ".join(versions) or "nothing"))

    def sweep(self):
        """Stops what the chain left running: the app, the core and its helpers (only programs of those names)."""
        for name in ("yuzora.exe", "hc.exe", "herdr-core.exe"):
            self.tracker.add(*[p["pid"] for p in self.probe.named(name)])
        self.tracker.kill_all()

    def time_left(self):
        return self.deadline - time.time()

    def server_entries(self):
        entries = []
        for line in read_text(os.path.join(self.run.out, "server.log")).splitlines():
            try:
                entries.append(json.loads(line))
            except ValueError:
                pass
        return entries

    def ua_seen(self, version, since=0):
        return any(e.get("ua") == "Yuzora/%s" % version and "/releases" in e.get("path", "") for e in self.server_entries()[since:])

    def offer(self, version):
        if not any(v == version for v, _ in self.server.releases):
            with open(self.archives[version], "rb") as f:
                self.server.add_release(version, f.read())
            self.run.log("the stand-in server now offers %s" % version)


# ---- the install ------------------------------------------------------------

def uninstall_all(ctx, cid):
    uninstaller = os.path.join(UNINSTALL, "uninstall.exe")
    if os.path.exists(uninstaller):
        ctx.run.sh([uninstaller, "/S", "_?=" + UNINSTALL], 120, name="%s/uninstall.txt" % cid)
    for path in [INSTALL, beside(".old"), beside(".new"), beside(".download"), UNINSTALL]:
        shutil.rmtree(path, ignore_errors=True)
    try:
        os.remove(beside(".new.json"))
    except OSError:
        pass


def package_list_state():
    names = {line.strip() for line in read_text(os.path.join(INSTALL, "yuzora.package")).splitlines() if line.strip()}
    actual = set(os.listdir(INSTALL)) if os.path.isdir(INSTALL) else set()
    return {"extra": sorted(actual - names), "missing": sorted(names - actual)}


def install_fresh(ctx, cid):
    # A reopen that came late from the last chain, and herdr-core, are ours to stop.
    ctx.sweep()
    time.sleep(2)
    uninstall_all(ctx, cid)
    left = [n for n in os.listdir(PROGRAMS) if n.startswith("Yuzora")] if os.path.isdir(PROGRAMS) else []
    if left:
        ctx.run.result("%s.clean_slate" % cid, PROBE_ERROR, "still there before installing: %s (a process of the last chain holds it)" % left, [])
        raise RuntimeError("no clean slate")
    shell = ctx.run.sh([ctx.installer, "/S"], 180, name="%s/install.txt" % cid)
    state = package_list_state()
    running = ctx.run.sh([EXE, "--version"], 30, name="%s/install-version.txt" % cid)
    ok = shell.rc == 0 and version_word(running.out) == ctx.v1 and not state["extra"] and not state["missing"]
    ctx.run.result("%s.fresh_install" % cid, PASS if ok else PROBE_ERROR, "installer rc=%s, installed exe says %r, folder vs yuzora.package: extra=%s missing=%s" % (shell.rc, running.out.strip(), state["extra"], state["missing"]), ["%s/install.txt" % cid])
    if not ok:
        raise RuntimeError("the installer did not leave a clean install")


def shortcut_check(ctx):
    lnk = os.path.join(os.environ["APPDATA"], "Microsoft", "Windows", "Start Menu", "Programs", "Yuzora.lnk")
    script = "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('%s'); 'target={0} startin={1}' -f $s.TargetPath,$s.WorkingDirectory" % lnk
    shell = ctx.run.sh(["powershell", "-NoProfile", "-NonInteractive", "-Command", script], 60, name="B0/shortcut.txt")
    match = re.search(r"startin=(.*)", shell.out)
    start_in = match.group(1).strip() if match else None
    if start_in is None:
        ctx.run.result("B0.shortcut_start_in", NOT_MEASURED, "could not read the shortcut: %s" % tail(shell.out + shell.err, 4), ["B0/shortcut.txt"])
    elif norm(start_in) == norm(INSTALL):
        ctx.run.result("B0.shortcut_start_in", PASS, "the Start menu shortcut starts the app with cwd = %s, so a user's app has the folder an update renames as its cwd" % start_in, ["B0/shortcut.txt"])
    else:
        ctx.run.result("B0.shortcut_start_in", FAIL, "Start in is %r, not the install folder: the 'cwd inside' chains do not match what users get" % start_in, ["B0/shortcut.txt"])


# ---- running the app --------------------------------------------------------

def app_env(ctx, home):
    env = {k: v for k, v in os.environ.items() if not k.startswith(("HERDR_", "YUZORA_"))}
    env.update(YUZORA_APP_HOME=home, YUZORA_UPDATE_URL=ctx.server.base, RUST_BACKTRACE="1")
    if ctx.flavor == "testkey":
        env["YUZORA_UPDATE_TEST_KEY"] = ctx.key.public_base64()
    return env


class Instance:
    """A running yuzora.exe: one the probe started (proc, with its log) or one it found (pid)."""

    def __init__(self, ctx, pid, label, proc=None, log_path=None, log_rel=None):
        self.ctx, self.pid, self.label, self.proc, self.log_path, self.log_rel = ctx, pid, label, proc, log_path, log_rel
        ctx.tracker.add(pid)

    def alive(self):
        return self.proc.poll() is None if self.proc else self.ctx.probe.alive(self.pid)

    def wait_exit(self, timeout):
        return wait_for(lambda: not self.alive(), timeout) is not None

    def log_text(self):
        return strip_ansi(read_text(self.log_path)) if self.log_path else ""

    def stop(self):
        """What a person ends a window app with, then the end for one that will not."""
        if self.alive():
            self.ctx.probe.close(self.pid)
            if not self.wait_exit(15):
                self.ctx.tracker.kill_some({self.pid})  # only this one: herdr-core may be a part of what a chain is measuring
                self.wait_exit(10)


def start_app(ctx, cid, label, home, cwd):
    os.makedirs(home, exist_ok=True)
    rel = "%s/%s.log" % (cid, label)
    path = os.path.join(ctx.run.out, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    log = open(path, "wb")
    proc = subprocess.Popen([EXE], cwd=cwd, env=app_env(ctx, home), stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
    ctx.run.log("%s: started pid %d, cwd %s" % (label, proc.pid, cwd))
    return Instance(ctx, proc.pid, label, proc, path, rel)


def update_lines(text, limit=6):
    """The lines of an app log that are about updating (a failed swap says why there, and the app
    goes on to log other things after it)."""
    keep = [line.strip() for line in text.splitlines() if "update" in line.lower() or "panic" in line.lower()]
    return " | ".join(keep[-limit:]) or tail(text, 4).replace("\n", " | ") or "(nothing logged)"


def new_app_pids(ctx, known):
    return [p["pid"] for p in ctx.probe.named("yuzora.exe") if p["pid"] not in known]


def folder_version(ctx, folder):
    exe = os.path.join(folder, "yuzora.exe")
    if not os.path.exists(exe):
        return None
    return version_word(ctx.run.sh([exe, "--version"], 30).out)


def snap(ctx, cid, label):
    names = sorted(n for n in os.listdir(PROGRAMS) if n.startswith("Yuzora")) if os.path.isdir(PROGRAMS) else []
    state = {
        "siblings": {n: ("dir" if os.path.isdir(os.path.join(PROGRAMS, n)) else "file") for n in names},
        "install_version": folder_version(ctx, INSTALL),
        "old_version": folder_version(ctx, beside(".old")),
        "new_version": folder_version(ctx, beside(".new")),
        "receipt": read_text(beside(".new.json")) or None,
        "install_files": [e["path"] for e in inventory(INSTALL)["files"]],
        "old_files": [e["path"] for e in inventory(beside(".old"))["files"]],
        "new_files": [e["path"] for e in inventory(beside(".new"))["files"]],
        "package_list": package_list_state() if os.path.isdir(INSTALL) else None,
    }
    ctx.run.evidence("%s/snapshot-%s.json" % (cid, label), json.dumps(state, indent=2))
    return state


def prestage(ctx, cid, version):
    """What `stage` leaves beside the install: the archive unpacked by System32 tar.exe, renamed to Yuzora.new, then the receipt."""
    shutil.rmtree(beside(".new"), ignore_errors=True)
    try:
        os.remove(beside(".new.json"))
    except OSError:
        pass
    # Beside the install, like `stage`: a rename from another drive would not work.
    unpacked = beside(".download")
    shutil.rmtree(unpacked, ignore_errors=True)
    os.makedirs(unpacked)
    tar = os.path.join(os.environ.get("SystemRoot", "C:\\Windows"), "System32", "tar.exe")
    ctx.run.sh([tar, "-xzf", ctx.archives[version], "-C", unpacked], 180, name="%s/prestage-tar-%s.txt" % (cid, version))
    os.rename(os.path.join(unpacked, "yuzora"), beside(".new"))
    shutil.rmtree(unpacked, ignore_errors=True)
    with open(beside(".new.json"), "w") as f:
        f.write(json.dumps({"version": version}))


def staged(version):
    try:
        receipt = json.loads(read_text(beside(".new.json")))
    except ValueError:
        return False
    return receipt.get("version") == version and os.path.exists(os.path.join(beside(".new"), "yuzora.exe"))


def cwd_class(info):
    cwd = (info.get("peb") or {}).get("cwd_final")
    if not cwd:
        return "unknown", None
    c = norm(cwd)
    if c == norm(beside(".old")) or c.startswith(norm(beside(".old")) + "\\"):
        return "inside-old", cwd
    if c == norm(INSTALL) or c.startswith(norm(INSTALL) + "\\"):
        return "inside-install", cwd
    return "other", cwd


BLOCKED = ("cannot move", "cannot put the new version", "cannot restore the previous version")

SHOT = ("Add-Type -AssemblyName System.Windows.Forms,System.Drawing; "
        "$b=[System.Windows.Forms.SystemInformation]::VirtualScreen; "
        "$bmp=New-Object System.Drawing.Bitmap $b.Width,$b.Height; "
        "$g=[System.Drawing.Graphics]::FromImage($bmp); "
        "$g.CopyFromScreen($b.Location,[System.Drawing.Point]::Empty,$b.Size); "
        "$bmp.Save('%s',[System.Drawing.Imaging.ImageFormat]::Png); $g.Dispose(); $bmp.Dispose()")


def screenshot(ctx, rel):
    """The runner's desktop as a PNG, to be looked at by a person (a message inside the app's window has no
    other way to be read). Returns the relative name, or None when the capture failed."""
    if ctx.shots >= 8:
        return None
    ctx.shots += 1
    path = os.path.join(ctx.run.out, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    shell = ctx.run.sh(["powershell", "-NoProfile", "-NonInteractive", "-Command", SHOT % path.replace("'", "''")], 60, name=rel + ".txt")
    ok = shell.rc == 0 and os.path.exists(path) and os.path.getsize(path) > 0
    ctx.run.log("screenshot %s: %s" % (rel, "saved" if ok else "failed (rc=%s)" % shell.rc))
    return rel if ok else None


def blocked_lines(text):
    return [line.strip() for line in text.splitlines() if any(m in line for m in BLOCKED)]


def watch_exit(ctx, old, timeout, since=0):
    """Waits for `old` to end. A swap that fails says so in the app's log and the app then goes on running,
    so that line ends the wait early (a few seconds later, for whatever the app does about it).
    Returns (exited, failure line or None)."""
    seen = {"line": None}

    def done():
        ctx.probe.ps()  # also remembers who is running, for a parent that is gone when it is asked about
        if not old.alive():
            return True
        lines = blocked_lines(old.log_text()[since:])
        if lines:
            seen["line"] = lines[0]
            return True
        return False
    wait_for(done, timeout, 1)
    return (not old.alive()), seen["line"]


def failure_signals(ctx, cid, k, old, since):
    """What a person could have noticed after a swap that failed while the app kept running: the app's own
    output since the trigger, its windows, and a picture of the desktop a few seconds later."""
    time.sleep(4)
    if not old.alive():
        return "the app ended on its own, so there was no window left to show a message in"
    shot = screenshot(ctx, "%s/failure-%d.png" % (cid, k))
    windows = [{"title": w.get("title"), "class": w.get("class")} for w in ctx.probe.windows(old.pid) if w.get("visible")]
    new_lines = [line for line in old.log_text()[since:].splitlines() if line.strip()]
    return "still running; visible windows %s; screenshot %s; its output since the trigger (%d lines): %s" % (
        json.dumps(windows), shot or "not taken", len(new_lines), " | ".join(new_lines[-8:]) or "(none)")


def observe_swap(ctx, cid, k, to_version, old, home, expect_relaunch, known, reader_ok, since=0, exit_timeout=30):
    """After `old` was told to apply an update: did the folder change, and did a new process appear?
    `since` is how much of the app's log existed before the trigger (earlier failures are not this one's).
    Returns (swapped, reopened): whether the folder now runs `to_version`, and the Instance the app
    reopened itself as (None when it did not, or when `expect_relaunch` is off)."""
    run, probe = ctx.run, ctx.probe
    mark = len(ctx.server_entries())
    known.add(old.pid)
    exited, failure = watch_exit(ctx, old, exit_timeout, since)
    signals = None
    if failure:
        signals = failure_signals(ctx, cid, k, old, since)
        exited = old.wait_exit(8)
    time.sleep(1)
    post = snap(ctx, cid, "after-swap%d" % k)
    marker = os.path.exists(os.path.join(home, ".updated"))
    run.evidence("%s/home-after-swap%d.json" % (cid, k), json.dumps(sorted(os.listdir(home)) if os.path.isdir(home) else None))
    evidence = ["%s/snapshot-after-swap%d.json" % (cid, k)] + ([old.log_rel] if old.log_rel else [])
    if post["install_version"] != to_version:
        tries = len(blocked_lines(old.log_text()[since:])) if old.log_path else None
        run.result("%s.swap_%d" % (cid, k), FAIL, "the folder still runs %s, not %s. old process exited=%s. failure line: %s. times the failure was logged: %s. its update lines: %s" % (
            post["install_version"], to_version, exited, failure or "none seen", tries, update_lines(old.log_text()) if old.log_path else "(no log kept for this process)"), evidence)
        if signals:
            shot = "%s/failure-%d.png" % (cid, k)
            run.result("%s.failure_visible_%d" % (cid, k), INFO, "after the failed swap: %s; Yuzora.new kept=%s, receipt kept=%s" % (signals, bool(post["new_version"]), bool(post["receipt"])), evidence + ([shot] if os.path.exists(os.path.join(run.out, shot)) else []))
        return False, None
    run.result("%s.swap_%d" % (cid, k), PASS, "the folder now runs %s; old process exited=%s; Yuzora.old runs %s; .updated marker=%s; siblings=%s" % (post["install_version"], exited, post["old_version"], marker, sorted(post["siblings"])), evidence)
    if not expect_relaunch:
        return True, None
    fresh = wait_for(lambda: new_app_pids(ctx, known), 60)
    if not fresh:
        run.result("%s.relaunch_%d" % (cid, k), FAIL, "the swap happened but no new yuzora.exe appeared within 60 s (the PowerShell Start-Process line)", evidence)
        return True, None
    pid = fresh[0]
    known.add(pid)
    info = probe.procinfo(pid, "%s/procinfo-relaunch%d.txt" % (cid, k))
    peb = info.get("peb") or {}
    ev = ["%s/procinfo-relaunch%d.txt" % (cid, k)]
    image = info.get("image_query")
    ok_image = image is not None and norm(image) == norm(EXE)
    parent = info.get("parent_name")
    if parent is None and info.get("ppid") is not None:
        parent = "%s (already gone when asked, name from an earlier process list)" % (probe.name_of(info.get("ppid")) or "unknown")
    run.result("%s.relaunch_%d" % (cid, k), PASS if ok_image else FAIL, "pid %s runs %s, parent %s (pid %s), command line %s" % (pid, image, parent, info.get("ppid"), peb.get("cmdline")), ev)
    kind, cwd = cwd_class(info)
    if not reader_ok or kind == "unknown":
        run.result("%s.relaunch_%d_cwd" % (cid, k), NOT_MEASURED, "cwd not read (reader self-check ok=%s)" % reader_ok, ev)
    elif kind == "inside-old":
        run.result("%s.relaunch_%d_cwd" % (cid, k), FAIL, "the reopened app's cwd handle is %s, inside the old folder: nothing can delete Yuzora.old while it runs" % cwd, ev)
    else:
        run.result("%s.relaunch_%d_cwd" % (cid, k), PASS, "the reopened app's cwd handle is %s (%s); PEB string %s" % (cwd, kind, peb.get("cwd_dos")), ev)
    trust = info.get("redirection_trust") or {}
    if trust.get("measured"):
        run.result("%s.relaunch_%d_redirection_trust" % (cid, k), INFO, "EnforceRedirectionTrust=%s AuditRedirectionTrust=%s (flags %s); parent was %s" % (trust.get("enforce"), trust.get("audit"), trust.get("flags"), parent), ev)
    else:
        run.result("%s.relaunch_%d_redirection_trust" % (cid, k), NOT_MEASURED, "GetProcessMitigationPolicy(RedirectionTrust) failed with Win32 error %s on this Windows" % trust.get("win32_error"), ev)
    asked = wait_for(lambda: ctx.ua_seen(to_version, mark), 40)
    lives = probe.alive(pid)
    if asked:
        verdict, note = PASS, "asked the stand-in server as Yuzora/%s" % to_version
    elif lives:
        verdict, note = FAIL, "was still running after 40 s but never asked the stand-in server (its first check is at 10 s)"
    else:
        verdict, note = NOT_MEASURED, "ended before its first check at 10 s; if the same build survives when started by hand, suspect the in-place version patch or the window, not the updater"
    run.result("%s.relaunch_%d_alive" % (cid, k), verdict, "the reopened app (pid %s) %s" % (pid, note), ["server.log"])
    return True, Instance(ctx, pid, "relaunched-%d" % k)


def staged_version():
    try:
        return json.loads(read_text(beside(".new.json"))).get("version")
    except ValueError:
        return None


def not_reached(ctx, cid, k, why):
    """The updates after the k-th were never tried, because the k-th did not get that far."""
    for later in range(k + 1, 3):
        ctx.run.result("%s.update_%d_not_reached" % (cid, later), NOT_MEASURED, "update %d was never tried: %s" % (later, why), [])


def check_app_cwd(ctx, cid, inst, started_inside, reader_ok, tag="app_cwd"):
    """Where the running app's own cwd is, read from a duplicated cwd handle (the path string goes stale
    after a rename). A fix that moves it at startup shows here."""
    info = ctx.probe.procinfo(inst.pid, "%s/procinfo-%s-%d.txt" % (cid, tag, inst.pid))
    kind, cwd = cwd_class(info)
    ev = ["%s/procinfo-%s-%d.txt" % (cid, tag, inst.pid)]
    rid = "%s.%s" % (cid, tag)
    if not reader_ok or kind == "unknown":
        ctx.run.result(rid, NOT_MEASURED, "the app's cwd could not be read (reader self-check ok=%s, process answered %s)" % (reader_ok, json.dumps(info)[:200]), ev)
    elif not started_inside:
        ctx.run.result(rid, INFO, "started elsewhere; its cwd handle is %s (%s)" % (cwd, kind), ev)
    elif kind in ("inside-install", "inside-old"):
        ctx.run.result(rid, FAIL, "started with the install folder as cwd, as the shortcut does; its cwd handle is still %s" % cwd, ev)
    else:
        ctx.run.result(rid, PASS, "started with the install folder as cwd, as the shortcut does; its cwd handle is now %s (%s)" % (cwd, kind), ev)


CORE_NAMES = ("hc.exe", "herdr-core.exe")


def list_holders(ctx, cid, app_pid, tag):
    """Every program the app started or that serves it, with cwd and command line read now. A program that
    ends between the process list and the read is marked as gone (a short-lived helper), not as unknown."""
    rows = ctx.probe.ps()
    kids = ctx.probe.descendants(app_pid, rows) if app_pid else set()
    found = []
    for p in rows:
        if p["name"].lower() in CORE_NAMES or p["pid"] in kids:
            ctx.tracker.add(p["pid"])
            info = ctx.probe.procinfo(p["pid"], "%s/procinfo-%s-%d.txt" % (cid, tag, p["pid"]))
            kind, cwd = cwd_class(info)
            found.append({"name": p["name"], "pid": p["pid"], "ppid": p.get("ppid"), "kind": kind, "cwd": cwd, "gone": info.get("alive") is False, "cmd": (info.get("peb") or {}).get("cmdline")})
    return found


def holder_text(h):
    if h["gone"]:
        return "%s pid %s ended before it could be read (a short-lived helper, not a holder)" % (h["name"], h["pid"])
    return "%s pid %s parent %s `%s` cwd %s (%s)" % (h["name"], h["pid"], h["ppid"], h["cmd"], h["cwd"], h["kind"])


def report_holders(ctx, cid, rows, reader_ok, tag, verdict_id=None):
    """The INFO line with every holder, and (verdict_id) PASS/FAIL on whether any herdr-core process has the
    install folder as its cwd."""
    ev = ["%s/procinfo-%s-%d.txt" % (cid, tag, h["pid"]) for h in rows]
    inside = [h for h in rows if h["kind"] in ("inside-install", "inside-old")]
    ctx.run.result("%s.holders_%s" % (cid, tag), INFO if reader_ok else NOT_MEASURED, "%d program(s) the app started or that serve it; %d with a cwd in the install folder: %s" % (len(rows), len(inside), "; ".join(holder_text(h) for h in rows)), ev)
    if verdict_id:
        core = [h for h in rows if h["name"].lower() in CORE_NAMES and not h["gone"] and h["kind"] != "unknown"]
        core_inside = [h for h in core if h in inside]
        if not reader_ok or not core:
            ctx.run.result(verdict_id, NOT_MEASURED, "no herdr-core process could be read (reader self-check ok=%s)" % reader_ok, ev)
        elif core_inside:
            ctx.run.result(verdict_id, FAIL, "%d of %d herdr-core process(es) have the install folder as cwd and will stop a swap: %s" % (len(core_inside), len(core), "; ".join(holder_text(h) for h in core_inside)), ev)
        else:
            ctx.run.result(verdict_id, PASS, "all %d herdr-core process(es) read have a cwd outside the install folder: %s" % (len(core), "; ".join(holder_text(h) for h in core)), ev)



# ---- chains -----------------------------------------------------------------

def new_home(ctx, cid):
    home = os.path.join(ctx.work, "home-" + cid)
    shutil.rmtree(home, ignore_errors=True)
    os.makedirs(home)
    return home


def chain_release(ctx, mode, reader_ok):
    """R1: the shipped build, update files staged by hand, startup swap and relaunch, twice."""
    cid = "R1-" + mode
    home = new_home(ctx, cid)
    cwd = INSTALL if mode == "install" else ctx.elsewhere
    install_fresh(ctx, cid)
    if mode == "install":
        shortcut_check(ctx)
    known = set()
    for k, version in [(1, ctx.v2), (2, ctx.v3)]:
        prestage(ctx, cid, version)
        snap(ctx, cid, "before-swap%d" % k)
        app = start_app(ctx, cid, "app%d" % k, home, cwd)
        swapped, reopened = observe_swap(ctx, cid, k, version, app, home, True, known, reader_ok)
        if not swapped:
            if app.alive():
                check_app_cwd(ctx, cid, app, cwd == INSTALL, reader_ok)
            not_reached(ctx, cid, k, "update %d was not applied" % k)
            break
        if reopened:
            reopened.stop()
        ctx.tracker.kill_all()
        known |= {p["pid"] for p in ctx.probe.named("yuzora.exe")}


def chain_release_extra(ctx):
    """R2/R3: a clean install keeps itself clean while running; one stray file stops updates without a word."""
    cid = "R2"
    home = new_home(ctx, cid)
    install_fresh(ctx, cid)
    mark = len(ctx.server_entries())
    app = start_app(ctx, cid, "clean-run", home, INSTALL)
    wait_for(lambda: ctx.ua_seen(ctx.v1, mark), 40)
    state = package_list_state()
    asked = ctx.ua_seen(ctx.v1, mark)
    alive = app.alive()
    app.stop()
    clean = not state["extra"] and not state["missing"]
    ctx.run.result("R3.app_leaves_install_folder_alone", PASS if clean and asked and alive else (FAIL if (not clean and alive) else NOT_MEASURED),
                   "after the app ran: alive=%s, asked the server for updates=%s (so is_install held), folder vs yuzora.package extra=%s missing=%s" % (alive, asked, state["extra"], state["missing"]), ["R2/clean-run.log", "server.log"])
    ctx.tracker.kill_all()

    with open(os.path.join(INSTALL, "notes.txt"), "w") as f:
        f.write("a person's file")
    prestage(ctx, cid, ctx.v2)
    mark = len(ctx.server_entries())
    app = start_app(ctx, cid, "stray-file-run", home, INSTALL)
    time.sleep(35)
    still = app.alive()
    asks = [e for e in ctx.server_entries()[mark:] if "/releases" in e.get("path", "")]
    snapshot = snap(ctx, cid, "stray-file")
    silent = still and not asks and snapshot["install_version"] == ctx.v1 and staged(ctx.v2)
    ctx.run.result("R2.stray_file_disables_updates", INFO,
                   "with notes.txt in the folder and Yuzora.new staged: app alive=%s, release-list requests=%d, folder still %s, stage left untouched=%s => updates %s" % (still, len(asks), snapshot["install_version"], staged(ctx.v2), "were silently off" if silent else "did something else, read the snapshot"),
                   ["R2/stray-file-run.log", "R2/snapshot-stray-file.json", "server.log"])
    app.stop()
    ctx.tracker.kill_all()


def wait_staged_and_window(ctx, inst, version, timeout):
    """Waits for `version` to be staged by the app's own check. Also notes whether a window showed, and
    stops early when the app staged some other version (the stand-in server offered something else:
    the probe's fault, said as such)."""
    seen = {"window": None}

    def done():
        if seen["window"] is None:
            visible = [w for w in ctx.probe.windows(inst.pid) if w.get("visible")]
            if visible:
                seen["window"] = visible
        other = staged_version()
        return staged(version) or other not in (None, version) or not inst.alive()
    wait_for(done, timeout, 2)
    other = staged_version()
    return staged(version), seen["window"], inst.alive(), (other if other not in (None, version) else None)


def chain_testkey(ctx, mode, reader_ok, startup_first=False):
    """D1: the whole path against the stand-in server, twice; applied when the window closes.
    D2 (startup_first): the first update is applied by ending the app and starting it again, so the
    program that does the second update is the one PowerShell reopened (its cwd is whatever
    Start-Process gave it) and needs no herdr-core."""
    cid = ("D2-" if startup_first else "D1-") + mode
    home = new_home(ctx, cid)
    cwd = INSTALL if mode == "install" else ctx.elsewhere
    install_fresh(ctx, cid)
    ctx.fresh_server(ctx.v2)
    known = set()
    app = start_app(ctx, cid, "app1", home, cwd)
    for k, from_v, to_v in [(1, ctx.v1, ctx.v2), (2, ctx.v2, ctx.v3)]:
        mark = len(ctx.server_entries())
        ok, window, alive, other = wait_staged_and_window(ctx, app, to_v, 100)
        if k == 1:
            ctx.run.result("%s.window_opened" % cid, INFO if window else NOT_MEASURED, "visible top-level windows of the app: %s" % (json.dumps(window) if window else "none seen"), [])
        requests = [e.get("path", "").split("?")[0] for e in ctx.server_entries()[mark:]]
        ev = ["server.log"] + ([app.log_rel] if app.log_rel else [])
        if other:
            ctx.run.result("%s.stage_%d" % (cid, k), PROBE_ERROR, "the app staged %s while the probe expected %s: the stand-in server offered %s (a probe fault, nothing about the product)" % (other, to_v, [v for v, _ in ctx.server.releases]), ev)
            not_reached(ctx, cid, k, "the probe's server state was wrong")
            return
        if not ok:
            checked = ctx.ua_seen(from_v, mark)
            detail = "%s was not staged: app alive=%s (exit code %s), asked the server=%s, requests seen=%s, update lines: %s" % (to_v, alive, app.proc.poll() if app.proc else "n/a", checked, requests, update_lines(app.log_text()) if app.log_path else "(no log kept for this process)")
            if alive or checked:
                ctx.run.result("%s.stage_%d" % (cid, k), FAIL, detail, ev)
            else:
                ctx.run.result("%s.stage_%d" % (cid, k), NOT_MEASURED, "the app ended before its first check at 10 s, so the update path never ran (a window or startup problem, not an update result): " + detail, ev)
            not_reached(ctx, cid, k, "update %d was not staged" % k)
            return
        ctx.run.result("%s.stage_%d" % (cid, k), PASS, "check, download, signature, checksum, unpack and stage all ran in the app: requests %s; Yuzora.new + receipt for %s" % (requests, to_v), ev)
        if k == 1:
            check_app_cwd(ctx, cid, app, cwd == INSTALL, reader_ok)
        ctx.offer(ctx.v3)
        snap(ctx, cid, "before-swap%d" % k)
        forced = startup_first and k == 1
        since = len(app.log_text())
        closed = {"windows": "not asked", "posted": 0} if forced else ctx.probe.close(app.pid)
        quit_path = bool(closed.get("posted"))
        if forced:
            note = "ended the app and started it again on purpose: the staged files are applied at startup and PowerShell reopens the app"
        elif quit_path:
            note = "closed %s window(s) with WM_CLOSE, as taskkill does: the app quits and applies the update" % closed.get("posted")
        else:
            note = "no window to close (%s windows found): the app was ended and started again, so the staged files are applied at startup" % closed.get("windows")
        ctx.run.result("%s.trigger_%d" % (cid, k), INFO, note, [])
        if quit_path:
            swapped, _ = observe_swap(ctx, cid, k, to_v, app, home, False, known, reader_ok, since=since)
            if not swapped:
                not_reached(ctx, cid, k, "update %d was not applied" % k)
                return
            app = start_app(ctx, cid, "app%d" % (k + 1), home, cwd)
            up = wait_for(lambda: ctx.ua_seen(to_v, mark), 60)
            if up and app.alive():
                verdict, text = PASS, "alive and asked the server as Yuzora/%s" % to_v
            elif app.alive():
                verdict, text = FAIL, "alive but never asked the server as Yuzora/%s" % to_v
            else:
                verdict, text = NOT_MEASURED, "ended before its first check (exit code %s); if the build it replaced started fine, suspect the in-place version patch" % app.proc.poll()
            ctx.run.result("%s.start_after_swap_%d" % (cid, k), verdict, "started from the Start-in folder after the swap: " + text, ["server.log", app.log_rel])
        else:
            ctx.tracker.kill_all()
            app.wait_exit(10)
            restarted = start_app(ctx, cid, "app%d-restart" % k, home, cwd)
            swapped, reopened = observe_swap(ctx, cid, k, to_v, restarted, home, True, known, reader_ok)
            if not swapped:
                not_reached(ctx, cid, k, "update %d was not applied" % k)
                return
            # A reopen that did not happen is itself a result; a person would start the app again.
            app = reopened or start_app(ctx, cid, "app%d-by-hand" % (k + 1), home, cwd)


# ---- chains with herdr-core ---------------------------------------------------

HELLO = b'{"jsonrpc":"2.0","id":0,"method":"hello","params":{"framed":true}}\n'


def ensure_core(ctx):
    """herdr-core from its own installer, once per job. Returns its path, or None (and says why, once)."""
    if ctx.core or ctx.core_tried:
        return ctx.core
    ctx.core_tried = True
    shell = ctx.run.sh(["powershell", "-NoProfile", "-NonInteractive", "-Command", CORE_INSTALLER], 300, name="core/install.txt")
    core = next((p for p in (os.path.join(PROGRAMS, "herdr-core", n) for n in ("herdr-core.exe", "hc.exe")) if os.path.exists(p)), None)
    version = ctx.run.sh([core, "--version"], 30, name="core/version.txt").out.strip() if core else ""
    if version.startswith("hc "):
        ctx.core = core
        ctx.run.result("core.install", PASS, "%s at %s" % (version, core), ["core/install.txt"])
    else:
        ctx.run.result("core.install", NOT_MEASURED, "herdr-core did not install from releases.herdr.dev (rc=%s, found=%s %r): no main window, no Restart button, no core cwd holders" % (shell.rc, core, version), ["core/install.txt"])
    return ctx.core


def ensure_cli(ctx):
    """The control client, a copy of the exe outside the install folder: a client inside it would be one more
    program the swap has to move past."""
    if not ctx.cli:
        os.makedirs(os.path.join(ctx.work, "cli"), exist_ok=True)
        ctx.cli = os.path.join(ctx.work, "cli", "yuzora.exe")
        shutil.copy(EXE, ctx.cli)


def run_cli(ctx, cid, home, *words, timeout=60):
    return ctx.run.sh([ctx.cli] + list(words), timeout, env=app_env(ctx, home), cwd=ctx.elsewhere, name="%s/cli-%s-%d.txt" % (cid, "-".join(words).replace(".", "_"), int(time.time())))


def main_window_up(ctx, home):
    shell = ctx.run.sh([ctx.cli, "status"], 20, env=app_env(ctx, home), cwd=ctx.elsewhere)
    return shell.rc == 0 and isinstance(shell.json(), dict)


def wait_main_window(ctx, cid, home, app, k, timeout=90):
    up = wait_for(lambda: main_window_up(ctx, home) or not app.alive(), timeout, 3)
    if not up or not app.alive():
        ctx.run.result("%s.main_window_%d" % (cid, k), NOT_MEASURED, "no main window answered `yuzora status` (app alive=%s): the setup screen does not serve commands. log: %s" % (app.alive(), tail(app.log_text(), 6).replace("\n", " | ")), [app.log_rel] if app.log_rel else [])
        return False
    ctx.run.result("%s.main_window_%d" % (cid, k), PASS, "`yuzora status` answered: the main window is up on herdr-core", [])
    return True


def stage_via_cli(ctx, cid, k, to_v, app, home):
    """`yuzora do app.check_updates`, then wait for the app to stage `to_v` on its own."""
    run_cli(ctx, cid, home, "do", "app.check_updates")
    got = wait_for(lambda: staged(to_v) or staged_version() not in (None, to_v), 60, 2)
    other = staged_version()
    if got and staged(to_v):
        ctx.run.result("%s.stage_%d" % (cid, k), PASS, "staged %s by the app's own check" % to_v, ["server.log"])
        return True
    if got and other not in (None, to_v):
        ctx.run.result("%s.stage_%d" % (cid, k), PROBE_ERROR, "the app staged %s while the probe expected %s: the stand-in server offered %s (a probe fault, nothing about the product)" % (other, to_v, [v for v, _ in ctx.server.releases]), ["server.log"])
        return False
    ctx.run.result("%s.stage_%d" % (cid, k), FAIL, "%s not staged within 60 s of `app.check_updates`; log: %s" % (to_v, tail(app.log_text(), 6).replace("\n", " | ")), ["server.log"] + ([app.log_rel] if app.log_rel else []))
    return False


def start_old_core(ctx, cid, home):
    """The state an app of the build before the fix leaves behind: `herdr-core connect` started from the folder
    the app was started in (the app spawns it with the cwd it has), and the `serve` that connect starts, which
    stays when the client goes. Done with the same call hc.rs makes, so it needs no old build in this job.
    Returns {"serve": [pids]} when a serve with its cwd in the install folder survived the client, else None."""
    run, probe = ctx.run, ctx.probe
    before = {p["pid"] for p in probe.named("herdr-core.exe")}
    proc = subprocess.Popen([ctx.core, "connect"], cwd=INSTALL, env=app_env(ctx, home), stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    ctx.tracker.add(proc.pid)
    got = []

    def read_reply():
        try:
            got.append(proc.stdout.readline())
        except (OSError, ValueError) as e:
            got.append(("read failed: %s" % e).encode())
    reader = threading.Thread(target=read_reply, daemon=True)
    reader.start()
    notes = []
    try:
        proc.stdin.write(HELLO)
        proc.stdin.flush()
    except OSError as e:
        notes.append("could not write hello: %s" % e)
    reader.join(30)
    reply = (got[0] if got else b"").decode("utf-8", "replace").strip()
    time.sleep(1)
    infos = []
    for p in probe.named("herdr-core.exe"):
        if p["pid"] in before or p["pid"] == proc.pid:
            continue
        ctx.tracker.add(p["pid"])
        infos.append((p["pid"], probe.procinfo(p["pid"], "%s/procinfo-oldcore-%d.txt" % (cid, p["pid"]))))
    serve = [pid for pid, info in infos if re.search(r"\bserve\b", (info.get("peb") or {}).get("cmdline") or "")]
    proc.kill()  # the client goes, as it does when the app that started it ends
    try:
        _, err = proc.communicate(timeout=10)
    except Exception as e:
        err = ("communicate: %s" % e).encode()
    time.sleep(3)
    survivors = [pid for pid in serve if probe.alive(pid)]
    inside = []
    for pid in survivors:
        kind, cwd = cwd_class(probe.procinfo(pid, "%s/procinfo-oldcore-after-%d.txt" % (cid, pid)))
        if kind in ("inside-install", "inside-old"):
            inside.append(pid)
    run.evidence("%s/old-core.txt" % cid, "hello reply: %s\nnew herdr-core processes: %s\nserve: %s\nstill alive after the client was ended: %s\ncwd inside install: %s\nclient stderr: %s\nnotes: %s\n" % (reply[:600], [(pid, (i.get("peb") or {}).get("cmdline")) for pid, i in infos], serve, survivors, inside, err.decode("utf-8", "replace")[:600], notes))
    if inside:
        run.result("%s.old_core_state" % cid, PASS, "`herdr-core connect` started with cwd = the install folder (as an app of the build before the fix does) left a `serve` (pid %s) that outlives the client and has the install folder as its cwd" % inside, ["%s/old-core.txt" % cid])
        return {"serve": survivors}
    run.result("%s.old_core_state" % cid, PROBE_ERROR, "could not build the state of an old app's core: hello reply %r, new herdr-core processes %s, serve %s, alive after the client ended %s, cwd inside %s" % (reply[:200], [pid for pid, _ in infos], serve, survivors, inside), ["%s/old-core.txt" % cid])
    return None


def chain_core(ctx, reader_ok):
    """C1: herdr-core installed, the main window up, the update applied by `yuzora do app.update`."""
    cid = "C1"
    home = new_home(ctx, cid)
    if not ensure_core(ctx):
        ctx.run.result("C1.skipped", NOT_MEASURED, "herdr-core is not installed (see core.install)", [])
        return
    install_fresh(ctx, cid)
    ensure_cli(ctx)
    ctx.fresh_server(ctx.v2)
    known = set()
    app = start_app(ctx, cid, "app1", home, INSTALL)
    for k, to_v in [(1, ctx.v2), (2, ctx.v3)]:
        if not wait_main_window(ctx, cid, home, app, k):
            return
        if k == 1:
            check_app_cwd(ctx, cid, app, True, reader_ok)
            report_holders(ctx, cid, list_holders(ctx, cid, app.pid, "core"), reader_ok, "core", "C1.core_cwd_outside_install")
        if not stage_via_cli(ctx, cid, k, to_v, app, home):
            return
        ctx.offer(ctx.v3)
        snap(ctx, cid, "before-swap%d" % k)
        since = len(app.log_text())
        reply = run_cli(ctx, cid, home, "do", "app.update")
        ctx.run.result("%s.restart_command_%d" % (cid, k), INFO, "`yuzora do app.update` rc=%s: %s" % (reply.rc, (reply.out or reply.err).strip().replace("\n", " ")), [])
        swapped, reopened = observe_swap(ctx, cid, k, to_v, app, home, True, known, reader_ok, since=since)
        if not swapped:
            not_reached(ctx, cid, k, "update %d was not applied" % k)
            return
        app = reopened or start_app(ctx, cid, "app%d-by-hand" % (k + 1), home, INSTALL)
    time.sleep(30)
    after = snap(ctx, cid, "after-core-update")
    ctx.run.result("C1.old_folder_after_core_update", INFO, "30 s after the second update: Yuzora.old %s (%s); marker .updated %s" % ("is still there" if "Yuzora.old" in after["siblings"] else "is gone", after["old_files"][:6], "still there" if os.path.exists(os.path.join(home, ".updated")) else "consumed"), ["C1/snapshot-after-core-update.json"])


def chain_old_core(ctx, reader_ok):
    """C2: a herdr-core from before the fix is still alive with the install folder as its cwd (what a person
    upgrading by hand from the beta has), then the new app updates itself with Restart. If the swap fails,
    the person is told nothing the probe can read, so the follow-up is what they would try: end the old core
    and close the app, which applies the staged update on quit."""
    cid = "C2"
    home = new_home(ctx, cid)
    if not ensure_core(ctx):
        ctx.run.result("C2.skipped", NOT_MEASURED, "herdr-core is not installed (see core.install)", [])
        return
    install_fresh(ctx, cid)
    ensure_cli(ctx)
    ctx.fresh_server(ctx.v2)
    old = start_old_core(ctx, cid, home)
    if not old:
        return
    app = start_app(ctx, cid, "app1", home, INSTALL)
    if not wait_main_window(ctx, cid, home, app, 1):
        return
    check_app_cwd(ctx, cid, app, True, reader_ok)
    report_holders(ctx, cid, list_holders(ctx, cid, app.pid, "with-old-core"), reader_ok, "with-old-core")
    if not stage_via_cli(ctx, cid, 1, ctx.v2, app, home):
        return
    snap(ctx, cid, "before-swap1")
    since = len(app.log_text())
    reply = run_cli(ctx, cid, home, "do", "app.update")
    ctx.run.result("C2.restart_command_1", INFO, "`yuzora do app.update` rc=%s: %s" % (reply.rc, (reply.out or reply.err).strip().replace("\n", " ")), [])
    known = set()
    swapped, reopened = observe_swap(ctx, cid, 1, ctx.v2, app, home, True, known, reader_ok, since=since)
    alive_after = [pid for pid in old["serve"] if ctx.probe.alive(pid)]
    ctx.run.result("C2.old_core_after_attempt", INFO, "the old serve process(es) %s: still alive %s (the app %s stop it)" % (old["serve"], alive_after, "did not" if alive_after else "did"), [])
    if swapped:
        return
    if not app.alive():
        ctx.run.result("C2-retry.swap_1", NOT_MEASURED, "the app ended by itself after the failed swap, so there was no staged update left to apply by closing it", [])
        return
    ctx.tracker.kill_some(old["serve"])
    time.sleep(2)
    since = len(app.log_text())
    closed = ctx.probe.close(app.pid)
    ctx.run.result("C2-retry.trigger_1", INFO, "ended the old core, then closed %s window(s) with WM_CLOSE: the app quits and applies the staged update" % closed.get("posted"), [])
    observe_swap(ctx, "C2-retry", 1, ctx.v2, app, home, False, known, reader_ok, since=since)


def chain_overlay(ctx, cid, reader_ok, old_core, app_running):
    """C3-C5: the installer run again (/S) over a working install with herdr-core alive, as a person does who
    installs the next build by hand. `old_core`: the core is the one a build before the fix leaves (cwd in the
    install folder). `app_running`: the app is still open (otherwise it was closed and only the core lives)."""
    home = new_home(ctx, cid)
    if not ensure_core(ctx):
        ctx.run.result("%s.skipped" % cid, NOT_MEASURED, "herdr-core is not installed (see core.install)", [])
        return
    install_fresh(ctx, cid)
    ensure_cli(ctx)
    ctx.fresh_server()  # nothing to offer: the app must not stage anything while the installer runs
    if old_core and not start_old_core(ctx, cid, home):
        return
    app = start_app(ctx, cid, "app1", home, INSTALL)
    if not wait_main_window(ctx, cid, home, app, 1):
        return
    if not app_running:
        app.stop()
        time.sleep(2)
    rows = list_holders(ctx, cid, app.pid if app_running else None, "before-overlay")
    report_holders(ctx, cid, rows, reader_ok, "before-overlay")
    before_core = [h["pid"] for h in rows if h["name"].lower() in CORE_NAMES and not h["gone"]]
    app_was = app.alive()
    snap(ctx, cid, "before-overlay")
    shell = ctx.run.sh([ctx.installer, "/S"], 240, name="%s/overlay-install.txt" % cid)
    time.sleep(3)
    state = package_list_state()
    after = snap(ctx, cid, "after-overlay")
    running = ctx.run.sh([EXE, "--version"], 30, name="%s/overlay-version.txt" % cid)
    ok = shell.rc == 0 and version_word(running.out) == ctx.v1 and not state["extra"] and not state["missing"]
    ctx.run.result("%s.overlay_install" % cid, PASS if ok else FAIL, "installer %s rc=%s (state: app %s, old-style core %s); installed exe says %r; folder vs yuzora.package: extra=%s missing=%s; siblings %s; Yuzora.old files %s" % (
        "TIMED OUT" if shell.timed_out else "ended", shell.rc, "running" if app_running else "closed", old_core, running.out.strip(), state["extra"], state["missing"], sorted(after["siblings"]), after["old_files"][:6]), ["%s/overlay-install.txt" % cid, "%s/snapshot-after-overlay.json" % cid])
    survivors = [pid for pid in before_core if ctx.probe.alive(pid)]
    ctx.run.result("%s.core_after_overlay" % cid, INFO, "herdr-core processes before %s, alive after %s: the installer %s the core. app was running before=%s, running after=%s" % (before_core, survivors, "left" if survivors == before_core else "stopped some of", app_was, app.alive()), [])
    if not app.alive():
        again = start_app(ctx, cid, "app2-after-overlay", home, INSTALL)
        up = wait_for(lambda: main_window_up(ctx, home) or not again.alive(), 60, 3)
        ctx.run.result("%s.start_after_overlay" % cid, PASS if up and again.alive() else FAIL, "the app started from the overlaid install: main window answered=%s, alive=%s; log: %s" % (bool(up and again.alive()), again.alive(), tail(again.log_text(), 4).replace("\n", " | ")), [again.log_rel] if again.log_rel else [])



# ---- setup ----------------------------------------------------------------

def prepare(ctx):
    run = ctx.run
    ctx.installer = os.path.join(ctx.dist, "Yuzora-windows-x64-setup.exe")
    found = glob.glob(os.path.join(ctx.dist, "yuzora-*-windows-x86_64.tar.gz"))
    if len(found) != 1 or not os.path.exists(ctx.installer):
        raise RuntimeError("expected one windows archive and the installer in %s, found %s" % (ctx.dist, os.listdir(ctx.dist)))
    ctx.v1 = re.match(r"yuzora-(.+)-windows-x86_64\.tar\.gz$", os.path.basename(found[0])).group(1)
    ctx.v2, ctx.v3 = rs.newer_versions(ctx.v1)
    unpacked = os.path.join(ctx.work, "v1")
    shutil.rmtree(unpacked, ignore_errors=True)
    with tarfile.open(found[0]) as tar:
        tar.extractall(unpacked)
    folder = os.path.join(unpacked, "yuzora")
    with open(os.path.join(folder, "yuzora.exe"), "rb") as f:
        exe_bytes = f.read()
    os.makedirs(os.path.join(ctx.work, "archives"), exist_ok=True)
    notes = ["built version %s, newer ones %s and %s" % (ctx.v1, ctx.v2, ctx.v3)]
    for version in (ctx.v2, ctx.v3):
        patched, count = rs.patch_version(exe_bytes, ctx.v1, version)
        if count == 0:
            raise RuntimeError("the version string %s is not in yuzora.exe" % ctx.v1)
        archive = os.path.join(ctx.work, "archives", "yuzora-%s-windows-x86_64.tar.gz" % version)
        rs.build_archive(folder, archive, patched)
        gate = os.path.join(ctx.work, "gate-" + version)
        shutil.rmtree(gate, ignore_errors=True)
        shutil.copytree(folder, gate)
        with open(os.path.join(gate, "yuzora.exe"), "wb") as f:
            f.write(patched)
        said = run.sh([os.path.join(gate, "yuzora.exe"), "--version"], 30, name="setup/gate-%s.txt" % version).out.strip()
        if said != "herdr-app " + version:
            raise RuntimeError("the patched exe says %r, not 'herdr-app %s'" % (said, version))
        ctx.archives[version] = archive
        notes.append("%s: %d copies of the version string replaced, exe says %r" % (version, count, said))
    run.result("setup.newer_builds", PASS, "; ".join(notes), ["setup/gate-%s.txt" % ctx.v2])
    ctx.server = rs.FakeGithub(ctx.key, os.path.join(run.out, "server.log")).start()
    run.log("stand-in server at %s" % ctx.server.base)
    os.makedirs(ctx.elsewhere, exist_ok=True)


def reader_selfcheck(ctx):
    known = os.path.join(ctx.work, "reader-check")
    os.makedirs(known, exist_ok=True)
    proc = subprocess.Popen([ctx.probe.exe, "hold", "40"], cwd=known, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    ctx.tracker.add(proc.pid)
    time.sleep(1.5)
    info = ctx.probe.procinfo(proc.pid, "setup/reader-selfcheck.txt")
    got = (info.get("peb") or {}).get("cwd_final")
    ok = got is not None and norm(got) == norm(os.path.abspath(known))
    ctx.run.result("setup.cwd_reader_selfcheck", PASS if ok else PROBE_ERROR, "a process started in %s reads back cwd %s" % (known, got), ["setup/reader-selfcheck.txt"])
    ctx.tracker.kill_all()
    return ok


def facts(ctx):
    script = ("Get-CimInstance Win32_OperatingSystem | Format-List Caption,Version,BuildNumber; 'ps ' + $PSVersionTable.PSVersion; "
              "where.exe curl tar powershell; try { 'defender realtime: ' + (Get-MpComputerStatus).RealTimeProtectionEnabled } catch { 'defender: n/a' }; whoami; "
              "Add-Type -AssemblyName System.Windows.Forms; 'screen: ' + [System.Windows.Forms.SystemInformation]::PrimaryMonitorSize")
    ctx.run.sh(["powershell", "-NoProfile", "-NonInteractive", "-Command", script], 120, name="env/facts.txt")
    ctx.probe.version()
    for tool in ("curl", "tar"):
        ctx.run.sh([os.path.join(os.environ.get("SystemRoot", "C:\\Windows"), "System32", tool + ".exe"), "--version"], 30, name="env/%s-version.txt" % tool)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--flavor", choices=["release", "testkey"], required=True)
    parser.add_argument("--dist", required=True)
    parser.add_argument("--probe", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--work", required=True)
    parser.add_argument("--budget", type=int, default=1380)
    args = parser.parse_args()
    run = Run(args.out)
    probe = WinProbe(run, args.probe)
    ctx = Ctx(run, probe, args.flavor, os.path.abspath(args.dist), args.work, args.budget)
    os.makedirs(ctx.work, exist_ok=True)
    run.log("observe: flavor=%s dist=%s work=%s budget=%ss" % (args.flavor, ctx.dist, ctx.work, args.budget))
    try:
        run.guard("setup.facts", lambda: facts(ctx))
        try:
            prepare(ctx)
        except Exception as e:
            run.result("setup.newer_builds", PROBE_ERROR, "%s: %s" % (type(e).__name__, e), [])
            return
        reader_ok = bool(run.guard("setup.cwd_reader_selfcheck", lambda: reader_selfcheck(ctx)))
        if args.flavor == "release":
            chains = [("R1-install", lambda: chain_release(ctx, "install", reader_ok), 150), ("R1-elsewhere", lambda: chain_release(ctx, "elsewhere", reader_ok), 150), ("R2", lambda: chain_release_extra(ctx), 120)]
        else:
            chains = [
                ("D1-install", lambda: chain_testkey(ctx, "install", reader_ok), 200),
                ("D2-install", lambda: chain_testkey(ctx, "install", reader_ok, startup_first=True), 200),
                ("D1-elsewhere", lambda: chain_testkey(ctx, "elsewhere", reader_ok), 200),
                ("C1", lambda: chain_core(ctx, reader_ok), 300),
                ("C2", lambda: chain_old_core(ctx, reader_ok), 240),
                ("C3", lambda: chain_overlay(ctx, "C3", reader_ok, old_core=False, app_running=True), 160),
                ("C4", lambda: chain_overlay(ctx, "C4", reader_ok, old_core=True, app_running=True), 160),
                ("C5", lambda: chain_overlay(ctx, "C5", reader_ok, old_core=True, app_running=False), 160),
            ]
        for name, fn, need in chains:
            if ctx.time_left() < need:
                run.result("%s.skipped" % name, NOT_MEASURED, "not enough of the time budget left (%ds) to start it" % ctx.time_left(), [])
                continue
            run.guard(name + ".chain", fn)
            ctx.sweep()
    finally:
        ctx.sweep()
        if ctx.server:
            ctx.server.stop()
        uninstall_all(ctx, "cleanup")


if __name__ == "__main__":
    main()
