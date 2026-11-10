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
           `yuzora do app.update`, the Restart button).

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
import time

import common
import release_server as rs
from common import FAIL, INFO, NOT_MEASURED, PASS, PROBE_ERROR, Run, Tracker, WinProbe, inventory, read_text, tail, wait_for

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
    ctx.tracker.kill_all()
    uninstall_all(ctx, cid)
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
        return read_text(self.log_path) if self.log_path else ""

    def stop(self):
        """What a person ends a window app with, then the end for one that will not."""
        if self.alive():
            self.ctx.probe.close(self.pid)
            if not self.wait_exit(15):
                self.ctx.tracker.kill_all()
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


def observe_swap(ctx, cid, k, to_version, old, home, expect_relaunch, known, reader_ok):
    """After `old` was told to apply an update: did the folder change, and did a new process appear?
    Returns (swapped, reopened): whether the folder now runs `to_version`, and the Instance the app
    reopened itself as (None when it did not, or when `expect_relaunch` is off)."""
    run, probe = ctx.run, ctx.probe
    known.add(old.pid)
    exited = old.wait_exit(90)
    time.sleep(1)
    post = snap(ctx, cid, "after-swap%d" % k)
    marker = os.path.exists(os.path.join(home, ".updated"))
    run.evidence("%s/home-after-swap%d.json" % (cid, k), json.dumps(sorted(os.listdir(home)) if os.path.isdir(home) else None))
    evidence = ["%s/snapshot-after-swap%d.json" % (cid, k)] + ([old.log_rel] if old.log_rel else [])
    if post["install_version"] != to_version:
        run.result("%s.swap_%d" % (cid, k), FAIL, "the folder still runs %s, not %s. old process exited=%s. its log: %s" % (post["install_version"], to_version, exited, tail(old.log_text(), 8).replace("\n", " | ") or "(none kept)"), evidence)
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
    run.result("%s.relaunch_%d" % (cid, k), PASS if ok_image else FAIL, "pid %s runs %s, parent %s (pid %s), command line %s" % (pid, image, info.get("parent_name"), info.get("ppid"), peb.get("cmdline")), ev)
    kind, cwd = cwd_class(info)
    if not reader_ok or kind == "unknown":
        run.result("%s.relaunch_%d_cwd" % (cid, k), NOT_MEASURED, "cwd not read (reader self-check ok=%s)" % reader_ok, ev)
    elif kind == "inside-old":
        run.result("%s.relaunch_%d_cwd" % (cid, k), FAIL, "the reopened app's cwd handle is %s, inside the old folder: nothing can delete Yuzora.old while it runs" % cwd, ev)
    else:
        run.result("%s.relaunch_%d_cwd" % (cid, k), PASS, "the reopened app's cwd handle is %s (%s); PEB string %s" % (cwd, kind, peb.get("cwd_dos")), ev)
    trust = info.get("redirection_trust") or {}
    if trust.get("measured"):
        run.result("%s.relaunch_%d_redirection_trust" % (cid, k), INFO, "EnforceRedirectionTrust=%s AuditRedirectionTrust=%s (flags %s); parent was %s" % (trust.get("enforce"), trust.get("audit"), trust.get("flags"), info.get("parent_name")), ev)
    else:
        run.result("%s.relaunch_%d_redirection_trust" % (cid, k), NOT_MEASURED, "GetProcessMitigationPolicy(RedirectionTrust) failed with Win32 error %s on this Windows" % trust.get("win32_error"), ev)
    alive = wait_for(lambda: ctx.ua_seen(to_version), 40)
    run.result("%s.relaunch_%d_alive" % (cid, k), PASS if alive else FAIL, "the reopened app %s asked the stand-in server as Yuzora/%s within 40 s" % ("did" if alive else "did NOT", to_version), ["server.log"])
    return True, Instance(ctx, pid, "relaunched-%d" % k)


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
            for later in range(k + 1, 3):
                ctx.run.result("%s.swap_%d" % (cid, later), NOT_MEASURED, "update %d did not complete" % k, [])
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
    """Waits for `version` to be staged by the app's own check. Also notes whether a window showed."""
    seen = {"window": None}

    def done():
        if seen["window"] is None:
            visible = [w for w in ctx.probe.windows(inst.pid) if w.get("visible")]
            if visible:
                seen["window"] = visible
        return staged(version) or not inst.alive()
    wait_for(done, timeout, 2)
    return staged(version), seen["window"], inst.alive()


def chain_testkey(ctx, mode, reader_ok):
    """D1: the whole path against the stand-in server, twice; applied when the window closes."""
    cid = "D1-" + mode
    home = new_home(ctx, cid)
    cwd = INSTALL if mode == "install" else ctx.elsewhere
    install_fresh(ctx, cid)
    ctx.offer(ctx.v2)
    known = set()
    app = start_app(ctx, cid, "app1", home, cwd)
    for k, to_v in [(1, ctx.v2), (2, ctx.v3)]:
        mark = len(ctx.server_entries())
        ok, window, alive = wait_staged_and_window(ctx, app, to_v, 170)
        if k == 1:
            ctx.run.result("%s.window_opened" % cid, INFO if window else NOT_MEASURED, "visible top-level windows of the app: %s" % (json.dumps(window) if window else "none seen"), [])
        requests = [e["path"].split("?")[0] for e in ctx.server_entries()[mark:]]
        ev = ["server.log"] + ([app.log_rel] if app.log_rel else [])
        if not ok:
            ctx.run.result("%s.stage_%d" % (cid, k), FAIL, "%s was not staged: app alive=%s (exit code %s), requests seen=%s, log: %s" % (to_v, alive, app.proc.poll() if app.proc else "n/a", requests, tail(app.log_text(), 8).replace("\n", " | ")), ev)
            for later in range(k + 1, 3):
                ctx.run.result("%s.stage_%d" % (cid, later), NOT_MEASURED, "update %d was not staged" % k, [])
            return
        ctx.run.result("%s.stage_%d" % (cid, k), PASS, "check, download, signature, checksum, unpack and stage all ran in the app: requests %s; Yuzora.new + receipt for %s" % (requests, to_v), ev)
        ctx.offer(ctx.v3)
        snap(ctx, cid, "before-swap%d" % k)
        closed = ctx.probe.close(app.pid)
        quit_path = bool(closed.get("posted"))
        ctx.run.result("%s.trigger_%d" % (cid, k), INFO, ("closed %s window(s) with WM_CLOSE, as taskkill does: the app quits and applies the update" % closed.get("posted")) if quit_path else "no window to close (%s windows found): the app was ended and started again, so the staged files are applied at startup" % closed.get("windows"), [])
        if quit_path:
            swapped, _ = observe_swap(ctx, cid, k, to_v, app, home, False, known, reader_ok)
            if not swapped:
                for later in range(k + 1, 3):
                    ctx.run.result("%s.stage_%d" % (cid, later), NOT_MEASURED, "update %d was not applied" % k, [])
                return
            app = start_app(ctx, cid, "app%d" % (k + 1), home, cwd)
            up = wait_for(lambda: ctx.ua_seen(to_v, mark), 60)
            ctx.run.result("%s.start_after_swap_%d" % (cid, k), PASS if up and app.alive() else FAIL, "started from the Start-in folder after the swap: alive=%s, asked the server as Yuzora/%s=%s" % (app.alive(), to_v, bool(up)), ["server.log", app.log_rel])
        else:
            ctx.tracker.kill_all()
            app.wait_exit(10)
            restarted = start_app(ctx, cid, "app%d-restart" % k, home, cwd)
            swapped, reopened = observe_swap(ctx, cid, k, to_v, restarted, home, True, known, reader_ok)
            if not swapped:
                for later in range(k + 1, 3):
                    ctx.run.result("%s.stage_%d" % (cid, later), NOT_MEASURED, "update %d was not applied" % k, [])
                return
            # A reopen that did not happen is itself a result; a person would start the app again.
            app = reopened or start_app(ctx, cid, "app%d-by-hand" % (k + 1), home, cwd)


def run_cli(ctx, home, *words, timeout=60):
    """The control client, run from a copy of the exe outside the install folder: a client inside
    it would be one more program the swap has to move past."""
    return ctx.run.sh([ctx.cli] + list(words), timeout, env=app_env(ctx, home), cwd=ctx.elsewhere, name="C1/cli-%s-%d.txt" % ("-".join(words).replace(".", "_"), int(time.time())))


def main_window_up(ctx, home):
    shell = ctx.run.sh([ctx.cli, "status"], 20, env=app_env(ctx, home), cwd=ctx.elsewhere)
    return shell.rc == 0 and isinstance(shell.json(), dict)


def chain_core(ctx, reader_ok):
    """C1: herdr-core installed, the main window up, the update applied by `yuzora do app.update`."""
    cid = "C1"
    home = new_home(ctx, cid)
    shell = ctx.run.sh(["powershell", "-NoProfile", "-NonInteractive", "-Command", CORE_INSTALLER], 300, name="C1/core-install.txt")
    core = next((p for p in (os.path.join(PROGRAMS, "herdr-core", n) for n in ("herdr-core.exe", "hc.exe")) if os.path.exists(p)), None)
    version = ctx.run.sh([core, "--version"], 30, name="C1/core-version.txt").out.strip() if core else ""
    if not version.startswith("hc "):
        ctx.run.result("C1.core_install", NOT_MEASURED, "herdr-core did not install from releases.herdr.dev (rc=%s, found=%s %r): no main window, no Restart button, no foreign cwd holders" % (shell.rc, core, version), ["C1/core-install.txt"])
        return
    ctx.run.result("C1.core_install", PASS, "%s at %s" % (version, core), ["C1/core-install.txt"])
    install_fresh(ctx, cid)
    os.makedirs(os.path.join(ctx.work, "cli"), exist_ok=True)
    ctx.cli = os.path.join(ctx.work, "cli", "yuzora.exe")
    shutil.copy(EXE, ctx.cli)
    ctx.offer(ctx.v2)
    known = set()
    app = start_app(ctx, cid, "app1", home, INSTALL)
    for k, to_v in [(1, ctx.v2), (2, ctx.v3)]:
        up = wait_for(lambda: main_window_up(ctx, home) or not app.alive(), 150, 3)
        if not up or not app.alive():
            ctx.run.result("%s.main_window_%d" % (cid, k), NOT_MEASURED, "no main window answered `yuzora status` (app alive=%s): the setup screen does not serve commands. log: %s" % (app.alive(), tail(app.log_text(), 6).replace("\n", " | ")), [app.log_rel] if app.log_rel else [])
            return
        ctx.run.result("%s.main_window_%d" % (cid, k), PASS, "`yuzora status` answered: the main window is up on herdr-core", [])
        if k == 1:
            rows = ctx.probe.ps()
            holders = [p for p in rows if p["name"].lower() in ("hc.exe", "herdr-core.exe") or p["pid"] in ctx.probe.descendants(app.pid, rows)]
            lines = []
            for p in holders:
                ctx.tracker.add(p["pid"])
                info = ctx.probe.procinfo(p["pid"], "%s/procinfo-holder-%d.txt" % (cid, p["pid"]))
                lines.append("%s pid %s cwd %s" % (p["name"], p["pid"], (info.get("peb") or {}).get("cwd_final")))
            inside = [l for l in lines if norm(INSTALL) in norm(l)]
            ctx.run.result("C1.foreign_cwd_holders", INFO if reader_ok else NOT_MEASURED, "%d program(s) the app started or that serve it; %d with a cwd in the install folder: %s" % (len(holders), len(inside), "; ".join(lines)), ["%s/procinfo-holder-%d.txt" % (cid, p["pid"]) for p in holders])
        run_cli(ctx, home, "do", "app.check_updates")
        if not wait_for(lambda: staged(to_v), 90, 2):
            ctx.run.result("%s.stage_%d" % (cid, k), FAIL, "%s not staged within 90 s of `app.check_updates`; log: %s" % (to_v, tail(app.log_text(), 6).replace("\n", " | ")), ["server.log"])
            return
        ctx.run.result("%s.stage_%d" % (cid, k), PASS, "staged %s by the app's own check" % to_v, ["server.log"])
        ctx.offer(ctx.v3)
        snap(ctx, cid, "before-swap%d" % k)
        reply = run_cli(ctx, home, "do", "app.update")
        ctx.run.result("%s.restart_command_%d" % (cid, k), INFO, "`yuzora do app.update` rc=%s: %s" % (reply.rc, (reply.out or reply.err).strip().replace("\n", " ")), [])
        swapped, reopened = observe_swap(ctx, cid, k, to_v, app, home, True, known, reader_ok)
        if not swapped:
            return
        app = reopened or start_app(ctx, cid, "app%d-by-hand" % (k + 1), home, INSTALL)
    time.sleep(45)
    after = snap(ctx, cid, "after-core-update")
    ctx.run.result("C1.old_folder_after_core_update", INFO, "45 s after the second update: Yuzora.old %s (%s); marker .updated %s" % ("is still there" if "Yuzora.old" in after["siblings"] else "is gone", after["old_files"][:6], "still there" if os.path.exists(os.path.join(home, ".updated")) else "consumed"), ["C1/snapshot-after-core-update.json"])


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
    parser.add_argument("--budget", type=int, default=1100)
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
            chains = [("D1-install", lambda: chain_testkey(ctx, "install", reader_ok), 300), ("D1-elsewhere", lambda: chain_testkey(ctx, "elsewhere", reader_ok), 300), ("C1", lambda: chain_core(ctx, reader_ok), 420)]
        for name, fn, need in chains:
            if ctx.time_left() < need:
                run.result("%s.skipped" % name, NOT_MEASURED, "not enough of the time budget left (%ds) to start it" % ctx.time_left(), [])
                continue
            run.guard(name + ".chain", fn)
            ctx.tracker.kill_all()
    finally:
        ctx.tracker.kill_all()
        if ctx.server:
            ctx.server.stop()
        uninstall_all(ctx, "cleanup")


if __name__ == "__main__":
    main()
