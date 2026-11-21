#!/bin/bash
# Screenshots for the README, from a made-up library on a throwaway Navidrome (nothing of yours is
# used or shown). Everything happens off-screen (tools/shot.sh); pictures go to docs/screenshots/.
# The fake library is kept in ~/.cache/crate-demo so later runs are quick.
set -eu
cd "$(dirname "$0")/../.."
D=~/.cache/crate-demo
OUT=docs/screenshots
mkdir -p "$OUT"
python3 native/tools/demo/make-library.py "$D/Music"
cargo build -q --release -p crate-native
cargo build -q --release -p crate-core --example demo_server

rm -rf "$D/cfg" "$D/data" "$D/cache"
mkdir -p "$D/cfg/crate" "$D/data/crate" "$D/cache"
echo '{}' > "$D/cfg/crate/desktop-settings.json"
mkfifo "$D/hold"
XDG_CONFIG_HOME="$D/cfg" XDG_DATA_HOME="$D/data" target/release/examples/demo_server "$D" "$D/Music" < "$D/hold" > "$D/server.log" 2>&1 &
exec 3> "$D/hold"   # the server runs until this closes
trap 'exec 3>&-; wait; rm -f "$D/hold"' EXIT
until grep -q -e ready -e Error "$D/server.log"; do sleep 1; done
cat "$D/server.log"
grep -q ready "$D/server.log"

pl() { python3 -c "import json,sys; print(json.load(open('$D/ids.json'))['$1'][sys.argv[1]])" "$2"; }
shot() { SRC=$D CRATE_DEMO_PLAY=1 native/tools/shot.sh 1600 1000 "$OUT/$1.png" 8 > /dev/null; echo "$OUT/$1.png"; }

QUEUE=1 VIEW="{\"type\":\"playlist\",\"id\":\"$(pl playlists 'late night drive')\"}" shot playlist
VIEW="{\"type\":\"libAlbum\",\"id\":\"$(pl albums 'In Rainbows')\"}" shot album
SET='{"searchQ":"Radiohead","searchKind":"track"}' VIEW='{"type":"search"}' shot search
SET='{"theme":"light"}' VIEW="{\"type\":\"playlist\",\"id\":\"$(pl playlists 'dance in the kitchen')\"}" shot light
VIEW=home shot home
