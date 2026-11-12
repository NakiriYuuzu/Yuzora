#!/bin/sh
# What every yuzora:git call costs (crates/yuzora-git, perf.rs perf_every_operation), on a clone made by make-perf-repo.sh.
# Usage: run-ops-bench.sh <clone>/work <out folder> <label>=<yuzora-git test binary> [<label>=<binary> …]
#   the binary: cd app && cargo test --locked -p yuzora-git --lib --no-run --config 'profile.dev.package.yuzora-git.opt-level=2'
#               (prints …/deps/yuzora_git-<hash>; copy it out of a shared target folder before timing it)
#   YUZORA_GIT_PERF_ALONE=1 also times each read command by itself (in <label>-<round>.tsv.calls, beside what it took
#   inside the call); YUZORA_GIT_PERF_ONLY=<part>,… runs some parts only (attach, outside_edit, reads, diffs, log,
#   index_and_commit, refs_and_remote, history, trust).
#   ROUNDS (5) rounds, each running every binary once in turn, so all of them meet the same noise; RUNS (1) passes a round.
# Prints one table of all samples per label (median, range, runs, git processes, errors). HOME is a temporary one,
# and PATH is what an app started from the Finder has.
set -eu
REPO=$1; OUT=$2; shift 2
ROUNDS=${ROUNDS:-5}; RUNS=${RUNS:-1}
mkdir -p "$OUT/home"
SAMPLES=""
round=1
while [ "$round" -le "$ROUNDS" ]; do
  for side in "$@"; do
    label=${side%%=*}; bin=${side#*=}
    file="$OUT/$label-$round.tsv"
    env -i HOME="$OUT/home" PATH="/usr/bin:/bin:/usr/sbin:/sbin" TMPDIR="${TMPDIR:-/tmp}" ${YUZORA_GIT_PERF_ALONE:+YUZORA_GIT_PERF_ALONE=1} ${YUZORA_GIT_PERF_ONLY:+YUZORA_GIT_PERF_ONLY="$YUZORA_GIT_PERF_ONLY"} \
      YUZORA_GIT_PERF_REPO="$REPO" YUZORA_GIT_PERF_RUNS="$RUNS" YUZORA_GIT_PERF_OUT="$file" \
      "$bin" --ignored --nocapture --exact perf::perf_every_operation > "$OUT/$label-$round.log" 2>&1 || { echo "$label round $round failed: $OUT/$label-$round.log" >&2; tail -5 "$OUT/$label-$round.log" >&2; exit 1; }
    SAMPLES="$SAMPLES${SAMPLES:+,}$label=$file"
    echo "round $round $label: $(uptime | sed 's/.*load averages: //')" >&2
  done
  round=$((round + 1))
done
first=${1#*=}
YUZORA_GIT_PERF_SAMPLES="$SAMPLES" "$first" --ignored --nocapture --exact perf::perf_table | grep '^|' | tee "$OUT/table.md"
