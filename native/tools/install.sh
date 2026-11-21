#!/bin/bash
# Build Crate and install it for your user: ~/.local/bin/crate, its icon, and a launcher entry.
# The program is copied (not linked), so a half-finished build never replaces the app you're using.
set -e
cd "$(dirname "$0")/.."
cargo build --release
install -Dm755 ../target/release/crate-native ~/.local/bin/crate
install -Dm644 assets/crate.svg ~/.local/share/icons/hicolor/scalable/apps/crate-desktop.svg
# fixed-size copies from older installs would be used instead of the SVG
rm -f ~/.local/share/icons/hicolor/*x*/apps/crate-desktop.png
touch ~/.local/share/icons/hicolor   # tells desktops their icon cache is stale
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
command -v kbuildsycoca6 > /dev/null && kbuildsycoca6 > /dev/null 2>&1 || true
command -v update-desktop-database > /dev/null && update-desktop-database ~/.local/share/applications 2>/dev/null || true
echo "installed ~/.local/bin/crate (find \"Crate\" in your app launcher)"
