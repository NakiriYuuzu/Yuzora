#!/bin/sh
# Isolated fake only. No command invokes a real Herdr binary or a real socket.
set -eu
base=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
printf '%s\n' "$*" >> "$base/calls"
env | grep -i '^HERDR_' >> "$base/environment" || :
case "$*" in
  'session list --json') cat "$base/sessions.json"; exit 0 ;;
  'status --json') cat "$base/status.json"; exit 0 ;;
  'api schema --json') cat "$base/schema.json"; exit 0 ;;
esac
if [ "$1" = terminal ] && [ "$2" = session ] && { [ "$3" = observe ] || [ "$3" = control ]; }; then
  printf '%s\n' "$$" >> "$base/children"
  # Host passes target --cols COLS --rows ROWS; reflect fixed requested size.
  printf '{"type":"terminal.frame","seq":1,"full":true,"encoding":"ansi","width":%s,"height":%s,"bytes":"ZmFrZSBmcmFtZQ=="}\n' "$6" "$8"
  while IFS= read -r line; do printf '%s\n' "$line" >> "$base/input"; done
  exit 0
fi
if [ "$*" = '--session work client' ]; then
  printf '%s\n' "$$" >> "$base/children"
  stty -echo
  printf 'fake full frame\n'
  while IFS= read -r line; do printf '%s\n' "$line" >> "$base/full-input"; done
  exit 0
fi
printf 'fake rejected unexpected command\n' >&2
exit 2
