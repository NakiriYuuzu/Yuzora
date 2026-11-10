"""Runs app/scripts/yuzora/check-win-update.py (the check app-package.yml runs) on a build and says whether it
ended the way this job expects, as probe results (results.jsonl, the same format as the other jobs).

  --expect red    a build from before the fix: the check must end with exit 1 at its control step (the
                  folder cannot be renamed while the app started in it runs). Exit 0 would mean a check that
                  cannot fail; exit 2 means it could not run, which says nothing about the fix.
  --expect green  a build with the fix: exit 0. The replaced and left-alone copies of the version text it
                  printed are recorded.

usage: python run_check.py --check <check-win-update.py> --setup <setup.exe> --expect red|green --name <id> --out <dir>
"""
import argparse
import re
import sys

import common
from common import FAIL, INFO, PASS, PROBE_ERROR, Run

RENAME_REFUSED = "the install folder cannot be renamed while the app started in its install folder runs"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", required=True)
    parser.add_argument("--setup", required=True)
    parser.add_argument("--expect", choices=["red", "green"], required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()
    run = Run(args.out)
    name = args.name
    selftest = run.sh([sys.executable, "-I", args.check, "--self-test"], 120, name="%s/self-test.txt" % name)
    run.result("%s.self_test" % name, PASS if selftest.rc == 0 else FAIL, "check-win-update.py --self-test rc=%s: %s" % (selftest.rc, (selftest.out + selftest.err).strip().splitlines()[-1:] or ""), ["%s/self-test.txt" % name])
    shell = run.sh([sys.executable, "-u", args.check, args.setup], 1200, name="%s/check-output.txt" % name)
    text = common.strip_ansi(shell.out + shell.err)
    last = " | ".join(line.strip() for line in text.strip().splitlines()[-6:])
    ev = ["%s/check-output.txt" % name]
    if shell.timed_out:
        run.result("%s.check_ran" % name, PROBE_ERROR, "the check did not end within 20 minutes", ev)
        return
    if args.expect == "red":
        if shell.rc == 1 and RENAME_REFUSED in text:
            run.result("%s.red_baseline" % name, PASS, "on the build from before the fix the check ended with exit 1 at its control step, as it must: %s" % last, ev)
        elif shell.rc == 0:
            run.result("%s.red_baseline" % name, FAIL, "the check PASSED on the build from before the fix: it cannot fail, so a pass on the fixed build means nothing. %s" % last, ev)
        else:
            run.result("%s.red_baseline" % name, FAIL, "the check ended with exit %s on the build from before the fix, not exit 1 at its control step: %s" % (shell.rc, last), ev)
    else:
        verdict = PASS if shell.rc == 0 else FAIL
        run.result("%s.check_passes" % name, verdict, "check-win-update.py exit %s: %s" % (shell.rc, last), ev)
    staged = re.findall(r"staged (\S+): (\d+) copies of (\S+) in the exe replaced, (\d+) inside longer versions left alone", text)
    run.result("%s.version_copies" % name, INFO, "; ".join("%s: %s copies of %s replaced, %s left alone" % (v, n, c, left) for v, n, c, left in staged) or "no 'staged' line in the output (the check ended before it patched an exe)", ev)
    installed = re.search(r"installed (\S+); updates will be", text)
    run.result("%s.installed_version" % name, INFO, "the installed build says %s" % (installed.group(1) if installed else "(not reached)"), ev)


if __name__ == "__main__":
    main()
