#!/bin/sh
# Runs the remote make script (/w/make.sh) and the remote move script (/w/move.sh) on Linux, as
# ordinary users: what a new file or folder gets compared with what `touch` and `mkdir` give in the
# same folder, names that are taken, makers and movers that race, a move between file systems,
# programs that are missing.
# usage (as root, in a throwaway container with two tmpfs at /mnt/a and /mnt/b):
#   remote-entry.sh [shell to run the scripts under]
SH=${1:-sh}
MAKE=/w/make.sh
MOVE=/w/move.sh
fails=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fails=$((fails + 1)); }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: wanted [$2] got [$3]"; fi; }
no() { case $2 in *"$3"*) bad "$1: found [$3] in [$2]" ;; *) ok "$1" ;; esac; }
yes() { case $2 in *"$3"*) ok "$1" ;; *) bad "$1: wanted [$3] in [$2]" ;; esac; }

if command -v apk >/dev/null 2>&1; then
  apk add --no-cache acl >/dev/null 2>&1 || { echo "FAIL install acl"; exit 1; }
  mkuser() { adduser -D -u "$2" "$1"; }
  mkgroup() { addgroup -g "$2" "$1"; }
else
  { apt-get update && apt-get install -y acl; } >/dev/null 2>&1 || { echo "FAIL install acl"; exit 1; }
  mkuser() { useradd -m -u "$2" "$1"; }
  mkgroup() { groupadd -g "$2" "$1"; }
fi
{
  mkuser bob 1500
  mkgroup team 2000; mkgroup devs 2002
} >/dev/null 2>&1
echo "== $(. /etc/os-release; echo "$PRETTY_NAME") sh=$SH mv=$(readlink -f "$(command -v mv)") $(mv --version 2>&1 | head -1 | cut -c1-30)"

# make AS UMASK PATH KIND [ENV]: the script's one line, then exit=N
make() {
  su -s /bin/sh "$1" -c "umask $2; $5 $SH -c \"\$(cat $MAKE)\" sh \"\$0\" \"\$1\" 2>/tmp/stderr.txt; echo exit=\$?" "$3" "$4"
}
# move AS FROM TO FOLD [ENV]
move() {
  su -s /bin/sh "$1" -c "$5 $SH -c \"\$(cat $MOVE)\" sh \"\$0\" \"\$1\" \"\$2\" 2>/tmp/stderr.txt; echo exit=\$?" "$2" "$3" "$4"
}
verdict() { "$@" | sed -n 1p; }
as() { u=$1; shift; su -s /bin/sh "$u" -c "$*"; }
reset() { rm -rf "$1"; mkdir -p "$1"; chown bob:bob "$1"; chmod 755 "$1"; }
mode() { stat -c '%a %U %G' -- "$1"; }
acl() { getfacl -c -- "$1" 2>/dev/null | sed 's/[[:space:]]*#.*//'; }
# A program is taken off the machine by a copy of it that is put back with cp (mv may be the one gone).
hide() {
  mkdir -p /hidden
  for t in "$@"; do
    where=$(command -v "$t")
    cp -p "$where" "/hidden/$t" && echo "$where" > "/hidden/$t.where" && rm -f "$where"
  done
}
unhide() {
  for w in /hidden/*.where; do
    [ -e "$w" ] || continue
    t=${w%.where}
    cp -p "$t" "$(cat "$w")"
  done
  rm -rf /hidden
}
mkdir -p /tmp/shim
chmod 755 /tmp/shim

echo "-- 1. a new file or folder gets what touch and mkdir give there: under every umask"
for um in 022 077 002 027 000; do
  reset /tmp/u1
  as bob "umask $um; touch /tmp/u1/t; mkdir /tmp/u1/d"
  expect "1 file umask $um" "$(mode /tmp/u1/t)" "$( make bob $um /tmp/u1/f file >/dev/null; mode /tmp/u1/f)"
  expect "1 folder umask $um" "$(mode /tmp/u1/d)" "$( make bob $um /tmp/u1/g folder >/dev/null; mode /tmp/u1/g)"
done

echo "-- 2. a folder's default ACL reaches the new entry as it reaches touch's and mkdir's, and no wider"
reset /tmp/u2
setfacl -d -m g:team:rw /tmp/u2
as bob 'umask 022; touch /tmp/u2/t; mkdir /tmp/u2/d'
expect "2 file answer" "done" "$(verdict make bob 022 /tmp/u2/f file)"
expect "2 folder answer" "done" "$(verdict make bob 022 /tmp/u2/g folder)"
expect "2 file mode" "$(mode /tmp/u2/t)" "$(mode /tmp/u2/f)"
expect "2 file acl" "$(acl /tmp/u2/t)" "$(acl /tmp/u2/f)"
expect "2 folder mode" "$(mode /tmp/u2/d)" "$(mode /tmp/u2/g)"
expect "2 folder acl" "$(acl /tmp/u2/d)" "$(acl /tmp/u2/g)"
yes "2 the folder's entry did reach touch's file (the control)" "$(acl /tmp/u2/t)" "group:team:"

echo "-- 3. a setgid folder's group is handed down as to touch and mkdir"
reset /tmp/u3
chown bob:devs /tmp/u3; chmod 2775 /tmp/u3
as bob 'umask 002; touch /tmp/u3/t; mkdir /tmp/u3/d'
make bob 002 /tmp/u3/f file >/dev/null; make bob 002 /tmp/u3/g folder >/dev/null
expect "3 file" "$(mode /tmp/u3/t)" "$(mode /tmp/u3/f)"
expect "3 folder" "$(mode /tmp/u3/d)" "$(mode /tmp/u3/g)"
yes "3 the group came from the folder (the control)" "$(mode /tmp/u3/f)" "devs"

echo "-- 4. a name that is taken is taken: files, folders, links, links to nothing, pipes"
reset /tmp/u4
as bob 'echo mine > /tmp/u4/file; mkdir /tmp/u4/dir; ln -s nowhere /tmp/u4/dangling; ln -s file /tmp/u4/live; mkfifo /tmp/u4/pipe'
for n in file dir dangling live pipe; do
  expect "4 make file over $n" "exists" "$(verdict make bob 022 /tmp/u4/$n file)"
  expect "4 make folder over $n" "exists" "$(verdict make bob 022 /tmp/u4/$n folder)"
done
expect "4 file kept" "mine" "$(cat /tmp/u4/file)"
expect "4 nothing else made" "dangling dir file live pipe" "$(ls /tmp/u4 | tr '\n' ' ' | sed 's/ $//')"
expect "4 folder missing" "refused nofolder" "$(verdict make bob 022 /tmp/u4/nope/x file)"
expect "4 folder is a file" "refused nofolder" "$(verdict make bob 022 /tmp/u4/file/x folder)"
chmod 555 /tmp/u4
expect "4 folder not writable" "refused folder" "$(verdict make bob 022 /tmp/u4/new file)"
expect "4 folder not writable (folder)" "refused folder" "$(verdict make bob 022 /tmp/u4/new folder)"
chmod 755 /tmp/u4

echo "-- 5. twenty makers of one name: one makes it"
reset /tmp/u5
n=0
while [ "$n" -lt 20 ]; do ( verdict make bob 022 /tmp/u5/contested file > /tmp/u5.out.$n ) & n=$((n + 1)); done
wait
expect "5 one done" "1" "$(cat /tmp/u5.out.* | grep -c '^done$')"
expect "5 the others exist" "19" "$(cat /tmp/u5.out.* | grep -c '^exists$')"
rm -f /tmp/u5.out.*
reset /tmp/u5
n=0
while [ "$n" -lt 20 ]; do ( verdict make bob 022 /tmp/u5/contested folder > /tmp/u5.out.$n ) & n=$((n + 1)); done
wait
expect "5 one folder" "1" "$(cat /tmp/u5.out.* | grep -c '^done$')"
rm -f /tmp/u5.out.*

echo "-- 6. a rename moves, and never replaces what is there"
reset /tmp/u6
as bob 'echo mine > /tmp/u6/a; echo theirs > /tmp/u6/b; mkdir /tmp/u6/empty; ln -s nowhere /tmp/u6/dangling; ln -s b /tmp/u6/live; mkdir /tmp/u6/src; echo x > /tmp/u6/src/x'
for to in b empty dangling live; do
  expect "6 file onto $to" "exists" "$(verdict move bob /tmp/u6/a /tmp/u6/$to 0)"
  expect "6 folder onto $to" "exists" "$(verdict move bob /tmp/u6/src /tmp/u6/$to 0)"
done
expect "6 nothing moved" "mine theirs x" "$(cat /tmp/u6/a /tmp/u6/b /tmp/u6/src/x | tr '\n' ' ' | sed 's/ $//')"
expect "6 nothing went in" "" "$(ls /tmp/u6/empty)"
expect "6 the link is still a link" "nowhere" "$(readlink /tmp/u6/dangling)"
expect "6 file" "done" "$(verdict move bob /tmp/u6/a /tmp/u6/c 0)"
expect "6 folder" "done" "$(verdict move bob /tmp/u6/src /tmp/u6/moved 0)"
expect "6 link moved as a link" "done" "$(verdict move bob /tmp/u6/live /tmp/u6/live2 0)"
expect "6 result" "mine x b" "$( (cat /tmp/u6/c /tmp/u6/moved/x; readlink /tmp/u6/live2) | tr '\n' ' ' | sed 's/ $//')"
expect "6 gone" "gone" "$(verdict move bob /tmp/u6/a /tmp/u6/z 0)"
expect "6 no folder" "refused nofolder" "$(verdict move bob /tmp/u6/c /tmp/u6/nope/z 0)"
expect "6 same name" "exists" "$(verdict move bob /tmp/u6/c /tmp/u6/c 0)"
expect "6 case-sensitive: another name is another file" "exists" "$(as bob 'echo C > /tmp/u6/C'; verdict move bob /tmp/u6/c /tmp/u6/C 1)"
expect "6 nothing left behind" "C b c dangling empty live2 moved" "$(ls /tmp/u6 | tr '\n' ' ' | sed 's/ $//')"
chmod 555 /tmp/u6
expect "6 folder not writable" "refused folder" "$(verdict move bob /tmp/u6/c /tmp/u6/d 0)"
chmod 755 /tmp/u6

echo "-- 7. names are bytes"
reset /tmp/u7
as bob 'echo x > "/tmp/u7/a b"'
expect "7 space to quote" "done" "$(verdict move bob "/tmp/u7/a b" "/tmp/u7/it's" 0)"
expect "7 quote to dollar" "done" "$(verdict move bob "/tmp/u7/it's" '/tmp/u7/$HOME`x`' 0)"
nl='
'
expect "7 to a name with a newline" "done" "$(verdict move bob '/tmp/u7/$HOME`x`' "/tmp/u7/two${nl}lines" 0)"
expect "7 make a name with a newline" "done" "$(verdict make bob 022 "/tmp/u7/more${nl}lines" file)"
expect "7 and it is taken" "exists" "$(verdict make bob 022 "/tmp/u7/more${nl}lines" file)"
expect "7 a name that starts with a dash" "done" "$(verdict move bob "/tmp/u7/two${nl}lines" /tmp/u7/-dash 0)"
expect "7 content" "x" "$(cat /tmp/u7/-dash)"

echo "-- 8. a rename takes the name first (a file): another maker, just before the move, cannot"
cat > /tmp/shim/mv <<'SHIM'
#!/bin/sh
PATH=/usr/bin:/bin
case $1 in -f|-n|-nT)
  for last; do :; done
  case $YZ_KIND in
    file) ( set -C; echo racer > "$last" ) 2>/dev/null && echo made > /tmp/race || echo refused > /tmp/race ;;
    link) ln -s nowhere "$last" ;;
    folder) mkdir "$last" ;;
  esac
esac
exec mv "$@"
SHIM
chmod 755 /tmp/shim/mv
reset /tmp/u8
as bob 'echo mine > /tmp/u8/from'
rm -f /tmp/race
expect "8 file moved" "done" "$(verdict move bob /tmp/u8/from /tmp/u8/to 0 'PATH=/tmp/shim:$PATH YZ_KIND=file')"
expect "8 the other maker was refused" "refused" "$(cat /tmp/race)"
expect "8 content" "mine" "$(cat /tmp/u8/to)"

echo "-- 9. a rename of a folder onto a name made just before the move"
for kind in file link folder; do
  reset /tmp/u9
  as bob 'mkdir /tmp/u9/from; echo x > /tmp/u9/from/x'
  r=$(verdict move bob /tmp/u9/from /tmp/u9/to 0 "PATH=/tmp/shim:\$PATH YZ_KIND=$kind")
  if mv --version >/dev/null 2>&1; then
    expect "9 GNU mv asks the system not to replace ($kind)" "exists" "$r"
    expect "9 and the folder is where it was ($kind)" "x" "$(cat /tmp/u9/from/x)"
  else
    case $r in
      exists) ok "9 (this mv looked first) refused ($kind)"; expect "9 the folder is where it was ($kind)" "x" "$(cat /tmp/u9/from/x)" ;;
      *) if [ "$kind" = folder ]; then yes "9 (this mv looks first, then moves) said where it went ($kind)" "$(cat /tmp/stderr.txt)" "moved into"; expect "9 and it is there ($kind)" "x" "$(cat /tmp/u9/to/from/x)"; else bad "9 $kind: [$r] $(cat /tmp/stderr.txt)"; fi ;;
    esac
  fi
done

echo "-- 10. a move that fails leaves the entry and no empty file; one that is stopped says so"
cat > /tmp/shim/mv <<'SHIM'
#!/bin/sh
echo "mv: simulated: disk on fire" >&2
exit 1
SHIM
reset /tmp/u10
as bob 'echo mine > /tmp/u10/from; mkdir /tmp/u10/dir'
expect "10 file: failed" "exit=1" "$(move bob /tmp/u10/from /tmp/u10/to 0 'PATH=/tmp/shim:$PATH' | sed -n 1p)"
yes "10 file: its words" "$(cat /tmp/stderr.txt)" "disk on fire"
expect "10 file: the entry and nothing else" "dir from" "$(ls /tmp/u10 | tr '\n' ' ' | sed 's/ $//')"
expect "10 folder: failed" "exit=1" "$(move bob /tmp/u10/dir /tmp/u10/to2 0 'PATH=/tmp/shim:$PATH' | sed -n 1p)"
expect "10 folder: the entry and nothing else" "dir from" "$(ls /tmp/u10 | tr '\n' ' ' | sed 's/ $//')"
rm -f /tmp/shim/mv

echo "-- 11. a rename does not copy between file systems"
if [ -d /mnt/a ] && [ -d /mnt/b ] && [ "$(stat -c %d /mnt/a)" != "$(stat -c %d /mnt/b)" ]; then
  chown bob:bob /mnt/a /mnt/b
  as bob 'echo mine > /mnt/a/f; mkdir /mnt/a/d'
  expect "11 file" "refused device" "$(verdict move bob /mnt/a/f /mnt/b/f 0)"
  expect "11 folder" "refused device" "$(verdict move bob /mnt/a/d /mnt/b/d 0)"
  expect "11 the entries are where they were" "d f" "$(ls /mnt/a | tr '\n' ' ' | sed 's/ $//')"
  expect "11 nothing arrived" "" "$(ls -A /mnt/b)"
  expect "11 within one" "done" "$(verdict move bob /mnt/a/f /mnt/a/g 0)"
else
  echo "note 11: the container has no two file systems at /mnt/a and /mnt/b"
fi

echo "-- 12. a program that is missing is named"
reset /tmp/u12
as bob 'echo mine > /tmp/u12/from'
for tool in mv stat rm; do
  hide "$tool"
  expect "12 without $tool" "refused tool $tool" "$(verdict move bob /tmp/u12/from /tmp/u12/to 0)"
  unhide
done
hide mkdir
expect "12 without mkdir" "refused tool mkdir" "$(verdict make bob 022 /tmp/u12/d folder)"
expect "12 a file needs none" "done" "$(verdict make bob 022 /tmp/u12/f file)"
unhide
expect "12 nothing changed" "f from" "$(ls /tmp/u12 | tr '\n' ' ' | sed 's/ $//')"

echo "RESULT fails=$fails"
