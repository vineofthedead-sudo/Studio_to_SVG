# Silhouette Studio Linux Rebuild (`Craft Anything` / `ss_rebuild`)

A lightweight, native Linux replacement for Silhouette Studio built in Rust with `egui`, `tiny-skia`, `usvg`, and `resvg`. It provides an interactive cutting mat canvas for preparing, editing, previewing, and exporting vinyl cutting and scrapbooking designs.

---

## Features

- **Cutting Mat Canvas**:
  - Customizable mat dimensions (4 in to 24 in) with realistic 1-inch grid markers and registration borders.
  - Smooth pan (Middle/Right mouse drag) and zoom (Scroll wheel) at a steady 60 FPS.
- **Native `.craft` Project Format**:
  - Versioned project format preserving complete workspace state: mat size, paper background, all vector/raster/text shapes, fill/stroke styles, group hierarchies, visibility, lock states, and Print & Cut registration marks.
  - Dependency-free base64 PNG embedding for 100% fidelity persistence of embedded raster graphics.
  - `Ctrl+S` (Save), `Ctrl+Shift+S` (Save As), and `Ctrl+O` (Open).
- **Real Vector Crafting & Path Operations (`vector_ops.rs`)**:
  - **Offset Path (Sticker / Cardstock Mat Border)**: Generate parallel inward or outward offset contours around any shape with interactive distance slider (-0.25" to +0.50")—essential for sticker borders and layered cardstock mats.
  - **Compound Paths (Make / Release)**: Combine multiple shapes into a single `EvenOdd` compound path with inner cutouts, or split compound paths back into individual editable subpaths.
  - **Weld**: Union overlapping shapes into a single unified cut outline using convex hull geometry.
  - **Align & Distribute**: One-click alignment buttons (`Align Left`, `Center H`, `Align Right`, `Align Top`, `Center V`, `Align Bottom`, `Center to Mat`) and horizontal/vertical distribution for multi-selected shapes.
- **Rich Typography & Text-on-Canvas**:
  - System TrueType/OpenType font discovery and rendering (`ab_glyph`) across system font directories (`/usr/share/fonts`).
  - Font family dropdown (Liberation Sans, Liberation Serif, Liberation Mono, Noto Sans, JetBrains Mono).
  - Font size (pt), Bold, Italic, and letter spacing controls.
  - **"✂ Convert Text to Cut Paths"**: Converts live text into editable vector cut shapes ready for vinyl and craft cutters.
- **Print & Cut Registration Marks**:
  - Silhouette/Cricut standard corner registration marks (top-left filled square, top-right and bottom-left L-brackets) with adjustable inset, mark size, and line thickness.
  - Rendered live on the cutting mat and exported cleanly into both SVG and high-DPI PNG outputs.
- **Legacy Silhouette Studio Format Parser (`.studio` / `.studio3`)**:
  - Direct binary import and conversion of proprietary Silhouette Studio `.studio` (`silhouette04;`) and `.studio3` (`silhouette05;`) files on Linux without Windows or Wine.
  - Automatically identifies format headers and offsets (`0x0000` for v4, `0x0400` with section offset tables for v5).
  - Isolates vector document streams and decodes embedded PNG print-and-cut artwork (`89 50 4E 47`) underneath vector cut lines.
  - Zero-panic architecture: safe bounds checking and memory access across all stream offsets with zero naked unwraps.
  - Parses both 34-byte 2-handle (`01 21 00 00 00`) and 26-byte 1-handle (`01 19 00 00 00`) cubic Bézier vertex records, 2D affine transforms `[a, b, tx, c, d, ty]`, and RGBA cut/fill colors.
  - True physical unit scaling: converts Silhouette Studio native millimeters to inches (`1.0 / 25.4`) so designs appear at 100% true physical size on the mat.
  - Compound paths & hole punching: detects concentric same-color contours (e.g. wheels, cutouts) and punches holes cleanly using `FillRule::EvenOdd`.
  - Flattens Bézier curves via adaptive de Casteljau subdivision into smooth canvas polylines.
  - **Batch Converter**: One-click folder batch conversion preserving both vector paths and embedded raster layers, exporting clean `.svg` files for entire directories with status reporting.
  - **CLI File Loading**: Direct command-line file loading and headless verification via `ss_rebuild <file> --screenshot <path>`.
- **High-Performance SVG Import**:
  - Parses complex multi-layer commercial SVG cutting files with `usvg` 0.45.
  - Renders exact SVG styles, CSS classes, linear/radial gradients, and `FillRule::EvenOdd` cutouts into a cached `egui::TextureHandle` via `resvg` 0.45.
  - Precomputes polygon mesh triangles to avoid per-frame CPU ear-clipping lag.
- **Grouping & Multi-Selection**:
  - Auto-groups imported SVG files under a shared `group_id` for easy whole-design manipulation.
  - Group (`Ctrl+G`) and Ungroup (`Ctrl+Shift+G`) shortcuts and menu actions.
  - Group by Color: automatically separates multi-color designs into cardstock cut layers.
  - `Shift+Click` and marquee drag-box selection.
  - Corner resize handles for interactive scaling with aspect ratio preservation.
- **Layers Panel**:
  - Top-to-bottom z-order layer list in the right sidebar.
  - Reordering tools: Bring to Front, Bring Forward, Send Backward, Send to Back.
  - Visibility eye toggle (`is_visible`) and Lock toggle (`is_locked`).
  - Shape color swatches and click-to-select integration.
- **Custom Background Paper**:
  - Import custom digital scrapbooking / cardstock paper patterns (PNG/JPG) under Page Setup.
  - Stretches paper across the mat for visual layout previews without polluting SVG cut lines.
- **Export & Project Persistence**:
  - Save and open native `.craft` projects and legacy `.json` files.
  - Export machine-ready cut paths as SVG in mat-inch coordinates.
  - Export high-DPI composited raster previews as PNG (e.g. 150 DPI).
  - Headless `--screenshot <path>` argument for automated rendering and CI.

---

## Supported File Formats

| Format | Read | Write | Description / Capabilities |
| :--- | :---: | :---: | :--- |
| `.craft` | ✓ | ✓ | **Native Project Format**: Versioned JSON with base64 embedded rasters, shapes, layers, and registration marks. |
| `.studio` | ✓ | — | Silhouette Studio v4 binary (`silhouette04;`) with cubic Bézier knots and 2D affine transforms. |
| `.studio3` | ✓ | — | Silhouette Studio v5 binary (`silhouette05;`) with section tables, embedded PNG artwork, and cut lines. |
| `.svg` | ✓ | ✓ | Standard Scalable Vector Graphics for cutters, plotters, Inkscape, Cricut Design Space, etc. |
| `.png` | — | ✓ | High-DPI raster preview export with cutting mat grid and registration marks. |

---

## Desktop Integration (Linux)

To install Craft Anything as a first-class Linux desktop application:

```bash
./install_desktop.sh
```

This will:
1. Build the optimized release binary (`ss_rebuild`).
2. Install the executable as `/usr/local/bin/craft-anything`.
3. Install high-resolution vector and raster icons (`assets/craft-anything.svg` and `craft-anything.png`).
4. Install `craft-anything.desktop` into `/usr/share/applications/` (or `~/.local/share/applications/`).
5. Register MIME type associations for `.craft`, `.studio`, `.studio3`, and `.svg` files.

---

## Building and Running

### Prerequisites

Ensure you have Rust (edition 2021) and standard Linux graphics/windowing libraries installed:

```bash
# Ubuntu / Debian
sudo apt-get install -y build-essential pkg-config libfontconfig1-dev libx11-dev libwayland-dev

# Fedora / RHEL
sudo dnf install -y fontconfig-devel libX11-devel wayland-devel

# Arch Linux
sudo pacman -S base-devel fontconfig libx11 wayland
```

### Build & Run

```bash
# Clone the repository
git clone https://github.com/vineofthedead-sudo/silhouette-studio-linux.git
cd silhouette-studio-linux

# Run release binary
cargo run --release --manifest-path app/Cargo.toml
```

### Running Automated Tests

```bash
cargo test --manifest-path app/Cargo.toml
```

### Headless Screenshot Verification

To capture a high-resolution screenshot without a display server (e.g. in CI or SSH):

```bash
xvfb-run -a -s "-screen 0 1280x800x24" ./app/target/release/ss_rebuild "test_samples/Peeker Bears.studio" --screenshot /tmp/screenshot.png
```
