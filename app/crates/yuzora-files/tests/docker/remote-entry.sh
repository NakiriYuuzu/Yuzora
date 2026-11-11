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

# The stand-in for mv and perl: it does YZ_WHAT just before the real one, which YZ_REALMV can name. YZ_MVLOOKS
# makes mv answer --version and --help as another mv would, so that the script takes the way it takes there.
cat > /tmp/shim/hook <<'SHIM'
#!/bin/sh
tool=${0##*/}
PATH=/usr/local/bin:/usr/bin:/bin
case $tool:$1 in
  mv:--version|mv:--help)
    case $YZ_MVLOOKS in
      bsd) echo "usage: mv [-f | -i | -n] [-hv] source target" >&2; exit 64 ;;
      gnuless) if [ "$1" = --version ]; then echo "mv (uutils coreutils) 0.2.2"; else echo "  -T, --no-target-directory  treat DEST as a normal file"; fi; exit 0 ;;
    esac ;;
esac
case $1 in -f|-n|-T|-fT|-nT|-e|--no-copy)
  for a in "$@"; do from=$to; to=$a; done
  case $YZ_WHAT in
    vanish) mv -- "$from" /tmp/elsewhere/ ;;
    swap) rm -f -- "$to"; rmdir -- "$to" 2>/dev/null; ln -s /tmp/victim "$to" ;;
    racefile) ( set -C; echo racer > "$to" ) 2>/dev/null && echo made > /tmp/race || echo refused > /tmp/race ;;
    racelink) ln -s nowhere "$to" ;;
    racefolder) mkdir "$to" ;;
    fail) echo "mv: simulated: disk on fire" >&2; exit 1 ;;
  esac
esac
exec "${YZ_REALMV:-$tool}" "$@"
SHIM
chmod 755 /tmp/shim/hook
have_perl() { command -v perl >/dev/null 2>&1; }
is_gnu() { mv --version 2>&1 | grep -q 'GNU coreutils'; }
# The ways the script goes about it: with this machine's own mv (native), with a mv that has no -T and perl (bsd), with
# one that has -T and no perl (mvT), and with the uutils mv where there is one (uutils).
variant_env() {
  case $1 in
    native) echo "" ;;
    bsd) have_perl && echo "YZ_MVLOOKS=bsd" || echo SKIP ;;
    mvT) echo "YZ_MVLOOKS=gnuless" ;;
    uutils) [ -x /usr/lib/cargo/bin/coreutils/mv ] && echo "YZ_REALMV=/usr/lib/cargo/bin/coreutils/mv" || echo SKIP ;;
  esac
}
variant_start() {
  rm -f /tmp/shim/mv /tmp/shim/perl
  ln -s hook /tmp/shim/mv
  case $1 in
    native|bsd) have_perl && ln -s hook /tmp/shim/perl ;;
    mvT|uutils) have_perl && hide perl ;;
  esac
  return 0
}
variant_end() { rm -f /tmp/shim/mv /tmp/shim/perl; [ ! -d /hidden ] || unhide; }
# whether the script takes the new name first in this variant (not on GNU's own mv)
takes_first() { if [ "$1" = native ] && is_gnu; then echo no; else echo yes; fi; }
VARIANTS="native bsd mvT uutils"

echo "-- 8. a name made just before the rename is never replaced"
for variant in $VARIANTS; do
  env=$(variant_env $variant); [ "$env" != SKIP ] || continue
  variant_start $variant
  reset /tmp/u8
  as bob 'echo mine > /tmp/u8/from'
  rm -f /tmp/race
  r=$(verdict move bob /tmp/u8/from /tmp/u8/to 0 "PATH=/tmp/shim:\$PATH YZ_WHAT=racefile $env")
  if [ "$(takes_first $variant)" = yes ]; then
    expect "8 $variant: the name was taken first, so the other maker was refused" "refused" "$(cat /tmp/race)"
    expect "8 $variant: renamed" "done" "$r"
    expect "8 $variant: content" "mine" "$(cat /tmp/u8/to)"
  else
    expect "8 $variant: the system gave way to the other maker" "made" "$(cat /tmp/race)"
    expect "8 $variant: refused" "exists" "$r"
    expect "8 $variant: theirs is kept" "racer" "$(cat /tmp/u8/to)"
    expect "8 $variant: mine is where it was" "mine" "$(cat /tmp/u8/from)"
  fi
  variant_end
done

echo "-- 9. a folder is never moved into or onto a name made just before the rename"
for variant in $VARIANTS; do
  env=$(variant_env $variant); [ "$env" != SKIP ] || continue
  variant_start $variant
  for kind in file link folder; do
    reset /tmp/u9
    as bob 'mkdir /tmp/u9/from; echo x > /tmp/u9/from/x'
    r=$(verdict move bob /tmp/u9/from /tmp/u9/to 0 "PATH=/tmp/shim:\$PATH YZ_WHAT=race$kind $env")
    case $r in
      done) expect "9 $variant $kind: it is there" "x" "$(cat /tmp/u9/to/x)" ;;
      exists) expect "9 $variant $kind: refused, and it is where it was" "x" "$(cat /tmp/u9/from/x)" ;;
      *) bad "9 $variant $kind: [$r] $(cat /tmp/stderr.txt)" ;;
    esac
    [ ! -e /tmp/u9/to/from ] && ok "9 $variant $kind: it did not go into the name" || bad "9 $variant $kind: it went into the name"
  done
  variant_end
done

echo "-- 10. a move that fails leaves the entry and no empty file or folder in its name"
for variant in $VARIANTS; do
  env=$(variant_env $variant); [ "$env" != SKIP ] || continue
  variant_start $variant
  reset /tmp/u10
  as bob 'echo mine > /tmp/u10/from; mkdir /tmp/u10/dir'
  expect "10 $variant file: failed" "exit=1" "$(move bob /tmp/u10/from /tmp/u10/to 0 "PATH=/tmp/shim:\$PATH YZ_WHAT=fail $env" | sed -n 1p)"
  yes "10 $variant file: its words" "$(cat /tmp/stderr.txt)" "disk on fire"
  expect "10 $variant file: the entry and nothing else" "dir from" "$(ls /tmp/u10 | tr '\n' ' ' | sed 's/ $//')"
  expect "10 $variant folder: failed" "exit=1" "$(move bob /tmp/u10/dir /tmp/u10/to2 0 "PATH=/tmp/shim:\$PATH YZ_WHAT=fail $env" | sed -n 1p)"
  expect "10 $variant folder: the entry and nothing else" "dir from" "$(ls /tmp/u10 | tr '\n' ' ' | sed 's/ $//')"
  variant_end
done

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

echo "-- 13. the new name is swapped for a link to a folder just before the rename: nothing reaches that folder"
for variant in $VARIANTS; do
  env=$(variant_env $variant); [ "$env" != SKIP ] || { echo "note 13: no way to run $variant here"; continue; }
  variant_start $variant
  for k in file link folder; do
    reset /tmp/u13
    rm -rf /tmp/victim; mkdir /tmp/victim; chown bob:bob /tmp/victim
    case $k in
      folder) as bob 'mkdir /tmp/u13/entry; echo x > /tmp/u13/entry/x' ;;
      link) as bob 'ln -s somewhere /tmp/u13/entry; echo precious > /tmp/victim/entry' ;;
      file) as bob 'echo mine > /tmp/u13/entry; echo precious > /tmp/victim/entry' ;;
    esac
    r=$(verdict move bob /tmp/u13/entry /tmp/u13/renamed 0 "PATH=/tmp/shim:\$PATH YZ_WHAT=swap $env")
    if [ "$k" = folder ]; then
      expect "13 $variant $k: nothing went into the folder" "" "$(ls -A /tmp/victim)"
    else
      expect "13 $variant $k: the folder's entry is as it was" "precious" "$(cat /tmp/victim/entry)"
      expect "13 $variant $k: nothing else went in" "entry" "$(ls -A /tmp/victim)"
    fi
    case $r in
      done) if [ "$k" = link ]; then expect "13 $variant $k: it is the link that was renamed" "somewhere" "$(readlink /tmp/u13/renamed)"; else ok "13 $variant $k: renamed, and what took the name is gone"; fi ;;
      exists) if [ -e /tmp/u13/entry ] || [ -L /tmp/u13/entry ]; then ok "13 $variant $k: refused, and the entry is where it was"; else bad "13 $variant $k: refused but the entry is gone"; fi ;;
      *) bad "13 $variant $k: [$r] $(cat /tmp/stderr.txt)" ;;
    esac
  done
  variant_end
done

echo "-- 14. the source vanishes after the new name was taken: the name is given back, and it is said that the source is gone"
for variant in $VARIANTS; do
  env=$(variant_env $variant); [ "$env" != SKIP ] || continue
  variant_start $variant
  for k in file link folder; do
    reset /tmp/u14
    rm -rf /tmp/elsewhere; mkdir /tmp/elsewhere; chown bob:bob /tmp/elsewhere
    case $k in
      folder) as bob 'mkdir /tmp/u14/entry; echo x > /tmp/u14/entry/x' ;;
      link) as bob 'ln -s somewhere /tmp/u14/entry' ;;
      file) as bob 'echo mine > /tmp/u14/entry' ;;
    esac
    expect "14 $variant $k: said gone" "gone" "$(verdict move bob /tmp/u14/entry /tmp/u14/renamed 0 "PATH=/tmp/shim:\$PATH YZ_WHAT=vanish $env")"
    expect "14 $variant $k: nothing of ours left" "" "$(ls -A /tmp/u14)"
  done
  variant_end
done

echo "-- 15. a machine that has neither GNU mv, a mv with -T, nor perl is refused, not given a plain mv; the programs a folder rename needs are named"
rm -f /tmp/shim/mv /tmp/shim/perl
ln -s hook /tmp/shim/mv
bsd='PATH=/tmp/shim:$PATH YZ_MVLOOKS=bsd'
reset /tmp/u15
as bob 'echo mine > /tmp/u15/from; mkdir /tmp/u15/dir'
if have_perl; then hide perl; fi
expect "15 file" "refused rename" "$(verdict move bob /tmp/u15/from /tmp/u15/to 0 "$bsd")"
expect "15 folder" "refused rename" "$(verdict move bob /tmp/u15/dir /tmp/u15/to2 0 "$bsd")"
expect "15 a name that is there is still answered as it is" "exists" "$(verdict move bob /tmp/u15/from /tmp/u15/dir 0 "$bsd")"
expect "15 a source that is not there is still answered as it is" "gone" "$(verdict move bob /tmp/u15/nope /tmp/u15/to 0 "$bsd")"
expect "15 nothing changed" "dir from" "$(ls /tmp/u15 | tr '\n' ' ' | sed 's/ $//')"
variant_end
if have_perl; then
  variant_start bsd
  for tool in mkdir rmdir; do
    hide "$tool"
    expect "15 a folder without $tool (with perl, the way a Mac goes about it)" "refused tool $tool" "$(verdict move bob /tmp/u15/dir /tmp/u15/to2 0 "$bsd")"
    unhide
  done
  expect "15 a file needs neither" "done" "$(verdict move bob /tmp/u15/from /tmp/u15/to 0 "$bsd")"
  variant_end
fi

inode() { ls -di -- "$1" | cut -d' ' -f1; }

echo "-- 16. a folder that is a link: the change is in the folder it points to, on whatever file system that is"
if [ -d /mnt/a ] && [ "$(stat -c %d /mnt/a)" != "$(stat -c %d /tmp)" ]; then
  chown bob:bob /mnt/a
  reset /tmp/u16
  as bob 'mkdir /mnt/a/proj; echo mine > /mnt/a/proj/file; mkdir /tmp/u16/real2; echo two > /tmp/u16/real2/file; ln -s /mnt/a/proj /tmp/u16/proj; ln -s real2 /tmp/u16/proj2; echo out > /tmp/u16/outside; mkdir /tmp/u16/outdir; echo x > /tmp/u16/outdir/x'
  expect "16 a rename inside a link to a folder on another file system" "done" "$(verdict move bob /tmp/u16/proj/file /tmp/u16/proj/file2 0)"
  expect "16 it is renamed there" "mine" "$(cat /mnt/a/proj/file2)"
  expect "16 a file is made there" "done" "$(verdict make bob 022 /tmp/u16/proj/new file)"
  expect "16 a folder is made there" "done" "$(verdict make bob 022 /tmp/u16/proj/newdir folder)"
  expect "16 a rename inside a link to a folder on the same file system" "done" "$(verdict move bob /tmp/u16/proj2/file /tmp/u16/proj2/file2 0)"
  expect "16 it is renamed there" "two" "$(cat /tmp/u16/real2/file2)"
  i=$(inode /tmp/u16/outside)
  expect "16 a file from outside into the folder on another file system is refused" "refused device" "$(verdict move bob /tmp/u16/outside /tmp/u16/proj/in 0)"
  expect "16 it is where it was, and it is the same file" "$i" "$(inode /tmp/u16/outside)"
  i=$(inode /tmp/u16/outdir)
  expect "16 a folder from outside into the folder on another file system is refused" "refused device" "$(verdict move bob /tmp/u16/outdir /tmp/u16/proj/indir 0)"
  expect "16 it is where it was, and it is the same folder" "$i" "$(inode /tmp/u16/outdir)"
  expect "16 nothing arrived there" "file2 new newdir" "$(ls /mnt/a/proj | tr '\n' ' ' | sed 's/ $//')"
  expect "16 out of the folder on another file system is refused" "refused device" "$(verdict move bob /tmp/u16/proj/file2 /tmp/u16/out2 0)"
  expect "16 it is where it was" "mine" "$(cat /mnt/a/proj/file2)"
  expect "16 the link itself is renamed where it is" "done" "$(verdict move bob /tmp/u16/proj /tmp/u16/proj-renamed 0)"
  expect "16 it still points where it did" "/mnt/a/proj" "$(readlink /tmp/u16/proj-renamed)"
else
  echo "note 16: the container has no tmpfs at /mnt/a"
fi

echo "-- 17. a folder out of an image layer of an overlay file system is not copied: it stays, and nothing is at the new name"
root_fs=$(awk '$2 == "/" { t = $3 } END { print t }' /proc/mounts 2>/dev/null)
if [ "$root_fs" = overlay ] && [ -d /usr/local/share ] && [ -d /usr/local/lib ] && [ -d /usr/local/bin ]; then
  chown bob:bob /usr/local
  # Whether this overlay lets a folder out of an image layer be renamed at all (redirect_dir): perl's rename(2) says.
  exdev=unknown
  if have_perl; then
    if perl -e 'rename($ARGV[0], $ARGV[1]) or exit($!{EXDEV} ? 18 : 1)' /usr/local/bin /usr/local/bin.probe; then
      exdev=no; mv /usr/local/bin.probe /usr/local/bin
    else
      [ $? -eq 18 ] && exdev=yes
    fi
  fi
  echo "note 17: renaming a folder out of an image layer here gives EXDEV: $exdev"
  as bob 'mkdir /usr/local/runtime'
  # One folder of an image layer for each way the script goes about it: a rename that goes wrong must not spoil the next one.
  for variant in native bsd; do
    env=$(variant_env $variant); [ "$env" != SKIP ] || continue
    case $variant in native) dir=share ;; bsd) dir=lib ;; esac
    variant_start $variant
    pre=; [ "$variant" = native ] || pre="PATH=/tmp/shim:\$PATH $env"
    i=$(inode /usr/local/$dir)
    r=$(verdict move bob /usr/local/$dir /usr/local/$dir.new 0 "$pre")
    now=$(ls -di /usr/local/$dir /usr/local/$dir.new 2>/dev/null | cut -d' ' -f1 | tr '\n' ' ' | sed 's/ $//')
    expect "17 $variant: whatever the answer was ($r), what is there is the folder that was, not a copy" "$i" "$now"
    if [ "$exdev" = yes ]; then
      expect "17 $variant: it was not renamed, and it was said so" "exit=1" "$r"
      yes "17 $variant: with the system's own words" "$(cat /tmp/stderr.txt)" "cross-device"
    fi
    variant_end
  done
  runtime_inode=$(inode /usr/local/runtime)
  expect "17 a folder made here, not out of a layer, is renamed" "done" "$(verdict move bob /usr/local/runtime /usr/local/runtime.new 0)"
  expect "17 it is the same folder" "$runtime_inode" "$(inode /usr/local/runtime.new)"
else
  echo "note 17: the container's root is not an overlay file system with /usr/local/{share,lib,bin}"
fi

echo "RESULT fails=$fails"
