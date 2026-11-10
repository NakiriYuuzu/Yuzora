"""Checks the probe's own parts before any of it is believed: the signer against the
fixture src/yuzora/release/fixtures (signed by a key whose secret half is gone), the
manifest against the rules of manifest.rs `checks`, the server's answers, the version
and archive helpers. Runs anywhere:  python -I selftest.py
"""
import base64
import hashlib
import json
import os
import re
import sys
import tarfile
import tempfile
import unittest
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import release_server as rs  # noqa: E402

FIXTURES = os.path.join(HERE, "..", "..", "..", "src", "yuzora", "release", "fixtures")


def fixture(name):
    with open(os.path.join(FIXTURES, name), "rb") as f:
        return f.read()


def plausible(version):
    """channel.rs `plausible`."""
    core, _, pre = version.partition("-")
    numbers = core.split(".")
    ok = len(numbers) == 3 and all(n.isdigit() and 0 < len(n) <= 9 for n in numbers)
    return ok and (not pre or all(part.isalnum() and part.isascii() for part in pre.split(".")))


def newer(a, b):
    """update.rs `newer` for the shapes used here."""
    def parts(v):
        core, _, pre = v.partition("-")
        return [int(n) for n in core.split(".")], pre
    (ca, pa), (cb, pb) = parts(a), parts(b)
    return ca > cb or (ca == cb and pa == "" and pb != "")


def manifest_checks(manifest, version, downloads):
    """manifest.rs `checks`; returns the first complaint or None."""
    if manifest["schema"] != 1 or manifest["name"] != "yuzora" or manifest["version"] != version:
        return "identity"
    if manifest["channel"] != ("beta" if "-" in version else "stable"):
        return "channel"
    prefix = "%s/v%s/" % (downloads, version)
    for target, asset in manifest["targets"].items():
        name = asset["url"][len(prefix):] if asset["url"].startswith(prefix) else None
        if not name or not re.fullmatch(r"[A-Za-z0-9._-]+", name):
            return "url of " + target
        if not re.fullmatch(r"[0-9a-f]{64}", asset["sha256"]):
            return "sha256 of " + target
    return None


class Signer(unittest.TestCase):
    def test_rfc8032_vector_1(self):
        secret = bytes.fromhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
        self.assertEqual(rs.ed_public(secret).hex(), "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
        self.assertEqual(
            rs.ed_sign(secret, b"").hex(),
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        )

    def test_agrees_with_cryptography_when_present(self):
        try:
            from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
        except ImportError:
            self.skipTest("no cryptography here")
        secret = os.urandom(32)
        key = Ed25519PrivateKey.from_private_bytes(secret)
        self.assertEqual(key.sign(b"x" * 100), rs.ed_sign(secret, b"x" * 100))

    def test_the_committed_fixture_verifies(self):
        key = fixture("test-key.pub").decode().strip()
        manifest, signature = fixture("yuzora-update.json"), fixture("yuzora-update.json.sig").decode()
        self.assertTrue(rs.minisign_verify(key, manifest, signature))
        self.assertFalse(rs.minisign_verify(key, manifest + b" ", signature))

    def test_own_signatures_round_trip_and_tampering_fails(self):
        key = rs.TestKey()
        text = key.sign(b"manifest bytes")
        self.assertTrue(rs.minisign_verify(key.public_base64(), b"manifest bytes", text))
        self.assertTrue(rs.minisign_verify(key.public_base64(), b"manifest bytes", base64.b64encode(text.encode()).decode()))
        self.assertFalse(rs.minisign_verify(key.public_base64(), b"manifest byteS", text))
        self.assertFalse(rs.minisign_verify(rs.TestKey().public_base64(), b"manifest bytes", text))
        self.assertEqual(len(base64.b64decode(key.public_base64())), 42)
        self.assertTrue(key.public_base64().startswith("RW"))


class Versions(unittest.TestCase):
    def test_newer_versions_are_stable_plausible_later_and_as_long(self):
        for current in ["0.1.0-beta.4", "0.1.0", "0.1.0-beta.10"]:
            for version in rs.newer_versions(current):
                self.assertEqual(len(version), len(current))
                self.assertTrue(plausible(version), version)
                self.assertTrue(newer(version, current), version)
        first, second = rs.newer_versions("0.1.0-beta.4")
        self.assertTrue(newer(second, first))

    def test_patching_replaces_every_copy_and_only_those(self):
        exe = b"MZ..herdr-app 0.1.0-beta.4..Yuzora/0.1.0-beta.4..0.1.0-beta.5.."
        patched, count = rs.patch_version(exe, "0.1.0-beta.4", "99999999.9.1")
        self.assertEqual(count, 2)
        self.assertEqual(len(patched), len(exe))
        self.assertIn(b"herdr-app 99999999.9.1", patched)
        self.assertIn(b"0.1.0-beta.5", patched)
        with self.assertRaises(ValueError):
            rs.patch_version(exe, "0.1.0-beta.4", "9.9.1")


class Server(unittest.TestCase):
    def test_serves_what_the_updater_reads(self):
        with tempfile.TemporaryDirectory() as tmp:
            log = os.path.join(tmp, "server.log")
            key = rs.TestKey()
            server = rs.FakeGithub(key, log).start()
            try:
                archive = b"archive bytes"
                server.add_release("99999999.9.1", archive)

                def get(path, ua="Yuzora/0.1.0-beta.4"):
                    request = urllib.request.Request(server.base + path, headers={"User-Agent": ua})
                    with urllib.request.urlopen(request) as response:
                        return response.status, response.read()

                status, body = get("/repos/%s/releases?per_page=100" % rs.REPO)
                listed = json.loads(body)
                self.assertEqual(status, 200)
                self.assertEqual(listed[0]["tag_name"], "v99999999.9.1")
                self.assertFalse(listed[0]["prerelease"])
                names = {a["name"] for a in listed[0]["assets"]}
                self.assertEqual(names, {"yuzora-update.json", "yuzora-update.json.sig", "yuzora-99999999.9.1-windows-x86_64.tar.gz"})
                urls = {a["name"]: a["browser_download_url"] for a in listed[0]["assets"]}

                _, manifest_bytes = get(urls["yuzora-update.json"][len(server.base):])
                _, signature = get(urls["yuzora-update.json.sig"][len(server.base):])
                self.assertTrue(rs.minisign_verify(key.public_base64(), manifest_bytes, signature.decode()))
                manifest = json.loads(manifest_bytes)
                self.assertIsNone(manifest_checks(manifest, "99999999.9.1", server.downloads()))
                self.assertEqual(manifest["targets"]["windows-x86_64"]["sha256"], hashlib.sha256(archive).hexdigest())
                _, tar = get(manifest["targets"]["windows-x86_64"]["url"][len(server.base):])
                self.assertEqual(tar, archive)
                self.assertEqual(json.loads(get("/repos/%s/releases?per_page=100&page=2" % rs.REPO)[1]), [])
                with self.assertRaises(urllib.error.HTTPError):
                    get("/nope")
                with open(log, encoding="utf-8") as f:
                    lines = [json.loads(line) for line in f]
                self.assertTrue(all(line["ua"] == "Yuzora/0.1.0-beta.4" for line in lines[:2]))
                self.assertEqual(lines[-1]["status"], 404)
            finally:
                server.stop()


class Reset(unittest.TestCase):
    def test_a_reset_server_offers_only_what_is_added_after_it(self):
        """The first run's stage failures: a chain began with the last chain's newest release still on offer."""
        with tempfile.TemporaryDirectory() as tmp:
            server = rs.FakeGithub(rs.TestKey(), os.path.join(tmp, "server.log")).start()
            try:
                def listed():
                    with urllib.request.urlopen(urllib.request.Request(server.base + "/repos/%s/releases?per_page=100" % rs.REPO, headers={"User-Agent": "t"})) as response:
                        return [r["tag_name"] for r in json.loads(response.read())]
                server.add_release("99999999.9.1", b"one")
                server.add_release("99999999.9.2", b"two")
                self.assertEqual(listed()[:2], ["v99999999.9.2", "v99999999.9.1"])
                server.reset()
                self.assertEqual(listed(), ["v0.0.19"])
                server.add_release("99999999.9.1", b"one")
                self.assertEqual(listed()[0], "v99999999.9.1")
                self.assertNotIn("v99999999.9.2", listed())
                with self.assertRaises(urllib.error.HTTPError):
                    urllib.request.urlopen(urllib.request.Request(server.base + "/%s/releases/download/v99999999.9.2/yuzora-update.json" % rs.REPO))
            finally:
                server.stop()


class Text(unittest.TestCase):
    def test_colour_codes_are_removed_from_app_log_lines(self):
        import common
        raw = "\x1b[2m2026-11-10T14:13:05Z\x1b[0m \x1b[33m WARN\x1b[0m \x1b[2mherdr_app::update\x1b[0m\x1b[2m:\x1b[0m update: cannot move X aside"
        self.assertEqual(common.strip_ansi(raw), "2026-11-10T14:13:05Z  WARN herdr_app::update: update: cannot move X aside")
        self.assertEqual(common.strip_ansi(""), "")


class Archive(unittest.TestCase):
    def test_archive_has_the_package_layout_with_the_new_exe(self):
        with tempfile.TemporaryDirectory() as tmp:
            src = os.path.join(tmp, "yuzora")
            os.mkdir(src)
            for name, data in {"yuzora.exe": b"old", "yuzora.png": b"png", "LICENSE": b"l", "yuzora.package": b"LICENSE\nyuzora.exe\nyuzora.package\nyuzora.png\n"}.items():
                with open(os.path.join(src, name), "wb") as f:
                    f.write(data)
            archive = os.path.join(tmp, "a.tar.gz")
            sha = rs.build_archive(src, archive, b"new exe")
            with open(archive, "rb") as f:
                self.assertEqual(hashlib.sha256(f.read()).hexdigest(), sha)
            with tarfile.open(archive) as tar:
                names = sorted(tar.getnames())
                self.assertEqual(names, ["yuzora", "yuzora/LICENSE", "yuzora/yuzora.exe", "yuzora/yuzora.package", "yuzora/yuzora.png"])
                self.assertEqual(tar.extractfile("yuzora/yuzora.exe").read(), b"new exe")
            self.assertEqual([n for n in os.listdir(tmp) if n.startswith(".exe-")], [])


class Round3(unittest.TestCase):
    """The pieces of round 3 that run anywhere: what a herdr-core process is called, the welcome that is left out,
    the verdict lines, and the exe of the second installer."""

    def test_a_core_process_is_told_by_its_command_line(self):
        import observe
        self.assertEqual(observe.role_of('"C:\\x\\herdr-core.exe" hold --socket \\\\.\\pipe\\h-1'), "hold")
        self.assertEqual(observe.role_of("C:\\x\\herdr-core.exe serve"), "serve")
        self.assertEqual(observe.role_of("herdr-core.exe connect"), "connect")
        self.assertEqual(observe.role_of(None), "other")
        self.assertEqual(observe.role_of("herdr-core.exe server stop"), "other")

    def test_rows_that_hold_the_folder_are_the_ones_inside_it(self):
        import observe
        rows = [{"pid": 1, "role": "serve", "kind": "inside-install", "cwd": "a", "gone": False},
                {"pid": 2, "role": "hold", "kind": "inside-old", "cwd": "b", "gone": False},
                {"pid": 3, "role": "hold", "kind": "other", "cwd": "c", "gone": False},
                {"pid": 4, "role": "hold", "kind": "inside-install", "cwd": "d", "gone": True},
                {"pid": 5, "role": "other", "kind": "unknown", "cwd": None, "gone": False}]
        self.assertEqual([r["pid"] for r in observe.pinned(rows)], [1, 2])
        self.assertIn("hold pid 4", observe.core_text(rows))
        self.assertEqual(observe.core_text([]), "none")

    def test_the_welcome_is_marked_finished_in_the_homes_shared_store(self):
        import observe
        with tempfile.TemporaryDirectory() as home:
            observe.skip_welcome(home)
            with open(os.path.join(home, "shared.json")) as f:
                self.assertEqual(json.load(f), {"frame.welcomed": "skipped"})

    def test_a_verdict_is_pass_when_the_sentence_holds_and_fail_when_it_does_not(self):
        import observe
        seen = []

        class Run:
            def result(self, rid, status, detail, evidence=()):
                seen.append((rid, status, detail))

        class Ctx:
            run = Run()

        observe.claim(Ctx, "C9", "a", True, "yes", "no")
        observe.claim(Ctx, "C9", "b", False, "yes", "no")
        self.assertEqual(seen, [("C9.a", "pass", "yes"), ("C9.b", "fail", "no")])

    def test_the_second_exe_says_the_first_newer_version(self):
        if os.name == "nt":
            self.skipTest("the stand-in exe is a shell script")
        import subprocess
        with tempfile.TemporaryDirectory() as tmp:
            built = os.path.join(tmp, "built")
            with open(built, "w") as f:
                f.write("#!/bin/sh\necho 'herdr-app 0.1.0-beta.4'\n# 0.1.0-beta.4\n")
            os.chmod(built, 0o755)
            out = os.path.join(tmp, "second")
            done = subprocess.run([sys.executable, os.path.join(HERE, "second_installer.py"), built, out], capture_output=True, text=True)
            self.assertEqual(done.returncode, 0, done.stderr)
            self.assertEqual(done.stdout.strip(), "99999999.9.1")
            self.assertEqual(subprocess.run([out, "--version"], capture_output=True, text=True).stdout.strip(), "herdr-app 99999999.9.1")


class Round3Chains(unittest.TestCase):
    """Each chain of round 3 is run against stand-ins for everything Windows (the app, the core, the installers,
    the mouse): not a measurement, a check that the chain's own steps, names and arguments fit together and that
    it ends with the lines it is meant to write."""

    class Shell:
        def __init__(self, out="", rc=0, data=None):
            self.rc, self.out, self.err, self.timed_out, self._data = rc, out, "", False, data

        def json(self):
            return self._data

    class Inst:
        def __init__(self, pid=11):
            self.pid, self.log_rel, self.log_path, self.proc = pid, "x/app.log", None, None
            self.alive_now, self.log = True, ""

        def alive(self):
            return self.alive_now

        def log_text(self):
            return self.log

        def stop(self):
            self.alive_now = False

        def wait_exit(self, timeout):
            return True

    def setup_chain(self, out, swaps, hold_in_install=True):
        import observe
        import common
        saved = dict(observe.__dict__)
        self.addCleanup(lambda: observe.__dict__.update(saved))
        inst = self.Inst()
        run = common.Run(out)
        calls = []
        rows = [{"name": "herdr-core.exe", "pid": 100, "ppid": 1}, {"name": "herdr-core.exe", "pid": 101, "ppid": 100}]

        class Probe:
            exe = "winprobe.exe"

            def ps(self):
                return rows

            def named(self, name):
                return [r for r in rows if r["name"] == name]

            def alive(self, pid):
                return True

            def procinfo(self, pid, evidence_name=None):
                inside = hold_in_install or pid == 100
                return {"pid": pid, "peb": {"cmdline": "herdr-core.exe serve" if pid == 100 else "herdr-core.exe hold --socket x", "cwd_final": "/fixture/Programs/Yuzora" if inside else "/fixture/home"}}

            def windows(self, pid):
                return [{"title": "Yuzora", "visible": True}]

            def click_restart(self, pid, evidence_name=None):
                calls.append("click_restart")
                return {"found": True, "clicked": {"x": 900, "y": 690}, "capsule": {"from": 860, "to": 920}, "pill_color": "#fff", "row": []}

            def click(self, pid, from_right, from_bottom, evidence_name=None):
                calls.append("click %s %s" % (from_right, from_bottom))
                return {"found": True, "clicked": {"x": 800, "y": 600}}

        ctx = observe.Ctx(run, Probe(), "testkey", out, os.path.join(out, "work"), 5000, dist2=out)
        ctx.v1, ctx.v2, ctx.v3, ctx.v4 = "0.1.0-beta.4", "99999999.9.1", "99999999.9.2", "99999999.9.3"
        ctx.core, ctx.cli, ctx.installer, ctx.installer2 = "core.exe", "cli.exe", "setup.exe", "setup2.exe"
        ctx.server = type("Server", (), {"base": "http://127.0.0.1:9"})
        os.makedirs(ctx.elsewhere, exist_ok=True)
        overlays = [["toast"]]
        stage = {"done": True}

        def sh(args, timeout, cwd=None, env=None, name=None):
            calls.append(" ".join(str(a) for a in args[1:3]))
            return self.Shell("")

        run.sh = sh
        ctx.fresh_server = lambda *v: None
        observe.install_fresh = lambda ctx, cid: None
        observe.ensure_core = lambda ctx: "core.exe"
        observe.ensure_cli = lambda ctx: None
        observe.start_old_core = lambda ctx, cid, home: {"serve": [100]}
        observe.INSTALL, observe.PROGRAMS = "/fixture/Programs/Yuzora", "/fixture/Programs"
        started = []

        def start_app(ctx, cid, label, home, cwd):
            started.append(self.Inst(20 + len(started)))
            return started[-1]

        observe.start_app = start_app
        observe.wait_main_window = lambda *a, **k: True
        observe.check_app_cwd = lambda *a, **k: None
        observe.stage_via_cli = lambda *a, **k: stage["done"]
        observe.run_cli = lambda ctx, cid, home, *words, **k: self.Shell("{}")
        observe.screenshot = lambda ctx, rel: rel
        observe.snap = lambda ctx, cid, label: {"siblings": {"Yuzora": "dir"}, "old_files": [], "new_version": None, "receipt": None}
        observe.folder_version = lambda ctx, folder: ctx.v2
        observe.package_list_state = lambda: {"extra": [], "missing": []}
        observe.status_overlays = lambda ctx, home: overlays[0]
        observe.blocked_lines = lambda text: ["update: cannot move X aside"]
        results = iter(swaps)
        observe.observe_swap = lambda *a, **k: next(results)
        return observe, ctx, run, inst, calls, overlays

    def lines(self, run):
        import json as _json
        with open(run.results_path) as f:
            return {r["id"]: r for r in map(_json.loads, f)}

    def test_the_click_chain_asks_for_a_toast_that_stays_and_a_click_that_opens_updates(self):
        with tempfile.TemporaryDirectory() as out:
            observe, ctx, run, inst, calls, overlays = self.setup_chain(out, [])
            seen = []

            def overlays_now(ctx, home):
                seen.append(1)
                return ["toast"] if len(seen) <= 3 else ["frame-settings"]

            observe.status_overlays = overlays_now
            observe.time = type("T", (), {"sleep": staticmethod(lambda s: None), "time": staticmethod(__import__("time").time)})
            observe.wait_for = lambda cond, timeout, interval=0.5: cond()
            observe.chain_mouse(ctx, True)
            got = self.lines(run)
            for rid in ("C8.pill_found", "C8.click_reached_restart", "C8.toast_shown", "C8.toast_stays", "C8.toast_click_opens_updates", "C8.reason_later", "C8.cli_text"):
                self.assertIn(rid, got, sorted(got))
            self.assertEqual([got[k]["status"] for k in ("C8.pill_found", "C8.click_reached_restart", "C8.toast_shown", "C8.toast_stays", "C8.toast_click_opens_updates")], ["pass"] * 5, got)
            self.assertIn("click_restart", calls)
            self.assertIn("click 186 94", calls)

    def test_the_holder_and_restart_chain_names_each_sentence_it_checks(self):
        with tempfile.TemporaryDirectory() as out:
            observe, ctx, run, inst, calls, overlays = self.setup_chain(out, [(False, None), (False, None)])
            observe.time = type("T", (), {"sleep": staticmethod(lambda s: None), "time": staticmethod(__import__("time").time)})
            observe.chain_holders(ctx, True)
            got = self.lines(run)
            for rid in ("C6.claim_holder_has_install_cwd", "C6.second_terminal", "C6.claim_swap_blocked_by_old_core", "C6.hc_server_restart", "C6.claim_restart_leaves_the_folder_held", "C6.claim_swap_after_restart_blocked"):
                self.assertIn(rid, got, sorted(got))
            self.assertEqual(got["C6.claim_holder_has_install_cwd"]["status"], "pass")
            self.assertEqual(got["C6.claim_swap_blocked_by_old_core"]["status"], "pass")

    def test_a_holder_that_is_not_in_the_folder_makes_the_sentence_fail(self):
        with tempfile.TemporaryDirectory() as out:
            observe, ctx, run, inst, calls, overlays = self.setup_chain(out, [(False, None), (False, None)], hold_in_install=False)
            observe.time = type("T", (), {"sleep": staticmethod(lambda s: None), "time": staticmethod(__import__("time").time)})
            observe.chain_holders(ctx, True)
            got = self.lines(run)
            self.assertEqual(got["C6.claim_holder_has_install_cwd"]["status"], "fail")

    def test_the_stop_chain_checks_the_folder_is_free_then_the_swap(self):
        with tempfile.TemporaryDirectory() as out:
            observe, ctx, run, inst, calls, overlays = self.setup_chain(out, [(True, None)], hold_in_install=False)
            observe.time = type("T", (), {"sleep": staticmethod(lambda s: None), "time": staticmethod(__import__("time").time)})
            observe.core_rows = lambda ctx, cid, tag: [] if tag == "after-stop" else [{"pid": 100, "role": "serve", "kind": "inside-install", "cwd": "x", "cmd": "serve", "gone": False}]
            observe.chain_server_stop(ctx, True)
            got = self.lines(run)
            self.assertEqual(got["C7.claim_stop_frees_the_folder"]["status"], "pass")
            self.assertEqual(got["C7.claim_swap_after_stop_works"]["status"], "pass")
            self.assertIn("C7.hc_server_stop", got)

    def test_the_installer_chains_say_whether_the_exe_changed_and_the_first_auto_update_went_through(self):
        with tempfile.TemporaryDirectory() as out:
            observe, ctx, run, inst, calls, overlays = self.setup_chain(out, [], hold_in_install=False)
            observe.time = type("T", (), {"sleep": staticmethod(lambda s: None), "time": staticmethod(__import__("time").time)})
            observe.chain_other_version(ctx, True)
            got = self.lines(run)
            self.assertEqual(got["C9.exe_replaced"]["status"], "pass")
            self.assertIn("C9.start_after_overlay", got)
        with tempfile.TemporaryDirectory() as out:
            observe, ctx, run, inst, calls, overlays = self.setup_chain(out, [(False, None), (True, None)], hold_in_install=True)
            observe.time = type("T", (), {"sleep": staticmethod(lambda s: None), "time": staticmethod(__import__("time").time)})
            observe.chain_beta_to_stable(ctx, True)
            got = self.lines(run)
            self.assertEqual(got["C10.claim_first_auto_update_blocked"]["status"], "pass")
            self.assertEqual(got["C10.claim_auto_update_after_stop_works"]["status"], "pass")
            self.assertIn("C10.installer", got)


if __name__ == "__main__":
    unittest.main(verbosity=2)
