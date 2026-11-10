d=${1%/*}
[ -n "$d" ] || d=/
case $2 in
file)
  if there "$1"; then echo exists; exit 0; fi
  if msg=$( (set -C; : > "$1") 2>&1 ); then echo done; exit 0; fi
  ;;
folder)
  command -v mkdir >/dev/null 2>&1 || { echo 'refused tool mkdir'; exit 0; }
  if msg=$(mkdir -- "$1" 2>&1); then echo done; exit 0; fi
  ;;
*) fail "unknown kind $2" ;;
esac
blame "$1"
fail "$msg"
