from=$1 into=$2 name=$3 stem=$4 ext=$5 id=$6 move=$7
shift 7
case $id in ''|*[!0-9a-f]*) fail 'no name for the scratch folder' ;; esac
[ "${#id}" -eq 32 ] || fail 'no name for the scratch folder'
d=$into
case $into in /) sd=/.yuzora-save-$id ;; *) sd=$into/.yuzora-save-$id ;; esac
there "$from" || { echo gone; exit 0; }
blame "$sd"
for tool in cp mkdir chmod ls id stat rm rmdir; do
  command -v "$tool" >/dev/null 2>&1 || { echo "refused tool $tool"; exit 0; }
done
idof() { stat -c '%d:%i' -- "$1" 2>/dev/null || stat -f '%d:%i' -- "$1" 2>/dev/null; }
owner() { stat -c %u -- "$1" 2>/dev/null || stat -f %u -- "$1" 2>/dev/null; }
bare() { left=$(ls -A . 2>/dev/null) && [ -z "$left" ]; }
kind=file
if [ -d "$from" ] && [ ! -L "$from" ]; then
  kind=folder stem=$name ext=
  a=$(cd -P -- "$from" 2>/dev/null && pwd -P) b=$(cd -P -- "$into" 2>/dev/null && pwd -P)
  if [ -n "$a" ] && [ -n "$b" ]; then
    case $b/ in "$a"/*) echo 'refused inside'; exit 0 ;; esac
  fi
fi
made= in=
leave() {
  [ -n "$made" ] || return 0
  if [ -n "$in" ]; then
    # A folder copied without write permission keeps what it holds until its owner may change it again.
    find ./c -type d -exec chmod u+rwx {} \; 2>/dev/null
    rm -rf ./c 2>/dev/null
  fi
  cd / 2>/dev/null
  rmdir -- "$sd" 2>/dev/null
  return 0
}
trap leave EXIT
trap 'echo "stopped before it was done: it may or may not have been" >&2; exit 1' HUP INT TERM
if ! msg=$(umask 077 && mkdir -- "$sd" 2>&1); then blame "$sd"; fail "$msg"; fi
made=1
was=$(idof "$sd")
cd -P -- "$sd" || exit 1
me=$(id -u)
# Someone who may rename in $into can put a link, or a folder, at the name: nothing is changed in what is entered
# until it is known to be the folder made (a link leads to another one), and nothing in it is cleared before.
{ [ "$(idof .)" = "$was" ] && bare && [ "$(owner .)" = "$me" ] && chmod 700 . && bare; } || fail "$sd is not the folder just made"
in=1
msg=$(cp -RP -- "$from" ./c 2>&1) || fail "$msg"
mine=$(idof ./c)
k=0
while [ "$k" -lt 1000 ]; do
  case $k in 0) cand=$name ;; 1) cand="$stem copy$ext" ;; *) cand="$stem copy $k$ext" ;; esac
  case $into in /) to=/$cand ;; *) to=$into/$cand ;; esac
  if there "$to"; then k=$((k + 1)); continue; fi
  j=0
  for held do
    j=$((j + 1))
    if [ "$held" = "$cand" ]; then echo "refused open $j"; exit 0; fi
  done
  if [ -n "$mine" ] && [ "$(idof "$sd/c")" != "$mine" ]; then fail "$sd is not the folder the copy was made in"; fi
  said=$(sh -c "$move" sh "$sd/c" "$to" 0) || exit 1
  case $said in
    done)
      if [ -n "$mine" ] && [ "$(idof "$to")" != "$mine" ]; then fail "$to is there, but it is not the copy that was made"; fi
      echo "done $kind $k"
      exit 0
      ;;
    exists) k=$((k + 1)) ;;
    gone) fail "the copy made in $sd is gone" ;;
    *) printf '%s\n' "$said"; exit 0 ;;
  esac
done
fail "every name from $name to $cand is taken in $into"
