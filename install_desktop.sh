#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY_NAME="craft-anything"
RELEASE_BIN="$SCRIPT_DIR/app/target/release/ss_rebuild"

echo "=== Building Craft Anything (Release) ==="
cargo build --release --manifest-path "$SCRIPT_DIR/app/Cargo.toml"

echo "=== Installing Desktop Integration ==="

# Check if we can write to /usr/local/bin or should use ~/.local/bin
if [ "$EUID" -eq 0 ]; then
  SUDO=""
elif command -v sudo >/dev/null 2>&1; then
  SUDO="sudo"
else
  SUDO=""
fi

if [ -n "$SUDO" ] || [ "$EUID" -eq 0 ]; then
  echo "Installing system-wide (/usr/local/bin, /usr/share/...)"
  $SUDO install -m 755 "$RELEASE_BIN" /usr/local/bin/$BINARY_NAME
  $SUDO install -d /usr/share/pixmaps
  $SUDO install -m 644 "$SCRIPT_DIR/assets/craft-anything.png" /usr/share/pixmaps/craft-anything.png 2>/dev/null || true
  $SUDO install -m 644 "$SCRIPT_DIR/assets/craft-anything.svg" /usr/share/pixmaps/craft-anything.svg 2>/dev/null || true
  $SUDO install -d /usr/share/applications
  $SUDO install -m 644 "$SCRIPT_DIR/craft-anything.desktop" /usr/share/applications/
  if [ -d /usr/share/mime/packages ]; then
    $SUDO install -m 644 "$SCRIPT_DIR/craft-anything-mime.xml" /usr/share/mime/packages/
    $SUDO update-mime-database /usr/share/mime/ 2>/dev/null || true
  fi
  update-desktop-database /usr/share/applications/ 2>/dev/null || true
else
  echo "Installing into user home (~/.local/bin, ~/.local/share/...)"
  mkdir -p "$HOME/.local/bin"
  install -m 755 "$RELEASE_BIN" "$HOME/.local/bin/$BINARY_NAME"
  mkdir -p "$HOME/.local/share/icons/hicolor/256x256/apps"
  install -m 644 "$SCRIPT_DIR/assets/craft-anything.png" "$HOME/.local/share/icons/hicolor/256x256/apps/craft-anything.png" 2>/dev/null || true
  mkdir -p "$HOME/.local/share/applications"
  install -m 644 "$SCRIPT_DIR/craft-anything.desktop" "$HOME/.local/share/applications/"
  mkdir -p "$HOME/.local/share/mime/packages"
  install -m 644 "$SCRIPT_DIR/craft-anything-mime.xml" "$HOME/.local/share/mime/packages/"
  update-mime-database "$HOME/.local/share/mime" 2>/dev/null || true
  update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
fi

# Set default MIME handlers
echo "Registering default MIME associations..."
xdg-mime default craft-anything.desktop application/x-studio 2>/dev/null || true
xdg-mime default craft-anything.desktop application/x-studio3 2>/dev/null || true
xdg-mime default craft-anything.desktop application/x-craft 2>/dev/null || true

echo "✓ Craft Anything successfully installed!"
echo "  Launch it with: craft-anything"
