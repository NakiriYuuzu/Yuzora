"""Isolated, bounded CLI/PTY probe. Run only after checking preflight.json.
All Herdr subprocesses use isolated-herdr.sh; every command rechecks routing.
"""
import base64
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import termios
import time

ROOT = Path(__file__).resolve().parent
WRAPPER = str(ROOT / "isolated-herdr.sh")
FIX = ROOT / os.environ.get("YZC_SPIKE_FIX", "fixtures")
T = Path("/tmp/yzc-spike")
SESSION = ["--session", "companion-spike"]
children = []


def preflight():
    p = subprocess.run([WRAPPER, "session", "list", "--json"], capture_output=True, text=True, timeout=10, check=True)
    data = json.loads(p.stdout)
    for s in data["sessions"]:
        if not s["name"] in ("default", "companion-spike"):
            raise SystemExit(s)
        for field in ("session_dir", "socket_path"):
            if not os.path.realpath(s[field]).startswith(os.path.realpath(T) + os.sep):
                raise SystemExit(s)
        if s["name"] == "default" and s["running"]:
            raise SystemExit(s)
    with (FIX / "routing-checks.ndjson").open("a") as f:
        f.write(p.stdout.strip() + "\n")
    return data


def run(label, args):
    preflight()
    p = subprocess.run([WRAPPER, *args], capture_output=True, text=True, timeout=15)
    (FIX / (label + ".txt")).write_text("$ isolated-herdr.sh " + " ".join(args) + f"\nexit={p.returncode}\n" + p.stdout + p.stderr)
    return p


def spawn(label, args, **kwargs):
    preflight()
    out = (FIX / (label + ".ndjson")).open("wb")
    err = (FIX / (label + ".stderr.txt")).open("wb")
    p = subprocess.Popen([WRAPPER, *args], stdin=subprocess.PIPE, stdout=out, stderr=err, **kwargs)
    children.append(p)
    out.close()
    err.close()
    return p


def send(p, value):
    preflight()
    p.stdin.write((json.dumps(value) + "\n").encode())
    p.stdin.flush()
    time.sleep(0.5)


def stop_child(p):
    if p.poll() is None:
        p.terminate()
        try:
            p.wait(timeout=3)
        except subprocess.TimeoutExpired:
            p.kill()
            p.wait(timeout=3)


if __name__ == "__main__":
    raise SystemExit("Import probe helpers from the documented experiment; no implicit server start")
