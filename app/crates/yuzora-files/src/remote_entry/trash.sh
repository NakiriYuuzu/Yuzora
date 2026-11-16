p=$1
d=${p%/*}
[ -n "$d" ] || d=/
there "$p" || { echo gone; exit 0; }
usable "$d" || { echo 'refused folder'; exit 0; }
if command -v gio >/dev/null 2>&1; then
  gio trash "$p" >/dev/null 2>&1 </dev/null
  there "$p" || { echo done; exit 0; }
fi
if command -v trash >/dev/null 2>&1; then
  trash "$p" >/dev/null 2>&1 </dev/null
  there "$p" || { echo done; exit 0; }
fi
nothing() { echo 'refused tool trash'; exit 0; }
case ${XDG_DATA_HOME:-} in /*) data=$XDG_DATA_HOME ;; *) data=${HOME:+$HOME/.local/share} ;; esac
case $data in /*) ;; *) nothing ;; esac
for tool in mkdir mv stat od date rm ls chmod id; do
  command -v "$tool" >/dev/null 2>&1 || { echo "refused tool $tool"; exit 0; }
done
t=$data/Trash
umask 077
# What is in the trash is for its owner alone, as gio makes it: a trash others may look into is closed first, and
# nothing is made in one that is someone else's.
mine() { [ "$(stat -c %u -- "$1" 2>/dev/null || stat -f %u -- "$1" 2>/dev/null)" = "$(id -u)" ]; }
closed() { case $(ls -ld -- "$1" 2>/dev/null) in d???------*) return 0 ;; *) return 1 ;; esac; }
! there "$t" || mine "$t/." || nothing
{ mkdir -p -- "$t/files" "$t/info" 2>/dev/null && usable "$t/files" && usable "$t/info"; } || nothing
mine "$t/." || nothing
closed "$t/." || { chmod go-rwx "$t/." 2>/dev/null && closed "$t/."; } || nothing
# The freedesktop trash of the home is for what is on its disk: a move from another one would copy.
dev() { stat -c %d -- "$1" 2>/dev/null || stat -f %d -- "$1" 2>/dev/null; }
a=$(dev "$p") b=$(dev "$t/files/.")
{ [ -n "$a" ] && [ -n "$b" ]; } || nothing
[ "$a" = "$b" ] || { echo 'refused device'; exit 0; }
# The spec's Path is URL-escaped (RFC 2396): every byte but letters, digits and / - . _ ~ as %xx.
url() {
  for o in $(printf '%s' "$1" | od -An -v -to1); do
    case $o in
      055|056|057|137|176|06[0-7]|07[01]|10[1-7]|11[0-7]|12[0-7]|13[0-2]|14[1-7]|15[0-7]|16[0-7]|17[0-2]) printf "\\$o" ;;
      *) printf '%%%02x' "$((0$o))" ;;
    esac
  done
}
enc=$(url "$p") when=$(date +%Y-%m-%dT%H:%M:%S)
[ -n "$enc" ] || { echo 'refused tool od'; exit 0; }
name=${p##*/}
case $name in ?*.*) stem=${name%.*} ext=.${name##*.} ;; *) stem=$name ext= ;; esac
i=1 n=$name
while [ "$i" -le 1000 ]; do
  if ! there "$t/files/$n"; then
    if (set -C; : > "$t/info/$n.trashinfo") 2>/dev/null; then
      # The name is this move's now: a write that fails (a full disk) gives it back and says why.
      if ! msg=$( { printf '[Trash Info]\nPath=%s\nDeletionDate=%s\n' "$enc" "$when" >> "$t/info/$n.trashinfo"; } 2>&1); then
        rm -f -- "$t/info/$n.trashinfo"
        fail "the trash information for $n could not be written${msg:+: $msg}"
      fi
      if msg=$(mv -- "$p" "$t/files/$n" 2>&1) && ! there "$p"; then echo done; exit 0; fi
      rm -f -- "$t/info/$n.trashinfo"
      fail "$msg"
    fi
    there "$t/info/$n.trashinfo" || nothing
  fi
  i=$((i + 1)) n=$stem.$i$ext
done
nothing
