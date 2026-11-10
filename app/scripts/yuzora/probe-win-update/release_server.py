"""A stand-in for GitHub's releases that Yuzora's updater (src/yuzora/release/update.rs)
accepts: a release list, signed manifests, archives. Also the pieces to make newer
builds out of the one that was built: a same-length version replacement in the exe, an
archive laid out like scripts/yuzora/package's, and a minisign signer (pure Python,
RFC 8032 Ed25519 + BLAKE2b prehash, the format src/yuzora/release/manifest.rs verifies).

Test key: only a debug-assertions build without HERDR_APP_RELEASE trusts it, through
YUZORA_UPDATE_TEST_KEY (update.rs `keys`).
"""
import base64
import hashlib
import json
import os
import tarfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

REPO = "NakiriYuuzu/Yuzora"
TARGET = "windows-x86_64"

# ---- Ed25519 (RFC 8032, section 6) ----------------------------------------

_P = 2**255 - 19
_Q = 2**252 + 27742317777372353535851937790883648493


def _inv(x):
    return pow(x, _P - 2, _P)


_D = -121665 * _inv(121666) % _P
_SQRT_M1 = pow(2, (_P - 1) // 4, _P)


def _add(a, b):
    A, B = (a[1] - a[0]) * (b[1] - b[0]) % _P, (a[1] + a[0]) * (b[1] + b[0]) % _P
    C, D = 2 * a[3] * b[3] * _D % _P, 2 * a[2] * b[2] % _P
    E, F, G, H = B - A, D - C, D + C, B + A
    return (E * F % _P, G * H % _P, F * G % _P, E * H % _P)


def _mul(s, point):
    q = (0, 1, 1, 0)
    while s > 0:
        if s & 1:
            q = _add(q, point)
        point = _add(point, point)
        s >>= 1
    return q


def _same(a, b):
    return (a[0] * b[2] - b[0] * a[2]) % _P == 0 and (a[1] * b[2] - b[1] * a[2]) % _P == 0


def _recover_x(y, sign):
    if y >= _P:
        return None
    x2 = (y * y - 1) * _inv(_D * y * y + 1)
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (_P + 3) // 8, _P)
    if (x * x - x2) % _P != 0:
        x = x * _SQRT_M1 % _P
    if (x * x - x2) % _P != 0:
        return None
    if (x & 1) != sign:
        x = _P - x
    return x


_GY = 4 * _inv(5) % _P
_GX = _recover_x(_GY, 0)
_G = (_GX, _GY, 1, _GX * _GY % _P)


def _compress(point):
    zinv = _inv(point[2])
    x, y = point[0] * zinv % _P, point[1] * zinv % _P
    return int.to_bytes(y | ((x & 1) << 255), 32, "little")


def _decompress(s):
    if len(s) != 32:
        return None
    y = int.from_bytes(s, "little")
    sign = y >> 255
    y &= (1 << 255) - 1
    x = _recover_x(y, sign)
    return None if x is None else (x, y, 1, x * y % _P)


def _h(s):
    return int.from_bytes(hashlib.sha512(s).digest(), "little") % _Q


def _expand(secret):
    h = hashlib.sha512(secret).digest()
    a = int.from_bytes(h[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    return a, h[32:]


def ed_public(secret):
    return _compress(_mul(_expand(secret)[0], _G))


def ed_sign(secret, msg):
    a, prefix = _expand(secret)
    public = _compress(_mul(a, _G))
    r = _h(prefix + msg)
    rs = _compress(_mul(r, _G))
    s = (r + _h(rs + public + msg) * a) % _Q
    return rs + int.to_bytes(s, 32, "little")


def ed_verify(public, msg, signature):
    a = _decompress(public)
    r = _decompress(signature[:32])
    if a is None or r is None or len(signature) != 64:
        return False
    s = int.from_bytes(signature[32:], "little")
    if s >= _Q:
        return False
    h = _h(signature[:32] + public + msg)
    return _same(_mul(s, _G), _add(r, _mul(h, a)))


# ---- minisign (what minisign-verify 0.3 reads) ------------------------------

def prehash(message):
    return hashlib.blake2b(message, digest_size=64).digest()


class TestKey:
    """A throwaway signing key for one run."""

    def __init__(self, secret=None, key_id=None):
        self.secret = secret or os.urandom(32)
        self.key_id = key_id or os.urandom(8)
        self.public = ed_public(self.secret)

    def public_base64(self):
        """The key line: what YUZORA_UPDATE_TEST_KEY and TRUSTED_KEYS hold."""
        return base64.b64encode(b"Ed" + self.key_id + self.public).decode()

    def sign(self, message, trusted_comment="timestamp:0\tfile:probe\tprehashed"):
        signature = ed_sign(self.secret, prehash(message))
        global_signature = ed_sign(self.secret, signature + trusted_comment.encode())
        return (
            "untrusted comment: signature from the update probe\n"
            + base64.b64encode(b"ED" + self.key_id + signature).decode()
            + "\ntrusted comment: " + trusted_comment + "\n"
            + base64.b64encode(global_signature).decode() + "\n"
        )


def minisign_verify(public_b64, message, signature_text):
    """minisign-verify's PublicKey::verify(.., allow_legacy=false), plus Tauri's base64 wrapper."""
    text = signature_text.strip()
    if not text.startswith("untrusted comment:"):
        text = base64.b64decode(text).decode()
    lines = text.splitlines()
    if len(lines) < 4 or not lines[2].startswith("trusted comment: "):
        return False
    bin1, bin2 = base64.b64decode(lines[1]), base64.b64decode(lines[3])
    key = base64.b64decode(public_b64)
    if len(bin1) != 74 or len(bin2) != 64 or len(key) != 42 or bin1[:2] != b"ED" or key[2:10] != bin1[2:10]:
        return False
    signature = bin1[10:]
    trusted = lines[2][len("trusted comment: "):].encode()
    return ed_verify(key[10:], prehash(message), signature) and ed_verify(key[10:], signature + trusted, bin2)


# ---- newer builds out of the one that was built -----------------------------

def newer_versions(current, count=2):
    """Versions of the same length as `current`, stable, and later than it, so a copy of
    the exe can say it is one of them by replacing the bytes in place."""
    n = len(current)
    if not 5 <= n <= 13:
        raise ValueError("cannot make a same-length stable version for %r" % current)
    return ["%s.9.%d" % ("9" * (n - 4), k) for k in range(1, count + 1)]


def patch_version(exe_bytes, current, new):
    """The exe with every `current` replaced by `new` (same length). Returns (bytes, replacements)."""
    if len(current) != len(new):
        raise ValueError("versions must have the same length")
    count = exe_bytes.count(current.encode())
    return exe_bytes.replace(current.encode(), new.encode()), count


def build_archive(unpacked_dir, archive_path, exe_bytes):
    """yuzora-<version>-windows-x86_64.tar.gz: a `yuzora/` folder like unpacked_dir, with
    this exe in place of its yuzora.exe. Returns the archive's sha256."""
    with tarfile.open(archive_path, "w:gz") as tar:
        tar.add(unpacked_dir, arcname="yuzora", recursive=False)
        for name in sorted(os.listdir(unpacked_dir)):
            path = os.path.join(unpacked_dir, name)
            if name == "yuzora.exe":
                patched = os.path.join(os.path.dirname(archive_path), ".exe-" + os.path.basename(archive_path))
                with open(patched, "wb") as f:
                    f.write(exe_bytes)
                tar.add(patched, arcname="yuzora/yuzora.exe")
                os.remove(patched)
            else:
                tar.add(path, arcname="yuzora/" + name)
    with open(archive_path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


# ---- the server ---------------------------------------------------------------

def asset_path(version, name):
    return "/%s/releases/download/v%s/%s" % (REPO, version, name)


class FakeGithub:
    """http://127.0.0.1:<port> laid out like github.com plus api.github.com for this repo."""

    def __init__(self, key, log_path):
        self.key = key
        self.log_path = log_path
        self.releases = []  # newest first: (version, {asset name: bytes})
        self.lock = threading.Lock()
        outer = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args):
                pass

            def do_GET(self):
                status, body, kind = outer.answer(self.path)
                outer.record(self, status, len(body))
                self.send_response(status)
                self.send_header("Content-Type", kind)
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Connection", "close")
                self.end_headers()
                try:
                    self.wfile.write(body)
                except OSError:
                    pass

        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.base = "http://127.0.0.1:%d" % self.httpd.server_address[1]
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)

    def start(self):
        self.thread.start()
        return self

    def stop(self):
        self.httpd.shutdown()
        self.httpd.server_close()

    def downloads(self):
        return "%s/%s/releases/download" % (self.base, REPO)

    def record(self, handler, status, size):
        line = json.dumps({"t": round(time.time(), 3), "path": handler.path, "ua": handler.headers.get("User-Agent", ""), "status": status, "bytes": size})
        with self.lock, open(self.log_path, "a", encoding="utf-8") as f:
            f.write(line + "\n")

    def reset(self):
        """No release at all (the Tauri app's v0.0.19 entry stays, it is always listed). A chain starts
        from here, so what an earlier chain offered cannot be served first."""
        with self.lock:
            self.releases.clear()

    def add_release(self, version, archive_bytes, archive_sha256=None, sign=True):
        """A stable release `version` with a signed manifest and its archive, newest first."""
        name = "yuzora-%s-%s.tar.gz" % (version, TARGET)
        sha = archive_sha256 or hashlib.sha256(archive_bytes).hexdigest()
        manifest = json.dumps(
            {
                "schema": 1,
                "name": "yuzora",
                "version": version,
                "channel": "beta" if "-" in version else "stable",
                "pub_date": "2026-11-10T00:00:00Z",
                "targets": {TARGET: {"url": self.downloads() + "/v%s/%s" % (version, name), "sha256": sha, "size": len(archive_bytes)}},
            },
            indent=2,
        ).encode() + b"\n"
        assets = {"yuzora-update.json": manifest, name: archive_bytes}
        if sign:
            assets["yuzora-update.json.sig"] = self.key.sign(manifest).encode()
        with self.lock:
            self.releases.insert(0, (version, assets))

    def list_json(self):
        out = []
        for version, assets in self.releases:
            out.append(
                {
                    "tag_name": "v" + version,
                    "draft": False,
                    "prerelease": "-" in version,
                    "assets": [{"name": n, "browser_download_url": self.base + asset_path(version, n)} for n in assets],
                }
            )
        # The Tauri app's last stable release: carries latest.json, no Yuzora manifest.
        out.append({"tag_name": "v0.0.19", "draft": False, "prerelease": False, "assets": [{"name": "latest.json", "browser_download_url": self.base + asset_path("0.0.19", "latest.json")}]})
        return json.dumps(out).encode()

    def answer(self, path):
        route = path.split("?")[0]
        if route == "/repos/%s/releases" % REPO:
            with self.lock:
                return 200, (b"[]" if "page=2" in path else self.list_json()), "application/json"
        prefix = "/%s/releases/download/v" % REPO
        if route.startswith(prefix):
            version, _, name = route[len(prefix):].partition("/")
            with self.lock:
                for v, assets in self.releases:
                    if v == version and name in assets:
                        return 200, assets[name], "application/octet-stream"
        return 404, b"Not Found", "text/plain"
