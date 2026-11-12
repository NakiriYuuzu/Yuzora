#!/bin/sh
# A disposable clone to time Git on (crates/yuzora-git, perf.rs perf_every_operation), dirty the way a person's repo is.
# Usage: make-perf-repo.sh <source repo> <dest folder under /tmp>
#   <dest>/origin.git   a bare clone of the source's current branch: the clone's only remote, so no fetch or push leaves /tmp
#   <dest>/work         the clone the harness opens: 40 modified, 10 staged, 1 deleted, 1 renamed, 27 untracked files,
#                       two side branches and one that conflicts, a tag, three stashes, a linked worktree,
#                       one commit ahead of origin
# The source is only read (`git clone` over file://). The harness refuses a repo without the marker this leaves.
set -eu
SRC=$(cd "$1" && pwd -P); DEST=$2
case "$(mkdir -p "$DEST" && cd "$DEST" && pwd -P)" in /tmp/*|/private/tmp/*) ;; *) echo "the clone goes under /tmp, not $DEST" >&2; exit 2;; esac
DEST=$(cd "$DEST" && pwd -P)
[ ! -e "$DEST/work" ] || { echo "$DEST/work exists: remove it first" >&2; exit 2; }
# Nothing of the person's config, hooks or credentials takes part.
export HOME="$DEST/home" GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0
mkdir -p "$HOME"
BRANCH=$(git -C "$SRC" symbolic-ref --short HEAD)
git clone -q --bare --single-branch --branch "$BRANCH" "file://$SRC" "$DEST/origin.git"
git clone -q "$DEST/origin.git" "$DEST/work"
cd "$DEST/work"
git config user.name "Yuzora Perf"; git config user.email perf@yuzora.invalid
git config commit.gpgSign false; git config core.autocrlf false; git config gc.auto 0
: > .git/yuzora-perf-clone

# Text files big enough to have something to diff and blame, in a fixed order.
git ls-files -z | xargs -0 wc -l 2>/dev/null | awk '$1 >= 40 && $1 <= 4000 && $2 != "total" { $1=""; sub(/^ /, ""); print }' | grep -v '[^ -~]' | LC_ALL=C sort > .git/yuzora-perf-files
[ "$(wc -l < .git/yuzora-perf-files)" -ge 60 ] || { echo "fewer than 60 text files to change in $SRC" >&2; exit 2; }
pick() { sed -n "$1,$2p" .git/yuzora-perf-files; }
change() { while IFS= read -r f; do printf '\n// yuzora perf %s\n' "$1" >> "$f"; sed -i '' -e '20s/$/ yuzora-perf/' "$f"; done; }

git branch yz-perf/old HEAD~500 2>/dev/null || git branch yz-perf/old HEAD~20
git checkout -q -b yz-perf/feature HEAD~50 2>/dev/null || git checkout -q -b yz-perf/feature HEAD~5
pick 59 59 | change feature; git commit -q -am "perf: a commit on the side branch"
git checkout -q "$BRANCH"
git tag yz-perf-tag HEAD~10
for n in 1 2 3; do pick 58 58 | change "stash $n"; git stash push -q -m "perf stash $n"; done
pick 57 57 | change ahead; git commit -q -am "perf: one commit ahead of origin"
# A branch that conflicts with that commit (the same line, changed another way).
git checkout -q -b yz-perf/conflict HEAD~1
pick 57 57 | while IFS= read -r f; do sed -i '' -e '20s/$/ yuzora-perf-theirs/' "$f"; done; git commit -q -am "perf: a commit that conflicts"
git checkout -q "$BRANCH"
git worktree add -q --detach "$DEST/worktree" HEAD~1

pick 1 40 | change modified
pick 41 50 | change staged; pick 41 50 | tr '\n' '\0' | xargs -0 git add --
git rm -q "$(pick 51 51)"
git mv "$(pick 52 52)" "$(pick 52 52).yz-perf-renamed"
# Untracked files go beside tracked ones: a repo may ignore everything new at its top (Homebrew's does).
DIR=$(dirname "$(pick 1 1)")
mkdir -p "$DIR/yz-perf-untracked"
for n in $(seq 1 25); do seq 1 200 > "$DIR/yz-perf-untracked/file-$n.txt"; done
seq 1 50 > "$DIR/yz-perf-top-1.txt"; seq 1 50 > "$DIR/yz-perf-top-2.txt"
[ "$(git ls-files --others --exclude-standard | wc -l | tr -d ' ')" -eq 27 ] || { echo "the untracked files are ignored in $DIR" >&2; exit 2; }
printf '%s\n' "$DIR" > .git/yuzora-perf-dir
git status --short | awk '{ print substr($0, 1, 2) }' | sort | uniq -c
echo "$DEST/work: $(git rev-list --count HEAD) commits, $(git ls-files | wc -l | tr -d ' ') files, branch $BRANCH"
