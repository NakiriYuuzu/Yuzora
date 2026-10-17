#!/bin/sh
# Spike only: never inherit the enclosing live Herdr pane's routing or HOME.
set -eu
T=/tmp/yzc-spike
umask 077
if [ -L "$T" ]; then
  echo "Refusing symlink spike root: $T" >&2
  exit 1
fi
if [ ! -e "$T" ]; then mkdir "$T"; fi
if [ ! -d "$T" ] || [ ! -O "$T" ]; then
  echo "Spike root must be a directory owned by the current user: $T" >&2
  exit 1
fi
for v in $(env | sed -n 's/^\(HERDR_[A-Za-z0-9_]*\)=.*/\1/p'); do unset "$v"; done
if [ "${1:-}" = --spike-env ]; then
  env | grep '^HERDR_' || true
  exit 0
fi
mkdir -p "$T/home" "$T/config" "$T/state" "$T/data" "$T/run" "$T/cache"
exec env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_SESSION \
  -u HERDR_PANE_ID -u HERDR_TAB_ID -u HERDR_WORKSPACE_ID -u HERDR_ENV \
  -u HERDR_BIN_PATH HOME="$T/home" XDG_CONFIG_HOME="$T/config" \
  XDG_STATE_HOME="$T/state" XDG_DATA_HOME="$T/data" XDG_RUNTIME_DIR="$T/run" XDG_CACHE_HOME="$T/cache" \
  "${YZC_SPIKE_BIN:-/Users/yuuzu/.local/bin/herdr}" "$@"
