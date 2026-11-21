#!/bin/bash
# Build crate as an AppImage: target/crate-<version>-x86_64.AppImage
#
# An AppImage only runs on systems at least as new as the one it was built on (glibc), so release
# builds are made on an older Ubuntu by .github/workflows/release.yml. Building on a rolling distro
# works, but the result may only run on other up-to-date systems.
#
# Nothing else is bundled: the libraries crate uses (X11/Wayland, ALSA, D-Bus, Vulkan) come with
# every desktop, and the GPU driver must be the system's own.
set -eu
cd "$(dirname "$0")/../.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' native/Cargo.toml | head -1)
ARCH=$(uname -m)
cargo build --release -p crate-native

TOOL=target/appimagetool-$ARCH.AppImage
if [ ! -x "$TOOL" ]; then
    curl -fL -o "$TOOL.part" "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"
    chmod +x "$TOOL.part" && mv "$TOOL.part" "$TOOL"
fi

APP=target/crate.AppDir
rm -rf "$APP"
install -Dm755 target/release/crate-native "$APP/usr/bin/crate"
install -Dm644 native/assets/crate.svg "$APP/usr/share/icons/hicolor/scalable/apps/crate-desktop.svg"
cp native/assets/crate.svg "$APP/crate-desktop.svg"
cat > "$APP/crate-desktop.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=crate
GenericName=Music Player
Comment=Play your music library and find new music
Exec=crate
Icon=crate-desktop
Terminal=false
Categories=AudioVideo;Audio;Player;
Keywords=music;player;navidrome;playlist;
StartupWMClass=crate-desktop
EOF
install -Dm644 "$APP/crate-desktop.desktop" "$APP/usr/share/applications/crate-desktop.desktop"
cat > "$APP/AppRun" <<'EOF'
#!/bin/sh
exec "$(dirname "$(readlink -f "$0")")/usr/bin/crate" "$@"
EOF
chmod +x "$APP/AppRun"

OUT=target/crate-$VERSION-$ARCH.AppImage
# --appimage-extract-and-run: works without FUSE (e.g. in CI containers)
ARCH=$ARCH "$TOOL" --appimage-extract-and-run --no-appstream "$APP" "$OUT" > target/appimagetool.log 2>&1 || { cat target/appimagetool.log; exit 1; }
echo "$OUT ($(du -h "$OUT" | cut -f1))"
