into=$1 id=$2
case $id in ''|*[!0-9a-f]*) fail 'no name for the scratch folder' ;; esac
[ "${#id}" -eq 32 ] || fail 'no name for the scratch folder'
s=.yuzora-save-$id
# Looked up once, by its path: not there means gone only where that folder can be entered (a folder renamed away,
# or one that cannot be entered, may hold it out of sight).
cd -P -- "$into" 2>/dev/null || { echo left; exit 0; }
there "./$s" || { echo done; exit 0; }
idof() { stat -c '%d:%i' -- "$1" 2>/dev/null || stat -f '%d:%i' -- "$1" 2>/dev/null; }
owner() { stat -c %u -- "$1" 2>/dev/null || stat -f %u -- "$1" 2>/dev/null; }
{ [ -d "./$s" ] && [ ! -L "./$s" ]; } || { echo left; exit 0; }
was=$(idof "./$s")
cd -P -- "./$s" 2>/dev/null || { echo left; exit 0; }
{ [ -n "$was" ] && [ "$(idof .)" = "$was" ] && [ "$(owner .)" = "$(id -u)" ]; } || { echo left; exit 0; }
# Only the copy named after the folder: anything else in it is not the copy's, and keeps the folder.
if there "./$id"; then
  find "./$id" -type d -exec chmod u+rwx {} \; 2>/dev/null
  rm -rf "./$id" 2>/dev/null
fi
# The name in the folder it is in (its ".."), and only while it is still the folder checked.
cd -P .. 2>/dev/null && [ "$(idof "./$s")" = "$was" ] && rmdir -- "./$s" 2>/dev/null && { echo done; exit 0; }
echo left
