#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY_NAME="studio-to-svg"
RELEASE_BIN="$SCRIPT_DIR/app/target/release/studio-to-svg"

echo "=== Building Studio to SVG (Release) ==="
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
  # Clean up legacy craft-anything references if present
  $SUDO rm -f /usr/local/bin/craft-anything /usr/share/applications/craft-anything.desktop /usr/share/mime/packages/craft-anything-mime.xml /usr/share/pixmaps/craft-anything.png /usr/share/pixmaps/craft-anything.svg 2>/dev/null || true

  $SUDO install -m 755 "$RELEASE_BIN" /usr/local/bin/$BINARY_NAME
  $SUDO install -d /usr/share/pixmaps
  $SUDO install -m 644 "$SCRIPT_DIR/assets/studio-to-svg.png" /usr/share/pixmaps/studio-to-svg.png 2>/dev/null || true
  $SUDO install -m 644 "$SCRIPT_DIR/assets/studio-to-svg.svg" /usr/share/pixmaps/studio-to-svg.svg 2>/dev/null || true
  $SUDO install -d /usr/share/applications
  $SUDO install -m 644 "$SCRIPT_DIR/studio-to-svg.desktop" /usr/share/applications/
  if [ -d /usr/share/mime/packages ]; then
    $SUDO install -m 644 "$SCRIPT_DIR/studio-to-svg-mime.xml" /usr/share/mime/packages/
    $SUDO update-mime-database /usr/share/mime/ 2>/dev/null || true
  fi
  update-desktop-database /usr/share/applications/ 2>/dev/null || true
else
  echo "Installing into user home (~/.local/bin, ~/.local/share/...)"
  # Clean up legacy craft-anything references if present
  rm -f "$HOME/.local/bin/craft-anything" "$HOME/.local/share/applications/craft-anything.desktop" "$HOME/.local/share/mime/packages/craft-anything-mime.xml" "$HOME/.local/share/icons/hicolor/256x256/apps/craft-anything.png" 2>/dev/null || true

  mkdir -p "$HOME/.local/bin"
  install -m 755 "$RELEASE_BIN" "$HOME/.local/bin/$BINARY_NAME"
  mkdir -p "$HOME/.local/share/icons/hicolor/256x256/apps"
  install -m 644 "$SCRIPT_DIR/assets/studio-to-svg.png" "$HOME/.local/share/icons/hicolor/256x256/apps/studio-to-svg.png" 2>/dev/null || true
  mkdir -p "$HOME/.local/share/applications"
  install -m 644 "$SCRIPT_DIR/studio-to-svg.desktop" "$HOME/.local/share/applications/"
  mkdir -p "$HOME/.local/share/mime/packages"
  install -m 644 "$SCRIPT_DIR/studio-to-svg-mime.xml" "$HOME/.local/share/mime/packages/"
  update-mime-database "$HOME/.local/share/mime" 2>/dev/null || true
  update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
fi

# Set default MIME handlers
echo "Registering default MIME associations..."
xdg-mime default studio-to-svg.desktop application/x-studio 2>/dev/null || true
xdg-mime default studio-to-svg.desktop application/x-studio3 2>/dev/null || true
xdg-mime default studio-to-svg.desktop application/x-craft 2>/dev/null || true

echo "✓ Studio to SVG successfully installed!"
echo "  Launch it with: studio-to-svg"
