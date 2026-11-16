into=$1 id=$2
case $id in ''|*[!0-9a-f]*) fail 'no name for the scratch folder' ;; esac
[ "${#id}" -eq 32 ] || fail 'no name for the scratch folder'
case $into in /) sd=/.yuzora-save-$id ;; *) sd=$into/.yuzora-save-$id ;; esac
there "$sd" || { echo done; exit 0; }
idof() { stat -c '%d:%i' -- "$1" 2>/dev/null || stat -f '%d:%i' -- "$1" 2>/dev/null; }
owner() { stat -c %u -- "$1" 2>/dev/null || stat -f %u -- "$1" 2>/dev/null; }
{ [ -d "$sd" ] && [ ! -L "$sd" ]; } || { echo left; exit 0; }
was=$(idof "$sd")
cd -P -- "$sd" 2>/dev/null || { echo left; exit 0; }
{ [ -n "$was" ] && [ "$(idof .)" = "$was" ] && [ "$(owner .)" = "$(id -u)" ]; } || { echo left; exit 0; }
# Only the copy named after the folder: anything else in it is not the copy's, and keeps the folder.
if there "./$id"; then
  find "./$id" -type d -exec chmod u+rwx {} \; 2>/dev/null
  rm -rf "./$id" 2>/dev/null
fi
cd / && rmdir -- "$sd" 2>/dev/null && { echo done; exit 0; }
echo left
