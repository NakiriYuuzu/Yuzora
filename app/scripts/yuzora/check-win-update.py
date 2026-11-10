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
installed exe with its version text replaced by text of the same length: only
where the text stands as a whole version (not `0.1.1` inside `0.1.11`, the
oldest herdr-core the app accepts), and how many copies it replaced and left
alone is printed.

What this covers: the real installer and package; a start with the install
folder as the working directory, which is the shortcut's; the swap and the
PowerShell reopen, twice; the app's own children (there is no herdr-core on
the runner, so none of that).

The app is started as one that was never used on this machine: every HERDR_*,
HC_* and YUZORA_* variable of the caller is dropped, and it gets its own
YUZORA_APP_HOME, its own herdr-core session (HC_SESSION) and its own state and
config folders (XDG_STATE_HOME, XDG_CONFIG_HOME), so nothing it starts can
reach the herdr-core, state or settings of whoever runs the check. Where the
app looks for a herdr-core program (%LOCALAPPDATA%\\Programs\\herdr-core, PATH)
and the working directory its children inherit do not depend on any of them
either, so this does not change what the check measures.

What it does not cover:
 - herdr-core. None is installed here, so the longest-lived holders of the
   folder (`herdr-core serve`, and the `hold` process of each terminal, both
   started by an app that had the folder as its working directory) are not
   exercised. Only the one-off probe (branch spike/v010-win-update-probe)
   looked at them; docs/beta2/release.md has what it found.
 - How the swap is started. The check uses the startup path
   (`apply_on_startup`); "Restart" (`app.update`) and quitting reach the same
   `swap`, but were measured only by the probe.
 - Downloading and checking an update: the staged files are made by hand.
 - A change that does not touch the paths of .github/workflows/app-package.yml
   (src/window.rs, hc.rs, spawn.rs, mods.rs, control.rs are not among them)
   does not start it; run the workflow by hand before a release.
 - A version of the app whose `herdr-app` text (`--version`) or version
   copies the replacement cannot make: that ends the check with exit 2.

It installs, replaces and removes the Yuzora at %LOCALAPPDATA%\\Programs\\Yuzora, so
it refuses to run (exit 2, touching nothing, before it runs the installer) when
Yuzora is running, when that folder exists, or when Yuzora's uninstall entry is
registered: the installer installs into the folder that entry names, so an
install made anywhere with /D= would be replaced and then removed.

usage: python check-win-update.py <setup.exe>
       python check-win-update.py --self-test

Exit 0: passed. Exit 1: the update did not work. Exit 2: the check could not
run (the app did not stay up, a patched copy would not start, wrong usage, a
failing --self-test, or any error the check did not expect): not a result
about the update.
"""
import csv
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import traceback

PROGRAMS = os.path.join(os.environ.get("LOCALAPPDATA", ""), "Programs")
INSTALL = os.path.join(PROGRAMS, "Yuzora")
UNINSTALL = INSTALL + "-uninstall"
UNINSTALL_KEY = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\dev.yuuzu.yuzora"
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


def version_pattern(current):
    """`current` where it stands as a whole version: not the start or end of a longer one (`0.1.1` in `0.1.11`, in `10.1.1`, in `2.0.1.1`)."""
    return re.compile(rb"(?<![0-9])(?<![0-9]\.)" + re.escape(current.encode()) + rb"(?![0-9])(?!\.[0-9])")


def patch_version(data, current, new):
    """The exe's bytes with `current` made `new`; how many copies that replaced and how many it left alone as part of a longer version."""
    if len(current) != len(new):
        raise Harness("versions must have the same length")
    patched, replaced = version_pattern(current).subn(new.encode(), data)
    return patched, replaced, data.count(current.encode()) - replaced


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


def app_env(base, home, work):
    """`base` as an app that was never used here would see it: nothing of the caller's HERDR_*, HC_* or YUZORA_*, and a herdr-core session, state and config of its own under `work`."""
    env = {k: v for k, v in base.items() if not k.upper().startswith(("HERDR_", "HC_", "YUZORA_"))}
    # No release server to reach (the port is closed), and nothing sent to anyone.
    env.update(YUZORA_APP_HOME=home, YUZORA_UPDATE_URL="http://127.0.0.1:9", HERDR_TELEMETRY="0", DO_NOT_TRACK="1", RUST_BACKTRACE="1")
    env.update(HC_SESSION=os.path.basename(work), XDG_STATE_HOME=os.path.join(work, "state"), XDG_CONFIG_HOME=os.path.join(work, "config"))
    return env


def start(label, home, work):
    """Yuzora as the Start menu shortcut starts it: an ordinary process whose working directory is the install folder."""
    env = app_env(os.environ, home, work)
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
    """What `stage` leaves beside the install: the new folder and its receipt. Returns (copies of the version text replaced, copies left alone)."""
    shutil.rmtree(beside(".new"), ignore_errors=True)
    if os.path.exists(beside(".new.json")):
        os.remove(beside(".new.json"))
    shutil.copytree(INSTALL, beside(".new"))
    patched, count, left = patch_version(original["bytes"], original["version"], version)
    if count == 0:
        raise Harness("the installed exe does not contain %r anywhere as a whole version (%d inside longer ones)" % (original["version"], left))
    with open(os.path.join(beside(".new"), "yuzora.exe"), "wb") as f:
        f.write(patched)
    said = version_of(beside(".new"))
    if said != version:
        raise Harness("the patched copy (%d replacements, %d left alone) says %r, not %r" % (count, left, said, version))
    with open(beside(".new.json"), "w") as f:
        f.write(json.dumps({"version": version}))
    print("staged %s: %d copies of %s in the exe replaced, %d inside longer versions left alone" % (version, count, original["version"], left), flush=True)
    return count, left


def update(n, version, was, original, home, work, known):
    copies, _ = stage(version, original)
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


def registered_install():
    """Where Yuzora's uninstall entry says it is installed (an empty string if it names no place), None when there is no entry. The installer installs into that folder, wherever /D= put it."""
    import winreg

    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, UNINSTALL_KEY) as key:
            try:
                return winreg.QueryValueEx(key, "InstallLocation")[0]
            except FileNotFoundError:
                return ""
    except FileNotFoundError:
        return None


def refusal(pids, install_exists, registered):
    """Why this machine is not one to run on, before anything is touched (None: it is one)."""
    if pids:
        return "Yuzora is already running (pids %s)" % sorted(pids)
    if registered is not None:
        return "Yuzora is registered as installed (%s). The installer would install over it, and this check removes what it installs: uninstall it first" % (registered or "no location given")
    if install_exists:
        return "%s exists. This check installs, replaces and removes Yuzora there, and an install that is not its own is not for it to remove: uninstall it first" % INSTALL
    return None


def cleanup(work, before):
    """Takes away what the check made. A step that fails is said and never changes the check's result."""
    steps = (
        ("end the apps it started", lambda: kill(yuzora_pids() - before)),
        ("wait for them", lambda: time.sleep(2)),
        ("uninstall", uninstall),
        ("remove its work folder", lambda: shutil.rmtree(work, ignore_errors=True)),
    )
    for what, step in steps:
        try:
            step()
        except Exception as e:
            print("::warning title=Windows self-update check cleanup::could not %s: %s" % (what, e), flush=True)


def run(setup):
    """Returns 0 once both updates went in; raises Fail (the update did not work) or Harness (the check could not run)."""
    before = yuzora_pids()
    why = refusal(before, os.path.exists(INSTALL), registered_install())
    if why:
        raise Harness(why)
    work = tempfile.mkdtemp(prefix="yuzora-update-check-")
    home = os.path.join(work, "home")
    os.makedirs(home)
    known = set(before)
    try:
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
    finally:
        cleanup(work, before)


def exit_code(check):
    """The exit code for what `check` does: 0 it passed, 1 the update did not work, 2 it could not run, and 2 too for any error nobody expected (an uncaught one would otherwise be Python's 1, the code for a failed update)."""
    try:
        return check()
    except Fail as e:
        print("::error title=Windows self-update failed::%s" % e, flush=True)
        return 1
    except Harness as e:
        print("::error title=Windows self-update check could not run::%s" % e, flush=True)
        return 2
    except Exception as e:
        traceback.print_exc(file=sys.stdout)
        print("::error title=Windows self-update check crashed::%s: %s" % (type(e).__name__, e), flush=True)
        return 2


def self_test():
    assert newer_versions("0.1.0") == ["9.9.1", "9.9.2"]
    assert newer_versions("0.1.0-beta.4") == ["99999999.9.1", "99999999.9.2"]
    assert all(len(v) == len("0.1.0-beta.4") for v in newer_versions("0.1.0-beta.4"))
    data, count, left = patch_version(b"a0.1.0b herdr-app 0.1.0 c", "0.1.0", "9.9.1")
    assert (data, count, left) == (b"a9.9.1b herdr-app 9.9.1 c", 2, 0), (data, count, left)
    assert patch_version(b"none", "0.1.0", "9.9.1") == (b"none", 0, 0)
    # The oldest herdr-core the app takes is `0.1.11` (distribution/core-version): never made `9.9.11`.
    data, count, left = patch_version(b"core 0.1.11\n Yuzora/0.1.1\0 10.1.1 2.0.1.1 0.1.1.5 0.1.1.", "0.1.1", "9.9.1")
    assert (data, count, left) == (b"core 0.1.11\n Yuzora/9.9.1\0 10.1.1 2.0.1.1 0.1.1.5 9.9.1.", 2, 4), (data, count, left)
    data, count, left = patch_version(b"[0.1.0-beta.4] herdr-app 0.1.0", "0.1.0-beta.4", "99999999.9.1")
    assert (data, count, left) == (b"[99999999.9.1] herdr-app 0.1.0", 1, 0), (data, count, left)
    listing = '"yuzora.exe","1234","Console","1","12,345 K"\n"yuzora.exe","77","Console","1","9 K"\n'
    assert tasklist_pids(listing) == {1234, 77}
    assert tasklist_pids("INFO: No tasks are running which match the specified criteria.\n") == set()
    for short in ("0.1", "0.1.0-beta.4.extra.long"):
        try:
            newer_versions(short)
        except Harness:
            continue
        raise AssertionError("expected a harness error for %r" % short)

    assert refusal(set(), False, None) is None
    assert "already running" in refusal({4}, False, None)
    assert "uninstall it first" in refusal(set(), True, None)
    elsewhere = refusal(set(), False, r"D:\Apps\Yuzora")
    assert r"D:\Apps\Yuzora" in elsewhere and "uninstall it first" in elsewhere, elsewhere
    assert "registered" in refusal(set(), False, "")
    assert "already running" in refusal({4}, True, r"D:\Apps\Yuzora")

    class Registry:
        """Stands for winreg with one uninstall entry (or none)."""

        HKEY_CURRENT_USER = "HKCU"

        def __init__(self, entry):
            self.entry = entry

        def OpenKey(self, root, path):
            assert (root, path) == ("HKCU", UNINSTALL_KEY), (root, path)
            if self.entry is None:
                raise FileNotFoundError(path)
            return Opened(self.entry)

        def QueryValueEx(self, key, name):
            assert name == "InstallLocation", name
            if "InstallLocation" not in key.entry:
                raise FileNotFoundError(name)
            return key.entry["InstallLocation"], 1

    class Opened:
        def __init__(self, entry):
            self.entry = entry

        def __enter__(self):
            return self

        def __exit__(self, *_):
            return False

    real_winreg = sys.modules.get("winreg")
    try:
        sys.modules["winreg"] = Registry(None)
        assert registered_install() is None
        sys.modules["winreg"] = Registry({"InstallLocation": r"D:\Apps\Yuzora"})
        assert registered_install() == r"D:\Apps\Yuzora"
        sys.modules["winreg"] = Registry({})
        assert registered_install() == ""
    finally:
        if real_winreg is None:
            del sys.modules["winreg"]
        else:
            sys.modules["winreg"] = real_winreg

    caller = {
        "PATH": "p", "LOCALAPPDATA": "l", "TEMP": "t", "XDG_STATE_HOME": "real-state", "XDG_CONFIG_HOME": "real-config",
        "HC_SESSION": "mine", "HC_SOCKET": "s", "HC_INSTALL_DIR": "i", "HC_TERMINAL_ID": "t1", "hc_scope": "x",
        "HERDR_ENV": "1", "HERDR_SOCKET_PATH": "s", "YUZORA_APP_HOME": "real-home", "YUZORA_UPDATE_URL": "https://example.invalid",
    }
    work = os.path.join("scratch", "yuzora-update-check-ab12")
    env = app_env(caller, os.path.join(work, "home"), work)
    assert not [k for k in env if k.upper().startswith(("HC_", "HERDR_", "YUZORA_")) and k not in ("HC_SESSION", "HERDR_TELEMETRY", "YUZORA_APP_HOME", "YUZORA_UPDATE_URL")], env
    assert (env["PATH"], env["LOCALAPPDATA"], env["TEMP"]) == ("p", "l", "t"), env
    assert env["HC_SESSION"] == "yuzora-update-check-ab12", env
    assert env["XDG_STATE_HOME"] == os.path.join(work, "state") and env["XDG_CONFIG_HOME"] == os.path.join(work, "config"), env
    assert env["YUZORA_APP_HOME"] == os.path.join(work, "home") and env["YUZORA_UPDATE_URL"] == "http://127.0.0.1:9", env
    assert "mine" not in env.values() and "real-home" not in env.values() and "real-state" not in env.values(), env
    assert app_env({}, "h", os.path.join("a", "yuzora-update-check-cd34"))["HC_SESSION"] != env["HC_SESSION"]

    class Quiet:
        """Stands for stdout while an expected annotation prints."""

        def write(self, _text):
            return 0

        def flush(self):
            pass

    def raises(error):
        def check():
            raise error

        return check

    real = sys.stdout
    sys.stdout = Quiet()
    try:
        codes = {
            "passed": exit_code(lambda: 0),
            "update failed": exit_code(raises(Fail("no"))),
            "could not run": exit_code(raises(Harness("no"))),
            "installer hung": exit_code(raises(subprocess.TimeoutExpired("setup", 180))),
            "file error": exit_code(raises(OSError("disk"))),
            "bug": exit_code(raises(KeyError("x"))),
        }
        steps_fail = [0]

        def stubborn():
            steps_fail[0] += 1
            raise OSError("in use")

        real_kill, real_uninstall, real_pids = globals()["kill"], globals()["uninstall"], globals()["yuzora_pids"]
        globals()["kill"] = lambda pids: stubborn()
        globals()["uninstall"] = stubborn
        globals()["yuzora_pids"] = lambda: set()
        try:
            cleaned = exit_code(lambda: cleanup(tempfile.mkdtemp(), set()) or 0)
        finally:
            globals()["kill"], globals()["uninstall"], globals()["yuzora_pids"] = real_kill, real_uninstall, real_pids
    finally:
        sys.stdout = real
    assert codes == {"passed": 0, "update failed": 1, "could not run": 2, "installer hung": 2, "file error": 2, "bug": 2}, codes
    assert steps_fail[0] == 2 and cleaned == 0, (steps_fail, cleaned)
    sys.stderr = Quiet()
    try:
        wrong_usage = (main(["--no-such-flag", "x"]), main([]))
    finally:
        sys.stderr = sys.__stderr__
    assert wrong_usage == (2, 2), wrong_usage

    def patched(**names):
        class Patch:
            def __enter__(self):
                self.old = {name: globals()[name] for name in names}
                globals().update(names)

            def __exit__(self, *_):
                globals().update(self.old)

        return Patch()

    spawned = []

    def spawn(*args, **kwargs):
        spawned.append(args)
        raise AssertionError("started a program: %r" % (args,))

    def broken_self_test():
        raise AssertionError("the check itself is wrong")

    real_run = subprocess.run
    subprocess.run = spawn
    sys.stdout = Quiet()
    try:
        with patched(registered_install=lambda: r"D:\Apps\Yuzora", yuzora_pids=lambda: set()):
            try:
                run("setup.exe")
                refused = None
            except Harness as e:
                refused = str(e)
        with patched(self_test=broken_self_test):
            broken = main(["--self-test"])
    finally:
        sys.stdout = real
        subprocess.run = real_run
    assert refused and r"D:\Apps\Yuzora" in refused and spawned == [], (refused, spawned)
    assert broken == 2, broken
    print("self-test passed")
    return 0


def main(argv):
    if argv == ["--self-test"]:
        return exit_code(self_test)
    if len(argv) != 1:
        print(__doc__, file=sys.stderr)
        return 2
    return exit_code(lambda: run(os.path.abspath(argv[0])))


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
