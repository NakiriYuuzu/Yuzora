"""Makes the exe for a second installer: the built exe with its version text replaced by the first newer
version of the same length (what the stand-in server calls v2). The workflow packs it with
`YUZORA_PACKAGE_BIN=<this exe> YUZORA_DIST=<folder> scripts/yuzora/package`, so the installer it gets is the real
one around an exe of another version: what the overlay chains (C9, C10) install over the first.

usage: python second_installer.py <built herdr-app.exe> <out exe>
Prints the version the new exe says.
"""
import shutil
import subprocess
import sys

import release_server as rs


def main(argv):
    if len(argv) != 2:
        sys.exit(__doc__)
    built, out = argv
    said = subprocess.run([built, "--version"], capture_output=True, text=True, timeout=60).stdout.split()
    if len(said) != 2 or said[0] != "herdr-app":
        sys.exit("the built exe does not answer --version: %r" % said)
    current = said[1]
    new = rs.newer_versions(current, 3)[0]
    with open(built, "rb") as f:
        patched, count = rs.patch_version(f.read(), current, new)
    if count == 0:
        sys.exit("the version %s is not in the exe" % current)
    with open(out, "wb") as f:
        f.write(patched)
    shutil.copymode(built, out)
    again = subprocess.run([out, "--version"], capture_output=True, text=True, timeout=60).stdout.strip()
    if again != "herdr-app " + new:
        sys.exit("the patched exe says %r, not 'herdr-app %s'" % (again, new))
    print(new)
    print("%d copies of %s replaced" % (count, current), file=sys.stderr)


if __name__ == "__main__":
    main(sys.argv[1:])
