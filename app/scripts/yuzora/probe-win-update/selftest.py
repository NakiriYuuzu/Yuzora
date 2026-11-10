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


if __name__ == "__main__":
    unittest.main(verbosity=2)
