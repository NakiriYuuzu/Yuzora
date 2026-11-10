"""Windows: does the installed, shipped Yuzora replace itself, and again?

The Start menu shortcut starts Yuzora with its install folder as the working
directory, and an update renames that folder (`swap` in src/update.rs), which
Windows refuses while any program has it as its working directory (os error
32). This installs the package with its own installer, starts the app the way
the shortcut does with an update staged beside it, and checks that the folder
changes to the new version, that the app reopens, that nothing it started
keeps the folder from being renamed, and that a second update goes in too.

The shipped build only trusts the release key, so it cannot be offered an
update. What it can be given is what `stage` leaves beside the install
(`Yuzora.new` and its receipt); the app then applies it at startup, the path
that also recovers a window that never quit cleanly. The newer version is the
installed exe with its version text replaced by text of the same length.

usage: python check-win-update.py <setup.exe>
       python check-win-update.py --self-test

Exit 0: passed. Exit 1: the update did not work. Exit 2: the check could not
run (the app did not stay up, a patched copy would not start): not a result.
"""
import csv
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

PROGRAMS = os.path.join(os.environ.get("LOCALAPPDATA", ""), "Programs")
INSTALL = os.path.join(PROGRAMS, "Yuzora")
UNINSTALL = INSTALL + "-uninstall"
# How long the app must stay up for its window to count as open (it checks for updates at 10 s).
STAYS_UP = 15
SWAP_WITHIN = 60


class Fail(Exception):
    """The update did not work."""


class Harness(Exception):
    """The check could not run: says nothing about the update."""


def beside(suffix):
    return INSTALL + suffix


def newer_versions(current, count=2):
    """Stable versions of the same length as `current` and later than it, so the exe can say it is one by replacing bytes in place."""
    n = len(current)
    if not 5 <= n <= 13:
        raise Harness("cannot make a same-length newer version of %r" % current)
    return ["%s.9.%d" % ("9" * (n - 4), k) for k in range(1, count + 1)]


def patch_version(data, current, new):
    if len(current) != len(new):
        raise Harness("versions must have the same length")
    return data.replace(current.encode(), new.encode()), data.count(current.encode())


def tasklist_pids(text):
    return {int(row[1]) for row in csv.reader(text.splitlines()) if len(row) > 1 and row[0].lower() == "yuzora.exe"}


def yuzora_pids():
    out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq yuzora.exe", "/FO", "CSV", "/NH"], capture_output=True, text=True).stdout
    return tasklist_pids(out)


def kill(pids):
    for pid in pids:
        subprocess.run(["taskkill", "/F", "/PID", str(pid)], capture_output=True)


def wait_for(condition, seconds, interval=1.0):
    end = time.time() + seconds
    while True:
        value = condition()
        if value:
            return value
        if time.time() >= end:
            return None
        time.sleep(interval)


def version_of(folder):
    """What `yuzora --version` says in `folder` (`0.1.0`), else None."""
    exe = os.path.join(folder, "yuzora.exe")
    if not os.path.exists(exe):
        return None
    try:
        out = subprocess.run([exe, "--version"], capture_output=True, text=True, timeout=30).stdout.split()
    except (OSError, subprocess.TimeoutExpired):
        return None
    return out[1] if len(out) == 2 and out[0] == "herdr-app" else None


def tail(path, lines=8):
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            return " | ".join(line.strip() for line in f.read().strip().splitlines()[-lines:]) or "(nothing logged)"
    except OSError:
        return "(no log)"


def start(label, home, work):
    """Yuzora as the Start menu shortcut starts it: an ordinary process whose working directory is the install folder."""
    env = {k: v for k, v in os.environ.items() if not k.startswith(("HERDR_", "YUZORA_"))}
    # No release server to reach (the port is closed), and nothing sent to anyone.
    env.update(YUZORA_APP_HOME=home, YUZORA_UPDATE_URL="http://127.0.0.1:9", HERDR_TELEMETRY="0", DO_NOT_TRACK="1", RUST_BACKTRACE="1")
    log = os.path.join(work, label + ".log")
    with open(log, "wb") as out:
        proc = subprocess.Popen([os.path.join(INSTALL, "yuzora.exe")], cwd=INSTALL, env=env, stdin=subprocess.DEVNULL, stdout=out, stderr=subprocess.STDOUT)
    print("%s: pid %d started in %s" % (label, proc.pid, INSTALL), flush=True)
    return proc, log


def folder_is_free(who):
    """The install folder can be renamed (and back) while `who` runs: no program has it as its working directory."""
    moved = INSTALL + ".probe"
    try:
        os.rename(INSTALL, moved)
    except OSError as e:
        raise Fail("the install folder cannot be renamed while %s runs: %s. An update renames it." % (who, e))
    try:
        os.rename(moved, INSTALL)
    except OSError as e:
        raise Harness("could not put the install folder back: %s" % e)


def stay_up(proc, log, who, seconds=STAYS_UP):
    """Waits `seconds`; a program that ends meanwhile means the window cannot open here, which is not an update result."""
    ended = wait_for(lambda: proc.poll() is not None, seconds)
    if ended:
        raise Harness("%s ended within %d s (exit code %s), so there is no running app to read results from; the window probably cannot open on this machine. Its log: %s" % (who, seconds, proc.poll(), tail(log)))


def stage(version, original):
    """What `stage` leaves beside the install: the new folder and its receipt. Returns how many copies of the version text the exe had."""
    shutil.rmtree(beside(".new"), ignore_errors=True)
    if os.path.exists(beside(".new.json")):
        os.remove(beside(".new.json"))
    shutil.copytree(INSTALL, beside(".new"))
    patched, count = patch_version(original["bytes"], original["version"], version)
    if count == 0:
        raise Harness("the installed exe does not contain %r anywhere" % original["version"])
    with open(os.path.join(beside(".new"), "yuzora.exe"), "wb") as f:
        f.write(patched)
    said = version_of(beside(".new"))
    if said != version:
        raise Harness("the patched copy (%d replacements) says %r, not %r" % (count, said, version))
    with open(beside(".new.json"), "w") as f:
        f.write(json.dumps({"version": version}))
    return count


def update(n, version, was, original, home, work, known):
    copies = stage(version, original)
    proc, log = start("update%d" % n, home, work)
    known.add(proc.pid)
    if not wait_for(lambda: version_of(INSTALL) == version, SWAP_WITHIN):
        text = tail(log, 12)
        if "update: cannot" in text:
            raise Fail("update %d did not go in: the install folder still runs %s. The app said: %s" % (n, version_of(INSTALL), text))
        if proc.poll() not in (None, 0):
            raise Harness("update %d: the app ended with exit code %s before it applied anything. Its log: %s" % (n, proc.poll(), text))
        raise Fail("update %d did not go in within %d s: the install folder still runs %s. The app said: %s" % (n, SWAP_WITHIN, version_of(INSTALL), text))
    if version_of(beside(".old")) != was:
        raise Fail("update %d: the old folder runs %s, not %s" % (n, version_of(beside(".old")), was))
    if os.path.exists(beside(".new")) or os.path.exists(beside(".new.json")):
        raise Fail("update %d: what was staged is still beside the install after the swap" % n)
    print("update %d: the install folder now runs %s, %s runs %s" % (n, version, os.path.basename(beside(".old")), was), flush=True)
    reopened = wait_for(lambda: yuzora_pids() - known, SWAP_WITHIN)
    if not reopened:
        raise Fail("update %d: the app did not reopen itself after the swap (the PowerShell line)" % n)
    known |= reopened
    wait_for(lambda: proc.poll() is not None, 30)
    deadline = time.time() + STAYS_UP
    while time.time() < deadline:
        if not (reopened & yuzora_pids()):
            raise Harness("update %d: the reopened app ended within %d s; the app started before it stayed up, so suspect the patched copy (%d copies of %r replaced in its exe) or the window, not the update" % (n, STAYS_UP, copies, original["version"]))
        time.sleep(1)
    folder_is_free("the app it reopened")
    print("update %d: the reopened app (pid %s) is running and the install folder can be renamed" % (n, ", ".join(map(str, sorted(reopened)))), flush=True)
    kill(reopened)
    wait_for(lambda: not (reopened & yuzora_pids()), 20)


def uninstall():
    uninstaller = os.path.join(UNINSTALL, "uninstall.exe")
    if os.path.exists(uninstaller):
        subprocess.run([uninstaller, "/S", "_?=" + UNINSTALL], timeout=120, capture_output=True)
    for path in (INSTALL, INSTALL + ".old", INSTALL + ".new", INSTALL + ".download", INSTALL + ".probe", UNINSTALL):
        shutil.rmtree(path, ignore_errors=True)
    if os.path.exists(beside(".new.json")):
        os.remove(beside(".new.json"))


def run(setup):
    work = tempfile.mkdtemp(prefix="yuzora-update-check-")
    home = os.path.join(work, "home")
    os.makedirs(home)
    before = yuzora_pids()
    known = set(before)
    try:
        if before:
            raise Harness("Yuzora is already running (pids %s)" % sorted(before))
        uninstall()
        done = subprocess.run([setup, "/S"], timeout=180)
        if done.returncode != 0:
            raise Harness("the installer exited with %s" % done.returncode)
        current = version_of(INSTALL)
        if current is None:
            raise Harness("the installed yuzora.exe does not answer --version")
        with open(os.path.join(INSTALL, "yuzora.exe"), "rb") as f:
            original = {"bytes": f.read(), "version": current}
        first, second = newer_versions(current)
        print("installed %s; updates will be %s and %s, made by replacing the version text in the exe" % (current, first, second), flush=True)

        proc, log = start("control", home, work)
        known.add(proc.pid)
        stay_up(proc, log, "the shipped app started in its install folder")
        folder_is_free("the app started in its install folder")
        kill([proc.pid])
        proc.wait(timeout=20)

        update(1, first, current, original, home, work, known)
        update(2, second, first, original, home, work, known)
        print("passed: the installed app updated itself twice from a start in its install folder", flush=True)
        return 0
    except Fail as e:
        print("::error title=Windows self-update failed::%s" % e, flush=True)
        return 1
    except Harness as e:
        print("::error title=Windows self-update check could not run::%s" % e, flush=True)
        return 2
    finally:
        kill(yuzora_pids() - before)
        time.sleep(2)
        try:
            uninstall()
        finally:
            shutil.rmtree(work, ignore_errors=True)


def self_test():
    assert newer_versions("0.1.0") == ["9.9.1", "9.9.2"]
    assert newer_versions("0.1.0-beta.4") == ["99999999.9.1", "99999999.9.2"]
    assert all(len(v) == len("0.1.0-beta.4") for v in newer_versions("0.1.0-beta.4"))
    data, count = patch_version(b"a0.1.0b herdr-app 0.1.0 c", "0.1.0", "9.9.1")
    assert (data, count) == (b"a9.9.1b herdr-app 9.9.1 c", 2), (data, count)
    assert patch_version(b"none", "0.1.0", "9.9.1") == (b"none", 0)
    listing = '"yuzora.exe","1234","Console","1","12,345 K"\n"yuzora.exe","77","Console","1","9 K"\n'
    assert tasklist_pids(listing) == {1234, 77}
    assert tasklist_pids("INFO: No tasks are running which match the specified criteria.\n") == set()
    for short in ("0.1", "0.1.0-beta.4.extra.long"):
        try:
            newer_versions(short)
        except Harness:
            continue
        raise AssertionError("expected a harness error for %r" % short)
    print("self-test passed")
    return 0


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        sys.exit(self_test())
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    sys.exit(run(os.path.abspath(sys.argv[1])))
