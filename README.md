# Silhouette Studio Linux Rebuild (`ss_rebuild`)

A lightweight, native Linux replacement for Silhouette Studio built in Rust with `egui`, `tiny-skia`, `usvg`, and `resvg`. It provides an interactive cutting mat canvas for preparing, editing, previewing, and exporting vinyl cutting and scrapbooking designs.

---

## Features

- **Cutting Mat Canvas**:
  - Customizable mat dimensions (4 in to 24 in) with realistic 1-inch grid markers and registration borders.
  - Smooth pan (Middle/Right mouse drag) and zoom (Scroll wheel) at a steady 60 FPS.
- **Vector Tools**:
  - `Select`: Move, transform, multi-select, and marquee box-select objects.
  - `Line`: Two-point vector lines.
  - `Rectangle` & `Ellipse`: Standard vector primitive shapes.
  - `Freehand`: Sampled polyline and closed-polygon drawing.
  - `Text`: TrueType font rendering with customizable font size and placement.
- **Legacy Silhouette Studio Format Parser (`.studio` / `.studio3`)**:
  - Direct binary import and conversion of proprietary Silhouette Studio `.studio` (`silhouette04;`) and `.studio3` (`silhouette05;`) files on Linux without Windows or Wine.
  - Automatically identifies format headers and offsets (`0x0000` for v4, `0x0400` with section offset tables for v5).
  - Isolates vector document streams and decodes embedded PNG print-and-cut artwork (`89 50 4E 47`) underneath vector cut lines.
  - Zero-panic architecture: safe bounds checking and memory access across all stream offsets with zero naked unwraps.
  - Parses both 34-byte 2-handle (`01 21 00 00 00`) and 26-byte 1-handle (`01 19 00 00 00`) cubic Bézier vertex records, 2D affine transforms `[a, b, tx, c, d, ty]`, and RGBA cut/fill colors.
  - True physical unit scaling: converts Silhouette Studio native millimeters to inches (`1.0 / 25.4`) so designs appear at 100% true physical size on the mat.
  - Compound paths & hole punching: detects concentric same-color contours (e.g. wheels, cutouts) and punches holes cleanly using `FillRule::EvenOdd`.
  - Flattens Bézier curves via adaptive de Casteljau subdivision into smooth canvas polylines.
  - **Batch Converter**: One-click folder batch conversion to export clean, standard vector `.svg` files for entire directories of Silhouette Studio files.
  - **CLI File Loading**: Direct command-line file loading and headless verification via `ss_rebuild <file> --screenshot <path>`.
- **High-Performance SVG Import**:
  - Parses complex multi-layer commercial SVG cutting files with `usvg` 0.45.
  - Renders exact SVG styles, CSS classes, linear/radial gradients, and `FillRule::EvenOdd` cutouts into a cached `egui::TextureHandle` via `resvg` 0.45.
  - Precomputes polygon mesh triangles to avoid per-frame CPU ear-clipping lag.
- **Grouping & Multi-Selection**:
  - Auto-groups imported SVG files under a shared `group_id` for easy whole-design manipulation.
  - Group (`Ctrl+G`) and Ungroup (`Ctrl+Shift+G`) shortcuts and menu actions.
  - `Shift+Click` and marquee drag-box selection.
  - Synchronized multi-shape translation on drag.
- **Layers Panel**:
  - Top-to-bottom z-order layer list in the right sidebar.
  - Reordering tools: Bring to Front, Bring Forward, Send Backward, Send to Back.
  - Visibility eye toggle (`is_visible`) and Lock toggle (`is_locked`).
  - Shape color swatches and click-to-select integration.
- **Custom Background Paper**:
  - Import custom digital scrapbooking / cardstock paper patterns (PNG/JPG) under Page Setup.
  - Stretches paper across the mat for visual layout previews without polluting SVG cut lines.
- **Export & Project Persistence**:
  - Save and open full project state as clean JSON (`.json`).
  - Export machine-ready cut paths as SVG in mat-inch coordinates.
  - Export high-DPI composited raster previews as PNG (e.g. 150 DPI).
  - Headless `--screenshot <path>` argument for automated rendering and CI.

---

## Architecture Map

```
silhouette-studio-linux/
├── CHANGELOG.md              # Detailed log of completed milestones & next roadmap
├── README.md                 # Project documentation & build guide
└── app/
    ├── Cargo.toml            # Dependencies: eframe, egui, tiny-skia, usvg, resvg, image, rfd
    └── src/
        ├── main.rs           # Core application UI, viewport, undo stack & tools
        └── studio_format.rs  # Silhouette Studio binary parser, de Casteljau & SVG converter
```

### Main Components:

| Struct / Enum / Module | Role |
| :--- | :--- |
| `Tool` | Active tool mode (`Select`, `Line`, `Rectangle`, `Ellipse`, `Text`, `Freehand`). |
| `ShapeKind` | Vector shape classification (`Rectangle`, `Ellipse`, `Line`, `Freehand`, `Text`). |
| `CanvasShape` | In-memory canvas object: mat-inch bounds, points, fill/stroke styling, precomputed triangles, `group_id`, `is_visible`, `is_locked`, and `is_svg_path`. |
| `studio_format` | Binary parser & converter module decoding `silhouette04;` and `silhouette05;` streams, `01 21 00 00 00` Bézier knots, and affine transforms. |
| `SerializableShape` | Serde-compatible representation of shapes for JSON project files. |
| `CachedSvg` | Caches `tiny_skia::Pixmap` rasterization from `resvg` and GPU `egui::TextureHandle` for imported SVGs. |
| `StudioApp` | Central application state: canvas viewport, tool selection, layer stack, mat dimensions, paper background, and event handling. |

### Data Flow Pipeline:

```
[SVG File / XML] 
       │
       ▼
 [usvg::Tree] ───────────────┬──────────────────────────────┐
       │                     │                              │
       ▼                     ▼                              ▼
 [resvg::render]      [Collect Paths]               [Assign group_id]
       │                     │                              │
       ▼                     ▼                              │
[tiny_skia::Pixmap]    [Sample Points]                      │
       │                     │                              │
       ▼                     ▼                              │
[egui::TextureHandle] [Precompute Triangles]                │
       │                     │                              │
       │                     ▼                              │
       │             [CanvasShape] <────────────────────────┘
       │                     │
       ▼                     ▼
[60 FPS GPU Quad]    [Canvas Interaction: Hit-Test, Drag, Group, Layers]
```

---

## Building and Running

### Prerequisites

Ensure you have Rust (edition 2021) and standard Linux graphics/windowing libraries installed:

```bash
# Ubuntu / Debian
sudo apt-get install -y build-essential pkg-config libfontconfig1-dev libx11-dev libwayland-dev

# Fedora / RHEL
sudo dnf install -y fontconfig-devel libX11-devel wayland-devel
```

### Build & Run

```bash
# Clone the repository
git clone https://github.com/vineofthedead-sudo/silhouette-studio-linux.git
cd silhouette-studio-linux/app

# Run development build
cargo run

# Build optimized release binary
cargo build --release

# Run release binary
./target/release/ss_rebuild
```

### Running Automated Tests

```bash
cd app
cargo test
```

### Headless Screenshot Verification

To capture a high-resolution screenshot without a display server (e.g. in CI or SSH):

```bash
xvfb-run -a -s "-screen 0 1280x800x24" ./app/target/release/ss_rebuild --screenshot /tmp/screenshot.png
```
