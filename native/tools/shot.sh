#!/bin/bash
# Run crate-native inside a private, invisible KWin (virtual output) and screenshot it.
# Nothing appears on the real desktop and the screenshot can only contain crate.
#
# usage: tools/shot.sh WIDTH HEIGHT OUT.png [wait-seconds]
# env:   QUEUE=1        open with the queue panel
#        VIEW=home | VIEW='{"type":"libAlbum","id":"…"}'  page to open; LIBTAB=songs|albums|artists
#        CRATE_BENCH=f  run the scroll benchmark and write stats to f
#        CRATE_DEMO_EDIT=1  playlist-editing self-test on a throwaway playlist (use AFTER=12)
#        CRATE_DEMO_PLAY=1  start playing (muted) after 3 s
#        AFTER=s        keep the app running this long after the screenshot
#        SRC=dir        use dir/cfg, dir/data and dir/cache instead of your real config (tools/demo-shots.sh)
# Uses copies of the real config (player muted) in $XDG_RUNTIME_DIR/crate-test, deleted afterwards.
set -u
W=$1; H=$2; OUT=$(realpath -m "$3"); WAIT=${4:-6}
BIN=$(dirname "$(realpath "$0")")/../../target/release/crate-native
T=${XDG_RUNTIME_DIR:-/tmp}/crate-test; rm -rf "$T"; mkdir -p "$T/cfg/crate" "$T/data/crate"; chmod 700 "$T"
SRC=${SRC:-}
if [ -n "$SRC" ]; then cp "$SRC"/cfg/crate/*.json "$T/cfg/crate/"; CACHE="XDG_CACHE_HOME=$SRC/cache"
else cp ~/.config/crate/config.json ~/.config/crate/desktop-settings.json "$T/cfg/crate/"; CACHE=; fi
python3 - "$T" "$SRC" <<'PY'
import json, os, sys
t, src = sys.argv[1], sys.argv[2]
f = f"{t}/cfg/crate/desktop-settings.json"; d = json.load(open(f))
d["panel"] = "queue" if os.environ.get("QUEUE") else os.environ.get("PANEL")
v = os.environ.get("VIEW")
if v == "home": d["lastView"] = {"type": "home"}
elif v: d["lastView"] = json.loads(v)
if os.environ.get("LIBTAB"): d["libTab"] = os.environ["LIBTAB"]
if os.environ.get("SET"): d.update(json.loads(os.environ["SET"]))  # e.g. SET='{"theme":"light"}'
if os.environ.get("HIST"): d["searchHistory"] = os.environ["HIST"].split("|"); d["searchQ"] = ""
json.dump(d, open(f, "w"))
if os.environ.get("JOBS"):  # finished downloads only, so nothing re-runs in the test
    try:
        j = json.load(open(os.path.expanduser("~/.local/share/crate/desktop-jobs.json")))
        json.dump([x for x in j if x.get("status") in ("done", "failed")], open(f"{t}/data/crate/desktop-jobs.json", "w"))
    except Exception: pass
try:
    p = json.load(open(f"{src}/data/crate/player.json" if src else os.path.expanduser("~/.local/share/crate/player.json")))
    p["muted"] = not src  # the demo library is silent anyway
    p["scrobble"] = False  # never report test plays to Navidrome
    if os.environ.get("QIDX"): p["index"] = int(os.environ["QIDX"])
    json.dump(p, open(f"{t}/data/crate/player.json", "w"))
except Exception: pass
PY
cat > "$T/inner.sh" <<IN
#!/bin/bash
CRATE_DEMO_SEEK=${CRATE_DEMO_SEEK:-} CRATE_DEMO_MENU=${CRATE_DEMO_MENU:-} CRATE_DEMO_EDIT=${CRATE_DEMO_EDIT:-} CRATE_DEMO_PLAY=${CRATE_DEMO_PLAY:-} CRATE_DEMO_FIX=${CRATE_DEMO_FIX:-} CRATE_DEMO_SCROLL=${CRATE_DEMO_SCROLL:-} CRATE_BENCH=${CRATE_BENCH:-} CRATE_BENCH_STAY=${CRATE_BENCH_STAY:-} $CACHE XDG_CONFIG_HOME=$T/cfg XDG_DATA_HOME=$T/data CRATE_WINDOW=${W}x$((H-40)) ${EXTRA_ENV:-} "$BIN" > $T/app.log 2>&1 &
sleep $WAIT
spectacle -b -n -f -o "$OUT" > $T/spec.log 2>&1
sleep ${AFTER:-1}; kill %1 2>/dev/null
IN
sed -i -e "s/^CRATE_DEMO_SEEK= //" -e "s/CRATE_DEMO_MENU= //" -e "s/CRATE_BENCH= //" -e "s/CRATE_BENCH_STAY= //" -e "s/CRATE_DEMO_EDIT= //" -e "s/CRATE_DEMO_PLAY= //" -e "s/CRATE_DEMO_FIX= //" -e "s/CRATE_DEMO_SCROLL= //" "$T/inner.sh"
chmod +x "$T/inner.sh"
timeout 120 dbus-run-session kwin_wayland --virtual --width "$W" --height "$H" --socket crate-test-$$ --exit-with-session "$T/inner.sh" > "$T/kwin.log" 2>&1
grep -v -e '^$' -e radv -e SystemUIFont "$T/app.log" | head -20
rm -rf "$T/cfg"   # holds the Navidrome token
ls -la "$OUT" 2>/dev/null || echo "no screenshot (see $T/kwin.log)"
