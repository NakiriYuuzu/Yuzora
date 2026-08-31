#!/bin/sh
# In-distro POSIX installer for the Yuzora Pi WSL adapter.
# Windows orchestration must call: wsl.exe -d <distro> -- sh install.sh ...
# This script never edits files through UNC / 9p from Windows.
set -eu

VERSION="0.1.0"
ADAPTER_TS_NAME="yuzora-herdr-wsl.ts"
ADAPTER_REPORT_NAME="yuzora-herdr-wsl-report"
ADAPTER_MARKER_NAME="yuzora-herdr-wsl.marker"
OFFICIAL_NAME="herdr-agent-state.ts"

action=""
source_root=""
home_dir="${HOME:-}"
pi_dir=""

usage() {
  printf 'usage: install.sh install|status|uninstall --source-root DIR [--home DIR] [--pi-dir DIR]\n' >&2
  exit 2
}

while [ $# -gt 0 ]; do
  case "$1" in
    install|status|uninstall)
      action=$1
      shift
      ;;
    --source-root)
      source_root=$2
      shift 2
      ;;
    --home)
      home_dir=$2
      shift 2
      ;;
    --pi-dir)
      pi_dir=$2
      shift 2
      ;;
    *)
      usage
      ;;
  esac
done

[ -n "$action" ] || usage

if [ -z "$pi_dir" ]; then
  if [ -n "${PI_CODING_AGENT_DIR:-}" ]; then
    pi_dir=$PI_CODING_AGENT_DIR
  else
    pi_dir="$home_dir/.pi/agent"
  fi
fi

ext_dir="$pi_dir/extensions"
ts_src="$source_root/adapters/pi/$ADAPTER_TS_NAME"
report_src="$source_root/adapters/common/herdr-wsl-report"
ts_dest="$ext_dir/$ADAPTER_TS_NAME"
report_dest="$ext_dir/$ADAPTER_REPORT_NAME"
marker_dest="$ext_dir/$ADAPTER_MARKER_NAME"

checksum() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
    return
  fi
  shasum -a 256 "$1" | cut -d' ' -f1
}

atomic_copy() {
  src=$1
  dest=$2
  mode=$3
  tmp="$dest.tmp.$$"
  cp "$src" "$tmp"
  chmod "$mode" "$tmp"
  mv "$tmp" "$dest"
}

write_marker() {
  ts_hash=$(checksum "$ts_dest")
  report_hash=$(checksum "$report_dest")
  tmp="$marker_dest.tmp.$$"
  printf 'YUZORA_WSL_ADAPTER=pi\nYUZORA_WSL_ADAPTER_VERSION=%s\nYUZORA_WSL_ADAPTER_TS_SHA256=%s\nYUZORA_WSL_ADAPTER_REPORT_SHA256=%s\n' \
    "$VERSION" "$ts_hash" "$report_hash" >"$tmp"
  chmod 644 "$tmp"
  mv "$tmp" "$marker_dest"
}

owned_sentinel() {
  path=$1
  case "${path##*/}" in
    "$ADAPTER_TS_NAME") printf '%s\n' '// YUZORA_WSL_ADAPTER=pi' ;;
    "$ADAPTER_REPORT_NAME") printf '%s\n' '# YUZORA_WSL_ADAPTER=pi' ;;
    "$ADAPTER_MARKER_NAME") printf '%s\n' 'YUZORA_WSL_ADAPTER=pi' ;;
    *) return 1 ;;
  esac
}

path_exists() {
  path=$1
  [ -e "$path" ] || [ -L "$path" ]
}

owned_file() {
  path=$1
  path_exists "$path" || return 1
  [ -L "$path" ] && return 1
  [ -f "$path" ] || return 1
  sentinel=$(owned_sentinel "$path") || return 1
  grep -Fxq -- "$sentinel" "$path"
}

have_seq_lock_backend() {
  command -v python3 >/dev/null 2>&1 && return 0
  command -v flock >/dev/null 2>&1 && return 0
  return 1
}

require_seq_lock_backend() {
  if have_seq_lock_backend; then
    return 0
  fi
  printf 'install.sh: missing prerequisite: python3 or flock\n' >&2
  exit 1
}

require_owned_or_absent() {
  path=$1
  if path_exists "$path" && ! owned_file "$path"; then
    printf 'install.sh: refusing to overwrite foreign %s\n' "$path" >&2
    exit 1
  fi
}

remove_if_owned() {
  path=$1
  if [ -f "$path" ] && [ ! -L "$path" ] && owned_file "$path"; then
    rm -f "$path"
  fi
}

status_of() {
  if ! path_exists "$marker_dest" && ! path_exists "$ts_dest" && ! path_exists "$report_dest"; then
    printf 'absent\n'
    return
  fi
  if ! owned_file "$marker_dest" || ! owned_file "$ts_dest" || ! owned_file "$report_dest"; then
    printf 'drifted\n'
    return
  fi
  version=$(sed -n 's/^YUZORA_WSL_ADAPTER_VERSION=//p' "$marker_dest" | head -n 1)
  ts_hash_m=$(sed -n 's/^YUZORA_WSL_ADAPTER_TS_SHA256=//p' "$marker_dest" | head -n 1)
  report_hash_m=$(sed -n 's/^YUZORA_WSL_ADAPTER_REPORT_SHA256=//p' "$marker_dest" | head -n 1)
  ts_hash=$(checksum "$ts_dest")
  report_hash=$(checksum "$report_dest")
  if [ "$ts_hash" != "$ts_hash_m" ] || [ "$report_hash" != "$report_hash_m" ]; then
    printf 'drifted\n'
    return
  fi
  if [ "$version" != "$VERSION" ]; then
    printf 'outdated\n'
    return
  fi
  printf 'current\n'
}

do_install() {
  require_seq_lock_backend
  [ -n "$source_root" ] || { printf 'install.sh: --source-root is required for install\n' >&2; exit 2; }
  [ -f "$ts_src" ] || { printf 'install.sh: missing %s\n' "$ts_src" >&2; exit 1; }
  [ -f "$report_src" ] || { printf 'install.sh: missing %s\n' "$report_src" >&2; exit 1; }
  mkdir -p "$ext_dir"
  if [ -f "$ext_dir/$OFFICIAL_NAME" ]; then
    : # leave official Herdr integration untouched
  fi
  require_owned_or_absent "$ts_dest"
  require_owned_or_absent "$report_dest"
  require_owned_or_absent "$marker_dest"
  atomic_copy "$ts_src" "$ts_dest" 644
  atomic_copy "$report_src" "$report_dest" 755
  write_marker
  printf 'installed %s\n' "$(status_of)"
}

do_uninstall() {
  remove_if_owned "$ts_dest"
  remove_if_owned "$report_dest"
  remove_if_owned "$marker_dest"
  printf 'uninstalled %s\n' "$(status_of)"
}

case "$action" in
  install) do_install ;;
  status)
    if have_seq_lock_backend; then
      printf '%s\n' "$(status_of)"
    else
      printf 'missing-prerequisite\n'
    fi
    ;;
  uninstall) do_uninstall ;;
  *) usage ;;
esac
