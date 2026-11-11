there() { [ -e "$1" ] || [ -L "$1" ]; }
usable() { [ -d "$1" ] && [ -w "$1" ] && [ -x "$1" ]; }
blame() {
  if there "$1"; then echo exists; exit 0; fi
  up=$d
  until [ -e "$up" ] || [ "$up" = / ]; do up=${up%/*}; [ -n "$up" ] || up=/; done
  if [ "$up" != "$d" ]; then
    if [ -d "$up" ] && [ ! -x "$up" ]; then echo 'refused folder'; else echo 'refused nofolder'; fi
    exit 0
  fi
  [ -d "$d" ] || { echo 'refused nofolder'; exit 0; }
  usable "$d" || { echo 'refused folder'; exit 0; }
}
fail() { printf '%s\n' "${1:-it failed without saying why}" >&2; exit 1; }
