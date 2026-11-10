#!/bin/sh
# What one attach of the Git sidebar costs: open, status, messages (crates/yuzora-git, lib.rs perf_attach_to_a_repository).
# Usage: run-attach-bench.sh <yuzora-git test binary> <repo> [runs]
#   the binary: cd app && cargo test --locked -p yuzora-git --lib --no-run   (prints .../deps/yuzora_git-<hash>)
# Two passes: with a `git` stand-in on PATH (counts the processes; each call costs ~15 ms more for the stand-in's
# own two perl starts), and with the real git (the milliseconds). HOME is a temporary one: nothing of the user's is read.
set -e
BIN=$1; REPO=$2; RUNS=${3:-5}
OUT=${OUT:-$(mktemp -d /tmp/yz-git-attach.XXXXXX)}; mkdir -p "$OUT/shim" "$OUT/home"
HERE=$(cd "$(dirname "$0")" && pwd)
GIT_REAL=$(command -v git)
cp "$HERE/git-shim" "$OUT/shim/git"; chmod +x "$OUT/shim/git"
: > "$OUT/timeline.log"
echo "# with the stand-in (processes counted; ms include its overhead)"
env -i HOME="$OUT/home" PATH="$OUT/shim:/usr/bin:/bin:/usr/sbin" GIT_REAL="$GIT_REAL" OUT="$OUT" YUZORA_GIT_TIMELINE="$OUT/timeline.log" \
  YUZORA_GIT_PERF_REPO="$REPO" YUZORA_GIT_PERF_RUNS="$RUNS" "$BIN" --ignored --nocapture perf_attach | grep '^attach'
echo "# real git (ms)"
env -i HOME="$OUT/home" PATH="/usr/bin:/bin:/usr/sbin" YUZORA_GIT_PERF_REPO="$REPO" YUZORA_GIT_PERF_RUNS="$RUNS" "$BIN" --ignored --nocapture perf_attach | grep '^attach'
