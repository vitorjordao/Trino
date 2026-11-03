#!/bin/sh
# Two Azahar instances in one Azahar multiplayer room (issue #117): the 3DS
# local-wireless (`uds`) check. Run in a container with Xvfb, openbox,
# xdotool and ImageMagick (`cargo xtask net 3ds` starts one from the CI's
# 3DS image); the clicks are for a 1600x900 screen and Azahar 2126.2.
#
# 1. Instance A boots the host build and creates an unlisted room on
#    port 24872 (Multiplayer > Create Room).
# 2. Instance B first starts without a game to rename its console (Emulation
#    > Configure > System > Username, and a new MAC): a room refuses a
#    second "AZAHAR", the name every fresh Azahar has. It then boots the join
#    build and direct-connects to 127.0.0.1.
# 3. Azahar carries the two consoles' `uds` frames through the room; the
#    apps log `TRINO_UDS:` lines and `TRINO_NET_HASH:<frame>:<hash>`.
#
# Each instance has its own HOME/XDG dirs (their logs and configs never mix).
# Usage: two-azahar.sh <azahar> <host.3dsx> <join.3dsx> <out dir> [seconds]
set -u
AZAHAR=$1 HOST=$2 JOIN=$3 OUT=$4 SECS=${5:-120}
mkdir -p "$OUT"
export DISPLAY=:99
Xvfb :99 -screen 0 1600x900x24 >/dev/null 2>&1 &
sleep 2
openbox >/dev/null 2>&1 &
sleep 1

seed() { # <dir>
  mkdir -p "$1/config/azahar-emu" "$1/data"
  cat > "$1/config/azahar-emu/qt-config.ini" <<'INI'
[Debugging]
instant_debug_log=true
instant_debug_log\default=false

[Miscellaneous]
check_for_update_on_start=false
check_for_update_on_start\default=false
log_filter=*:Info Debug.Emulated:Trace Network:Trace
log_filter\default=false

[UI]
confirmClose=false
confirmClose\default=false
Multiplayer\room_name=trino
Multiplayer\room_name\default=false
INI
}

shot() { import -window root "$OUT/$1.png" 2>/dev/null; }
click() { xdotool mousemove "$1" "$2" click 1; sleep "${3:-1}"; }
log_of() { echo "$1/data/azahar-emu/log/azahar_log.txt"; }

seed /tmp/a
seed /tmp/b
# A: boot the host build, create an unlisted room.
HOME=/tmp/a XDG_CONFIG_HOME=/tmp/a/config XDG_DATA_HOME=/tmp/a/data \
  "$AZAHAR" "$HOST" >/tmp/a/stdout.log 2>&1 &
PA=$!
sleep 9
click 463 211          # Multiplayer
click 492 254 2        # Create Room
shot 01-create-room
click 965 544          # Public/Unlisted
click 960 567          # Unlisted
click 1051 544 4       # Host Room
shot 02-hosting
# B: rename the console (no game running), then boot the join build.
HOME=/tmp/b XDG_CONFIG_HOME=/tmp/b/config XDG_DATA_HOME=/tmp/b/data \
  "$AZAHAR" >/tmp/b/stdout.log 2>&1 &
PB=$!
sleep 7
click 342 211          # Emulation
click 370 346 3        # Configure...
click 415 81 2         # System
xdotool mousemove 1010 294 click --repeat 3 1
xdotool key ctrl+a
xdotool type TRINOB
click 1131 637         # MAC: Regenerate
click 910 414          # Yes
shot 03-b-renamed
click 1082 744 2       # OK
kill -9 "$PB"
sleep 2
HOME=/tmp/b XDG_CONFIG_HOME=/tmp/b/config XDG_DATA_HOME=/tmp/b/data \
  "$AZAHAR" "$JOIN" >/tmp/b/stdout.log 2>&1 &
PB=$!
sleep 9
click 463 211          # Multiplayer
click 525 276 2        # Direct Connect to Room
click 797 387 0
xdotool type 127.0.0.1
shot 04-direct-connect
click 975 504 8        # Connect
shot 05-connected
i=0
while [ $i -lt "$SECS" ]; do
  sleep 5
  i=$((i + 5))
  if grep -q TRINO_NET_HASH "$(log_of /tmp/a)" 2>/dev/null &&
     grep -q TRINO_NET_HASH "$(log_of /tmp/b)" 2>/dev/null; then
    break
  fi
done
# Both windows side by side.
i=0
for w in $(xdotool search --onlyvisible --name "Azahar 2126"); do
  xdotool windowsize "$w" 800 600
  xdotool windowmove "$w" $((i * 800)) 120
  i=$((i + 1))
done
for w in $(xdotool search --name "connected") $(xdotool search --name "Direct Connect"); do
  xdotool windowmove "$w" 1700 1000
done
sleep 2
shot 06-side-by-side
cp "$(log_of /tmp/a)" "$OUT/a.log" 2>/dev/null
cp "$(log_of /tmp/b)" "$OUT/b.log" 2>/dev/null
kill -9 "$PA" "$PB" 2>/dev/null
grep -h "TRINO_UDS\|TRINO_NET_HASH" "$OUT/a.log" "$OUT/b.log"
