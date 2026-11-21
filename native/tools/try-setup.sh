#!/bin/bash
# Click through crate's first-run setup without it changing anything.
#
# usage: native/tools/try-setup.sh            everything already installed
#        native/tools/try-setup.sh missing    pretend some helper programs are missing
#
# Every action on the setup screens is pretend (nothing is downloaded, installed, signed in to or
# saved), and crate runs with a throwaway copy of your settings that's deleted when you close it.
# When you finish, it opens your real library (from that copy) so you can see where you land;
# importing is pretend too.
set -u
BIN=~/.local/bin/crate
[ -x "$BIN" ] || BIN=$(dirname "$(realpath "$0")")/../../target/release/crate-native
T=$(mktemp -d "${XDG_RUNTIME_DIR:-/tmp}/crate-setup-demo.XXXXXX")
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/cfg/crate" "$T/data"
cp ~/.config/crate/config.json ~/.config/crate/desktop-settings.json "$T/cfg/crate/" 2>/dev/null
chmod -R go-rwx "$T"
CRATE_SETUP=welcome CRATE_SETUP_DEMO=${1:-1} XDG_CONFIG_HOME="$T/cfg" XDG_DATA_HOME="$T/data" "$BIN"
