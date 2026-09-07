#!/bin/sh
# Run inside the selected WSL distribution. Preview by default; --apply removes
# only exact released Yuzora adapter files. User hooks and official integrations remain.
set -eu
apply=false
if [ "${1:-}" = "--apply" ]; then apply=true; shift; fi
[ "$#" -eq 1 ] || { printf 'usage: cleanup-wsl-adapter.sh [--apply] PI_AGENT_DIRECTORY\n' >&2; exit 2; }
ext_dir="$1/extensions"
checksum() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}
clean() {
  leaf=$1 expected=$2
  path="$ext_dir/$leaf"
  if [ ! -e "$path" ] && [ ! -L "$path" ]; then printf 'absent: %s\n' "$leaf"; return; fi
  if [ -L "$path" ] || [ ! -f "$path" ] || [ "$(checksum "$path")" != "$expected" ]; then
    printf 'preserved (modified or not owned): %s\n' "$leaf"; return
  fi
  if "$apply"; then rm -- "$path"; printf 'removed: %s\n' "$leaf"
  else printf 'would remove: %s\n' "$leaf"; fi
}
clean yuzora-herdr-wsl.ts 10906cfb9987261d30e23e6af6fd615bbcab599d9f7089fa12f64fd8310a0167
clean yuzora-herdr-wsl-report d4fcb1d7edb84392aa91b04006ae57006edb20acfb79c0cb7d21915e7f68301b
# Remove the receipt only if both owned payloads have gone. Never evaluate it.
marker="$ext_dir/yuzora-herdr-wsl.marker"
if [ ! -e "$ext_dir/yuzora-herdr-wsl.ts" ] && [ ! -L "$ext_dir/yuzora-herdr-wsl.ts" ] &&
   [ ! -e "$ext_dir/yuzora-herdr-wsl-report" ] && [ ! -L "$ext_dir/yuzora-herdr-wsl-report" ] &&
   [ -f "$marker" ] && [ ! -L "$marker" ]; then
  expected=$(printf 'YUZORA_WSL_ADAPTER=pi\nYUZORA_WSL_ADAPTER_VERSION=0.1.0\nYUZORA_WSL_ADAPTER_TS_SHA256=10906cfb9987261d30e23e6af6fd615bbcab599d9f7089fa12f64fd8310a0167\nYUZORA_WSL_ADAPTER_REPORT_SHA256=d4fcb1d7edb84392aa91b04006ae57006edb20acfb79c0cb7d21915e7f68301b\n')
  if [ "$(cat "$marker")" = "$expected" ]; then
    if "$apply"; then rm -- "$marker"; printf 'removed: adapter receipt\n'
    else printf 'would remove: adapter receipt\n'; fi
  else printf 'preserved (modified): adapter receipt\n'; fi
fi
