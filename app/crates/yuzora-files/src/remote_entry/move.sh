from=$1 to=$2 fold=$3
d=${to%/*}
[ -n "$d" ] || d=/
fd=${from%/*}
[ -n "$fd" ] || fd=/
for tool in mv stat rm; do
  command -v "$tool" >/dev/null 2>&1 || { echo "refused tool $tool"; exit 0; }
done
idof() { stat -c '%d:%i' -- "$1" 2>/dev/null || stat -f '%d:%i' -- "$1" 2>/dev/null; }
dev() { stat -c %d -- "$1" 2>/dev/null || stat -f %d -- "$1" 2>/dev/null; }
held= heldid= heldkind= how= gnu= left=
giveback() {
  if [ -n "$held" ]; then
    if [ -z "$heldid" ]; then
      left="$held is left in place: it was made empty to hold the new name, and stat gives no identity here to tell it from an empty entry that someone else put there (delete it if it is empty and not yours)"
    elif [ "$(idof "$held")" != "$heldid" ]; then
      :
    elif [ "$heldkind" = dir ]; then
      rmdir -- "$held" 2>/dev/null
    elif [ -f "$held" ] && [ ! -L "$held" ] && [ ! -s "$held" ]; then
      rm -f -- "$held"
    fi
  fi
  held=
}
leave() { [ -n "$held" ] || return 0; giveback; [ -z "$left" ] || printf '%s\n' "$left" >&2; }
trap leave EXIT
trap 'echo "stopped before it was done: it may or may not have been" >&2; exit 1' HUP INT TERM
pick() {
  [ -z "$how" ] || return 0
  if command -v perl >/dev/null 2>&1; then
    how=perl
  else
    case $(mv --help 2>&1) in *-T*) how=mv ;; esac
  fi
  [ -n "$how" ] || { echo 'refused rename'; exit 0; }
}
rn() {
  if [ "$how" = perl ]; then
    perl -e 'rename($ARGV[0], $ARGV[1]) or do { print STDERR "$ARGV[0]: $!\n"; exit 1 }' "$1" "$2"
  else
    mv -fT -- "$1" "$2"
  fi
}
there "$from" || { echo gone; exit 0; }
if there "$to"; then
  if [ "$fold" = 1 ]; then
    a=$(idof "$from") b=$(idof "$to")
    if [ -n "$a" ] && [ "$a" = "$b" ]; then
      pick
      if msg=$(rn "$from" "$to" 2>&1); then echo done; exit 0; fi
      fail "$msg"
    fi
  fi
  echo exists
  exit 0
fi
blame "$to"
{ [ -w "$fd" ] && [ -x "$fd" ]; } || { echo 'refused folder'; exit 0; }
# stat looks at a link itself, and a rename goes through it: "/." leads to the folder that a link to one is
a=$(dev "$from") b=$(dev "$d/.")
if [ -n "$a" ] && [ -n "$b" ] && [ "$a" != "$b" ]; then echo 'refused device'; exit 0; fi
before=$(idof "$from")
case $(mv --version 2>&1) in *'GNU coreutils'*) gnu=1 ;; esac
if [ -n "$gnu" ]; then
  msg=$(mv -nT -- "$from" "$to" 2>&1)
  rc=$?
else
  pick
  if [ -d "$from" ] && [ ! -L "$from" ]; then
    for tool in mkdir rmdir; do
      command -v "$tool" >/dev/null 2>&1 || { echo "refused tool $tool"; exit 0; }
    done
    if msg=$(mkdir -- "$to" 2>&1); then held=$to heldkind=dir; else blame "$to"; fail "$msg"; fi
  else
    if msg=$( (set -C; : > "$to") 2>&1 ); then held=$to; else blame "$to"; fail "$msg"; fi
  fi
  heldid=$(idof "$to")
  msg=$(rn "$from" "$to" 2>&1)
  rc=$?
fi
if there "$from"; then
  giveback
  [ -z "$left" ] || fail "${msg:-it failed without saying why}: $left"
  blame "$to"
  fail "$msg"
fi
if [ "$rc" -ne 0 ]; then
  giveback
  [ -z "$left" ] || fail "$from is gone, and $left"
  echo gone
  exit 0
fi
held=
if [ -z "$before" ] || [ "$(idof "$to")" = "$before" ]; then echo done; exit 0; fi
fail "$from was renamed, but what is at $to is not the entry that was at $from: one of them was replaced meanwhile"
