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
held= heldid=
giveback() {
  if [ -n "$held" ] && [ "$(idof "$held")" = "$heldid" ]; then rm -f -- "$held"; fi
  held=
}
trap giveback EXIT
trap 'exit 1' HUP INT TERM
there "$from" || { echo gone; exit 0; }
if there "$to"; then
  if [ "$fold" = 1 ]; then
    a=$(idof "$from") b=$(idof "$to")
    if [ -n "$a" ] && [ "$a" = "$b" ]; then
      if msg=$(mv -- "$from" "$to" 2>&1); then echo done; exit 0; fi
      fail "$msg"
    fi
  fi
  echo exists
  exit 0
fi
blame "$to"
{ [ -w "$fd" ] && [ -x "$fd" ]; } || { echo 'refused folder'; exit 0; }
a=$(dev "$from") b=$(dev "$d")
if [ -n "$a" ] && [ -n "$b" ] && [ "$a" != "$b" ]; then echo 'refused device'; exit 0; fi
before=$(idof "$from")
if [ -d "$from" ] && [ ! -L "$from" ]; then
  if mv --version >/dev/null 2>&1; then flag=-nT; else flag=-n; fi
  msg=$(mv $flag -- "$from" "$to" 2>&1)
else
  if ! msg=$( (set -C; : > "$to") 2>&1 ); then blame "$to"; fail "$msg"; fi
  held=$to heldid=$(idof "$to")
  msg=$(mv -f -- "$from" "$to" 2>&1)
fi
if there "$from"; then
  giveback
  blame "$to"
  fail "$msg"
fi
held=
if [ -z "$before" ] || [ "$(idof "$to")" = "$before" ]; then echo done; exit 0; fi
fail "$from was moved into $to, a folder that took its name meanwhile"
