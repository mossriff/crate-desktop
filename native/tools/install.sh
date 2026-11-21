#!/bin/bash
# Build Crate and install it for your user: ~/.local/bin/crate, its icon, and a launcher entry.
# The program is copied (not linked), so a half-finished build never replaces the app you're using.
set -e
cd "$(dirname "$0")/.."
cargo build --release
install -Dm755 ../target/release/crate-native ~/.local/bin/crate
install -Dm644 assets/crate.svg ~/.local/share/icons/hicolor/scalable/apps/crate-desktop.svg
mkdir -p ~/.local/share/applications
cat > ~/.local/share/applications/crate-desktop.desktop <<EOF
[Desktop Entry]
Type=Application
Name=Crate
GenericName=Music Player
Comment=Play your music library and find new music
Exec=$HOME/.local/bin/crate
Icon=crate-desktop
Terminal=false
Categories=AudioVideo;Audio;Player;Music;
Keywords=music;player;navidrome;playlist;
StartupWMClass=crate-desktop
EOF
echo "installed ~/.local/bin/crate (find \"Crate\" in your app launcher)"
