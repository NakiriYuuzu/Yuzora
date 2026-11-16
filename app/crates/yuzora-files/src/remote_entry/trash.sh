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
[ -n "$data" ] || nothing
for tool in mkdir mv stat od date rm; do
  command -v "$tool" >/dev/null 2>&1 || { echo "refused tool $tool"; exit 0; }
done
t=$data/Trash
{ mkdir -p -- "$t/files" "$t/info" 2>/dev/null && usable "$t/files" && usable "$t/info"; } || nothing
# The freedesktop trash of the home is for what is on its disk: a move from another one would copy.
dev() { stat -c %d -- "$1" 2>/dev/null || stat -f %d -- "$1" 2>/dev/null; }
a=$(dev "$p") b=$(dev "$t/files/.")
{ [ -n "$a" ] && [ "$a" = "$b" ]; } || nothing
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
name=${p##*/}
case $name in ?*.*) stem=${name%.*} ext=.${name##*.} ;; *) stem=$name ext= ;; esac
i=1 n=$name
while [ "$i" -le 1000 ]; do
  if ! there "$t/files/$n"; then
    if (set -C; printf '[Trash Info]\nPath=%s\nDeletionDate=%s\n' "$enc" "$when" > "$t/info/$n.trashinfo") 2>/dev/null; then
      if msg=$(mv -- "$p" "$t/files/$n" 2>&1) && ! there "$p"; then echo done; exit 0; fi
      rm -f -- "$t/info/$n.trashinfo"
      fail "$msg"
    fi
    there "$t/info/$n.trashinfo" || nothing
  fi
  i=$((i + 1)) n=$stem.$i$ext
done
nothing
