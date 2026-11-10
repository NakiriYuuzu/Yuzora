#!/bin/sh
# Runs the remote save script (/w/write.sh) and the remote read script (/w/read.sh) on Linux, as
# ordinary users, against folders and files with POSIX ACLs.
# usage (as root, in a throwaway container): remote-acl.sh [shell to run the scripts under]
SH=${1:-sh}
W=/w/write.sh
R=/w/read.sh
fails=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fails=$((fails + 1)); }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: wanted [$2] got [$3]"; fi; }
no() { # name haystack needle
  case $2 in *"$3"*) bad "$1: found [$3] in [$2]" ;; *) ok "$1" ;; esac
}
yes() {
  case $2 in *"$3"*) ok "$1" ;; *) bad "$1: wanted [$3] in [$2]" ;; esac
}

if command -v apk >/dev/null 2>&1; then
  apk add --no-cache acl >/dev/null 2>&1 || { echo "FAIL install acl"; exit 1; }
  mkuser() { adduser -D -u "$2" "$1"; }
  mkgroup() { addgroup -g "$2" "$1"; }
  join() { addgroup "$1" "$2"; }
else
  { apt-get update && apt-get install -y acl; } >/dev/null 2>&1 || { echo "FAIL install acl"; exit 1; }
  mkuser() { useradd -m -u "$2" "$1"; }
  mkgroup() { groupadd -g "$2" "$1"; }
  join() { usermod -aG "$2" "$1"; }
fi
{
  mkuser bob 1500; mkuser eve 1600
  mkgroup team 2000; mkgroup www 2001; mkgroup devs 2002
  join eve team; join bob devs
} >/dev/null 2>&1
echo "== $(. /etc/os-release; echo "$PRETTY_NAME") sh=$SH ls=$(ls --version 2>&1 | head -1 | cut -c1-30)"

as() { u=$1; shift; su -s /bin/sh "$u" -c "$*"; }
seen() { s=$(stat -L -c '%s %Y' -- "$1"); c=$(cksum < "$1" | cut -d' ' -f1); echo "$s $c"; }
acl() { getfacl -c -- "$1" 2>/dev/null | sed 's/[[:space:]]*#.*//'; }
# save AS PATH CONTENT cas|overwrite [PATH-ENV]: prints the script's stdout, then exit=N
save() {
  who=$1 f=$2 content=$3 m=$4 extra=$5
  if [ "$m" = cas ]; then set -- ${SEEN:-$(seen "$f")}; else set -- - - -; fi
  printf '%s' "$content" > /tmp/in.bin
  chmod 644 /tmp/in.bin
  len=$(wc -c < /tmp/in.bin); len=$((len))
  su -s /bin/sh "$who" -c "$extra $SH -c \"\$(cat $W)\" sh \"\$0\" \"\$1\" \"\$2\" \"\$3\" \"\$4\" \"\$5\" < /tmp/in.bin 2>/tmp/stderr.txt; echo exit=\$?" "$f" "$m" "$1" "$2" "$3" "$len"
}
verdict() { save "$@" | sed -n 1p; }
flag() { su -s /bin/sh "$1" -c "$SH -c \"\$(cat $R)\" sh \"\$0\" 100 99 | sed -n 1p" "$2"; }
leftovers() { find "$1" -name '.yuzora-save-*' 2>/dev/null | wc -l | tr -d ' '; }
reset() { rm -rf "$1"; mkdir -p "$1"; chown bob:bob "$1"; chmod 755 "$1"; }
hide() { mkdir -p /hidden; for t in "$@"; do mv "$(command -v "$t")" /hidden/; done; }
unhide() { for t in /hidden/*; do mv "$t" "/usr/bin/${t##*/}"; done; }
scratch_up() { # FOLDER: waits until a save has made its scratch file there
  n=0
  while [ -z "$(find "$1" -name '.yuzora-save-*' 2>/dev/null)" ] && [ "$n" -lt 50 ]; do sleep 0.2; n=$((n + 1)); done
}
mkdir -p /tmp/shim
printf '#!/bin/sh\n/bin/cat "$@"; sleep 4\n' > /tmp/shim/cat
chmod 755 /tmp/shim/cat

echo "-- 1. a folder's default ACL does not widen a file that had none (640 stays out of reach of the folder's group)"
reset /tmp/s1
as bob 'printf "token=abc\n" > /tmp/s1/notes.txt; chmod 640 /tmp/s1/notes.txt'
setfacl -d -m g:team:rw /tmp/s1
expect "1 eve cannot read before" "denied" "$(as eve 'cat /tmp/s1/notes.txt >/dev/null 2>&1 && echo read || echo denied')"
yes "1 saved" "$(verdict bob /tmp/s1/notes.txt 'token=xyz
' cas)" "saved"
expect "1 content" "token=xyz" "$(cat /tmp/s1/notes.txt)"
expect "1 mode" "640" "$(stat -c %a /tmp/s1/notes.txt)"
expect "1 eve cannot read after" "denied" "$(as eve 'cat /tmp/s1/notes.txt >/dev/null 2>&1 && echo read || echo denied')"
no "1 no entry for the folder's group on the file" "$(acl /tmp/s1/notes.txt)" "team"
expect "1 nothing left" "0" "$(leftovers /tmp/s1)"

echo "-- 2. nobody but the owner can open the scratch file while it is written (a 600 file, a folder the team writes in)"
join bob team >/dev/null 2>&1
reset /tmp/s2
chown bob:team /tmp/s2; chmod 2770 /tmp/s2
as bob 'printf "SECRET=1\n" > /tmp/s2/secret.env; chmod 600 /tmp/s2/secret.env'
setfacl -d -m g:team:rw /tmp/s2
( save bob /tmp/s2/secret.env 'SECRET=2
' cas "PATH=/tmp/shim:\$PATH" > /tmp/s2.out ) &
scratch_up /tmp/s2
as eve 'for p in /tmp/s2/.yuzora-save-*; do [ -e "$p" ] || continue; echo "found"; (exec 3<>"$p") 2>/dev/null && echo "OPENED $p"; q="$p/${p##*/}"; (exec 3<>"$q") 2>/dev/null && echo "OPENED $q"; (exec 3<"$q") 2>/dev/null && echo "READ $q"; done; echo done' > /tmp/s2.eve
wait
yes "2 eve saw the scratch name" "$(cat /tmp/s2.eve)" "found"
no "2 eve opened nothing" "$(cat /tmp/s2.eve)" "OPENED"
no "2 eve read nothing" "$(cat /tmp/s2.eve)" "READ"
yes "2 saved" "$(sed -n 1p /tmp/s2.out)" "saved"
expect "2 mode" "600" "$(stat -c %a /tmp/s2/secret.env)"
expect "2 eve cannot read after" "denied" "$(as eve 'cat /tmp/s2/secret.env >/dev/null 2>&1 && echo read || echo denied')"
expect "2 nothing left" "0" "$(leftovers /tmp/s2)"

echo "-- 2b. the same where no ACL tool can take the folder's default ACL off the scratch file: refused, and still nobody gets in meanwhile"
reset /tmp/s2b
chown bob:team /tmp/s2b; chmod 2770 /tmp/s2b
as bob 'printf "SECRET=1\n" > /tmp/s2b/secret.env; chmod 600 /tmp/s2b/secret.env'
setfacl -d -m g:team:rw /tmp/s2b
hide getfacl setfacl
( save bob /tmp/s2b/secret.env 'SECRET=2
' cas "PATH=/tmp/shim:\$PATH" > /tmp/s2b.out ) &
if ls --version >/dev/null 2>&1; then sleep 1; else scratch_up /tmp/s2b; fi
as eve 'for p in /tmp/s2b/.yuzora-save-*; do [ -e "$p" ] || continue; echo "found"; (exec 3<>"$p") 2>/dev/null && echo "OPENED $p"; q="$p/${p##*/}"; (exec 3<>"$q") 2>/dev/null && echo "OPENED $q"; (exec 3<"$q") 2>/dev/null && echo "READ $q"; done; echo done' > /tmp/s2b.eve
wait
unhide
if ls --version >/dev/null 2>&1; then
  echo "note 2b: this ls shows the folder's ACL, so the save is refused before it makes a scratch file"
else
  yes "2b eve saw the scratch name" "$(cat /tmp/s2b.eve)" "found"
  no "2b eve opened nothing" "$(cat /tmp/s2b.eve)" "OPENED"
  no "2b eve read nothing" "$(cat /tmp/s2b.eve)" "READ"
fi
expect "2b refused" "refused acl" "$(sed -n 1p /tmp/s2b.out)"
expect "2b file untouched" "SECRET=1" "$(cat /tmp/s2b/secret.env)"
expect "2b nothing left" "0" "$(leftovers /tmp/s2b)"

echo "-- 3. a file with an ACL of its own keeps it"
reset /tmp/s3
as bob 'printf "a\n" > /tmp/s3/f; chmod 660 /tmp/s3/f'
setfacl -m g:team:rw,u:eve:r /tmp/s3/f
before=$(acl /tmp/s3/f)
yes "3 saved" "$(verdict bob /tmp/s3/f 'b
' cas)" "saved"
expect "3 same ACL" "$before" "$(acl /tmp/s3/f)"
expect "3 mode" "660" "$(stat -c %a /tmp/s3/f)"
expect "3 eve can still read" "b" "$(as eve 'cat /tmp/s3/f')"

echo "-- 4. the owning group is not given what the mask allows"
reset /tmp/s4
as bob 'printf "a\n" > /tmp/s4/g; chmod 600 /tmp/s4/g'
setfacl -m u:eve:rw /tmp/s4/g
before=$(acl /tmp/s4/g)
yes "4 group entry before" "$before" "group::---"
yes "4 saved" "$(verdict bob /tmp/s4/g 'b
' cas)" "saved"
expect "4 same ACL" "$before" "$(acl /tmp/s4/g)"

echo "-- 5. a new file made by a forced save has no ACL of the folder's and the mode a new file gets"
reset /tmp/s5
setfacl -d -m g:team:rw /tmp/s5
rm -f /tmp/s5/new
yes "5 saved" "$(verdict bob /tmp/s5/new 'made
' overwrite)" "saved"
expect "5 content" "made" "$(cat /tmp/s5/new)"
no "5 no entry for the folder's group" "$(acl /tmp/s5/new)" "team"

echo "-- 6. no ACL tools there: a clean folder still saves, a folder with a default ACL is refused"
reset /tmp/s6a
as bob 'printf "a\n" > /tmp/s6a/f; chmod 640 /tmp/s6a/f'
reset /tmp/s6b
as bob 'printf "a\n" > /tmp/s6b/f; chmod 640 /tmp/s6b/f'
setfacl -d -m g:team:rw /tmp/s6b
reset /tmp/s6c
as bob 'printf "a\n" > /tmp/s6c/f; chmod 640 /tmp/s6c/f'
hide getfacl setfacl
yes "6a clean folder saved" "$(verdict bob /tmp/s6a/f 'b
' cas)" "saved"
expect "6a mode" "640" "$(stat -c %a /tmp/s6a/f)"
expect "6b default ACL refused" "refused acl" "$(verdict bob /tmp/s6b/f 'b
' cas)"
expect "6b file untouched" "a" "$(cat /tmp/s6b/f)"
expect "6b nothing left" "0" "$(leftovers /tmp/s6b)"
if ls --version >/dev/null 2>&1; then
  expect "6b reads as read-only (ls shows the folder's ACL)" "r" "$(flag bob /tmp/s6b/f | cut -d' ' -f3)"
else
  echo "note 6b: this ls shows no ACL, so the folder's default ACL is found out by the save, not by the read"
fi
expect "6b eve cannot read" "denied" "$(as eve 'cat /tmp/s6b/f >/dev/null 2>&1 && echo read || echo denied')"
unhide
echo "-- 6d. getfacl without setfacl is no better"
setfacl -d -m g:team:rw /tmp/s6c
hide setfacl
expect "6d refused" "refused acl" "$(verdict bob /tmp/s6c/f 'b
' cas)"
expect "6d file untouched" "a" "$(cat /tmp/s6c/f)"
unhide

echo "-- 7. what opening calls editable, a save does not refuse for a reason that was there at open"
reset /tmp/s7
as bob 'printf "a\n" > /tmp/s7/own; printf "a\n" > /tmp/s7/devs; printf "a\n" > /tmp/s7/www'
chown bob:devs /tmp/s7/devs; chown bob:www /tmp/s7/www; chmod 664 /tmp/s7/devs /tmp/s7/www
printf 'a\n' > /tmp/s7/eves; chown eve:eve /tmp/s7/eves; chmod 666 /tmp/s7/eves
expect "7 own file: editable" "w" "$(flag bob /tmp/s7/own | cut -d' ' -f3)"
yes "7 own file: saved" "$(verdict bob /tmp/s7/own 'b
' cas)" "saved"
expect "7 a group I am in: editable" "w" "$(flag bob /tmp/s7/devs | cut -d' ' -f3)"
yes "7 a group I am in: saved" "$(verdict bob /tmp/s7/devs 'b
' cas)" "saved"
expect "7 a group I am in: kept" "bob:devs" "$(stat -c %U:%G /tmp/s7/devs)"
expect "7 a group I am not in: read-only" "r" "$(flag bob /tmp/s7/www | cut -d' ' -f3)"
expect "7 a group I am not in: refused as such" "refused group" "$(verdict bob /tmp/s7/www 'b
' cas)"
expect "7 a group I am not in: untouched" "a" "$(cat /tmp/s7/www)"
expect "7 another user's file: read-only" "r" "$(flag bob /tmp/s7/eves | cut -d' ' -f3)"
expect "7 another user's file: refused as such" "refused owner" "$(verdict bob /tmp/s7/eves 'b
' cas)"
expect "7 nothing left" "0" "$(leftovers /tmp/s7)"

echo "-- 8. a folder on the way that cannot be entered is an error, not a deleted file"
reset /tmp/s8
mkdir /tmp/s8/sub; chown bob:bob /tmp/s8/sub
as bob 'printf "old\n" > /tmp/s8/sub/f'
held=$(seen /tmp/s8/sub/f)
chmod 000 /tmp/s8/sub
out=$(save bob /tmp/s8/sub/f 'mine
' cas)
no "8 not gone" "$out" "conflict gone"
yes "8 an error" "$out" "exit=1"
yes "8 says why" "$(cat /tmp/stderr.txt)" "cannot look inside"
chmod 755 /tmp/s8/sub
expect "8 untouched" "old" "$(cat /tmp/s8/sub/f)"
rm /tmp/s8/sub/f
expect "8 really gone" "conflict gone" "$(SEEN=$held verdict bob /tmp/s8/sub/f 'mine
' cas)"

echo "RESULT fails=$fails"
