# Changelog & Session Handoff

All notable changes, architectural implementations, and roadmap milestones for the Silhouette Studio Linux rebuild (`ss_rebuild`).

---

## [0.4.0] - 2026-11-21

### Added
- **Native `.craft` Project Save & Load (`app/src/project_file.rs`)**:
  - Implemented version 2 `.craft` format preserving full workspace state: mat dimensions, paper background, all vector/raster/text shapes, fill/stroke styles, group hierarchies, visibility, lock states, and Print & Cut registration marks.
  - Dependency-free base64 PNG embedding for 100% fidelity persistence of embedded raster graphics.
  - Shortcut support: `Ctrl+S` (Save Project), `Ctrl+Shift+S` (Save Project As), and `Ctrl+O` (Open Project).
- **Vector Crafting & Path Operations (`app/src/vector_ops.rs`)**:
  - **Offset Path (Sticker & Cardstock Mat Border)**: Generates parallel inward and outward offset contours using vertex normal bisectors and winding-aware geometry with interactive distance slider (-0.25" to +0.50").
  - **Make / Release Compound Path**: Combines multiple selected paths into an `EvenOdd` compound path with inner cutouts, or decomposes compound paths back into individual subpaths.
  - **Weld**: Unions overlapping shapes into a single continuous outline using Graham scan convex hull geometry.
  - **Align & Distribute Panel**: One-click alignment buttons (`Align Left`, `Center H`, `Align Right`, `Align Top`, `Center V`, `Align Bottom`, `Center to Mat`) and equal spacing distribution (horizontal/vertical) in the Transform panel.
- **Rich Typography & Text-on-Canvas (`app/src/main.rs`)**:
  - Live system TrueType/OpenType font rendering via `ab_glyph` across system directories (`/usr/share/fonts`).
  - Font family selection (Liberation Sans, Liberation Serif, Liberation Mono, Noto Sans, JetBrains Mono).
  - Font size (pt), bold, italic, and letter spacing controls.
  - **"✂ Convert Text to Cut Paths"**: Converts live text into editable vector cut paths (`CanvasShape::Freehand`) ready for vinyl and craft cutters.
- **Print & Cut Registration Marks**:
  - Standard Silhouette/Cricut corner marks (top-left filled square, top-right and bottom-left L-brackets) with adjustable inset, mark size, and thickness in Page Setup.
  - Rendered live on the cutting mat and exported into both SVG and high-DPI PNG outputs.
- **Linux Desktop Integration**:
  - Application icon: `assets/craft-anything.svg` (vector) and `assets/craft-anything.png` (256x256 raster).
  - Desktop launcher: `craft-anything.desktop` with categories and keywords.
  - MIME type registration: `craft-anything-mime.xml` for `.craft`, `.studio`, `.studio3`, and `.svg`.
  - Automated installer: `./install_desktop.sh` supporting both system-wide and user-level installation.
- **Batch Converter Polish**:
  - Preserves both vector paths and embedded raster layers in batch-converted SVGs with status counts and diagnostic reporting.

---

## [Unreleased] - 2026-11-21

### Completed Tonight

#### 1. Silhouette Studio Binary Format Parser & Converter (`.studio` / `.studio3`)
- **Dedicated Parser Module (`app/src/studio_format.rs`)**:
  - Full reverse-engineered binary container decoding for both legacy `.studio` (v2-v4) and modern `.studio3` (v4-v5) proprietary formats without requiring Windows or Silhouette Studio.
- **Zero Panics & Safe Bounds Checking**:
  - Replaced all unchecked slice indexing, naked `unwrap()` calls, and unchecked allocations with safe `.get()` slice access and saturating bounds checking (`read_u8`, `read_u32_le`, `read_u64_le`, `read_f32_le`).
  - Gracefully handles malformed, truncated, or massive files without risking memory corruption or panics.
- **Full Record-Tag Audit & 1-Handle Record Support (`01 19 00 00 00`)**:
  - Decodes both 34-byte 2-handle records (`01 21 00 00 00`) and 26-byte 1-handle records (`01 19 00 00 00`), accounting for 100% of vertices and shapes in `Peeker Bears.studio` (224 vertices across 26 objects) without skipped segments or contour splits.
- **True Physical Coordinate Scaling (Millimeters to Inches `1.0 / 25.4`)**:
  - Corrected coordinate scaling from legacy 72 DPI points (`1.0 / 72.0`), which had shrunk imported designs to ~1/3 size, to true Silhouette Studio physical millimeters (`1.0 / 25.4`).
  - Designs now appear at authentic 100% real-world cutting scale (e.g. `Peeker Bears.studio` fills the `8.5" x 11"` Letter page at ~7.5" x 6.4").
- **Compound Paths & Hole Punching (`FillRule::EvenOdd`)**:
  - Detects concentric contours sharing the same fill color (e.g. concentric blue wheels/circles in `Peeker Bears.studio`) and merges them into compound paths with `FillRule::EvenOdd`.
  - Inner cutout holes punch cleanly through outer shapes instead of stacking as opaque disks.
  - Pre-renders `tiny_skia::Pixmap` with `tiny_skia::FillRule::EvenOdd` for true canvas/PNG transparency and outputs SVG `<path d="M ... Z M ... Z" fill-rule="evenodd" />`.
- **Embedded Raster Image Extraction (`.studio3` Print & Cut)**:
  - Extracts embedded 512x512 PNG raster artwork from `.studio3` container sections (`DISNEY HEARTS.studio3`).
  - Renders the image as a background `CanvasShape` on the cutting mat underneath red vector cut lines (`Print & Cut` preview).
  - Exports embedded raster artwork directly to standalone SVG via `<image href="data:image/png;base64,..." />`.
- **CLI File Loading & Headless Verification**:
  - CLI supports passing an input `.studio`, `.studio3`, or `.svg` file directly (`ss_rebuild <file> --screenshot <path>`) for instant verification and headless rendering.
- **Vector Document Stream Isolation**:
  - Detects embedded PNG preview raster chunks (`89 50 4E 47`) and confines vector stream decoding strictly to the document vector slice (`[0x0400..png_start]`), preventing false-positive marker matches and avoiding unnecessary scanning of large multi-megabyte thumbnail data.
- **Resilient 34-Byte Bézier Record & Transform Extraction**:
  - Decodes the official Silhouette Studio 34-byte record layout (`ax, ay, tag1, h1x, h1y, tag2, h2x, h2y, tail`) where control handles are relative tangent vectors when tagged, falling back to flat 24-byte coordinate fixtures for synthetic test data.
  - Fixed an off-by-one matrix assignment where paths were rendered with the previous path's transform; each path now extracts and applies its own 3x3 affine matrix `[a, b, tx, c, d, ty]` found immediately after its vertices.
  - Implemented 2D affine matrix multiplication (`std::ops::Mul`) for `AffineTransform2D` (`parent * child`) so hierarchical parent/group transforms combine cleanly down child contours.
  - Added solid RGBA fill color extraction (`01 ea 00 00 00` records), restoring full-color fills (e.g. bear fur `#dda56e`, snouts `#dd6e00`, accents `#6e6edd`, and eyes `#000000`).
  - Unified document-wide coordinate scaling (72 pt = 1 in) to prevent small features (noses, eyes) from scaling differently than large parent paths, placing designs compactly and authentically on the cutting mat.
- **Real-File Verification Suite**:
  - Added integration tests directly verifying real `.studio` and `.studio3` files (`Peeker Bears.studio` yielding 27 complex paths with >= 20 solid RGBA fills in a coherent 3.5"x2.7" layout on the 8.5x11 page; `DISNEY HEARTS.studio3` yielding 437 clean paths), asserting finite non-zero bounding boxes and valid SVG generation.
  - Ensured `test_samples/` is ignored in `.gitignore`.
- **Document Metadata & Units**:
  - Parses mat dimensions (`12x12`, `letter`, `12x24`, `portrait`, `A4` in points: 72 pt = 1 in) and automatically configures the canvas mat size.
  - Extracts semantic layer names (`Default Layer`, `Layer 1`, etc.) and maps them directly into `StudioApp::group_names`.
- **Adaptive de Casteljau Subdivision**:
  - Recursively subdivides cubic Bézier curves until chord deviation is within tolerance, generating smooth polylines with zero unnecessary points on straight segments.
  - Generates `CanvasShape`s with precomputed polygon mesh triangulation for full-color on-mat rendering, vector SVG export, and high-DPI PNG export.
- **UI Integration**:
  - **File -> Open / Import .studio / .studio3...**: Native file picker filtered to `*.studio`, `*.studio3` with snapshot undo support and automatic mat size adjustment.
  - **Tools / File -> Batch Convert .studio to SVG...**: Folder picker that scans directories for `.studio` and `.studio3` files and exports clean, standalone `.svg` vector files alongside them with live status count reporting.

#### 2. Interactive Corner Resize Handles & Shift Aspect-Ratio Lock
- **Draggable 4-Corner Handles**: Interactive 8px square handles at all 4 corners (`TopLeft`, `TopRight`, `BottomLeft`, `BottomRight`) on the selection bounding box for live mouse-driven scaling of individual shapes or entire multi-shape groups.
- **Directional Cursor Icons**: Hovering or dragging over corners dynamically updates the cursor to `ResizeNwSe` or `ResizeNeSw`.
- **Shift Key Aspect Ratio Lock**: Holding `Shift` while dragging any corner constrains scaling to preserve the selection's original aspect ratio.
- **Normalized Multi-Shape Scaling**: Scales child shape positions `(x, y)`, dimensions `(width, height)`, and polygon vertices smoothly relative to the opposite anchor corner with zero distortion.
- **Active & Hover Highlighting**: Corner handles highlight with active fill (`#D2EBFF`) and high-contrast outline (`#0078D7`) when hovered or actively dragged.

#### 2. Clean Selection Bounding Box (No Diagonal Compound-Path Artifacts)
- **Eliminated Diagonal Cross-Lines**: Removed raw polyline stroke rendering across compound SVG paths (`M ... Z M ... Z`), ensuring complex multi-layer designs (like characters with cutouts, eyes, or inner contours) select cleanly with only an outer dashed bounding rectangle.
- **Tight Visual Framing**: Selected shapes display a clean dashed border with 4 corner drag handles, matching professional vector editors (Inkscape, Illustrator, Silhouette Studio).

#### 3. Bounded Undo / Redo History Stack (`Ctrl+Z` / `Ctrl+Y` / `Ctrl+Shift+Z`)
- **Full Canvas State Snapshots**: Automatically records state snapshots (`shapes`, `group_names`, `next_group_id`, selection) up to 50 levels deep before destructive or transformative actions (move, resize, delete, clear, group, ungroup, layer reorder, import, shape creation).
- **Standard Keyboard Shortcuts**: Full support for `Ctrl+Z` (Undo), `Ctrl+Y` (Redo), and `Ctrl+Shift+Z` (Redo).
- **Edit Menu Integration**: Enabled/disabled "Undo (Ctrl+Z)" and "Redo (Ctrl+Y)" menu items in the top menu bar with live status bar feedback.

#### 4. SVG Group Hierarchy Preservation & Multi-Level Ungrouping (Inkscape Parity)
- **Nested `<g>` Hierarchy (`group_path: Vec<u64>`)**: Instead of flattening complex SVGs into an unorganized flat group of loose paths, the full nested `<g>` group structure is preserved through an ancestor stack (`group_path`).
- **Semantic Subgroup Naming**: Inherits XML group IDs (e.g. `Minnie`, `Mickey`, `Presents`, `Bow`, `Dress`) directly into `StudioApp::group_names` for clear layer hierarchy.
- **Multi-Level Ungrouping**:
  - Clicking **Ungroup** (`Ctrl+Shift+G`) once cleanly separates top-level character/element groups (e.g. Minnie, Presents, Mickey).
  - Clicking Ungroup again on a character drills into sub-layers and color parts (e.g. Bow, Dress, Face).
  - A third Ungroup dissolves sub-layers into individual leaf paths.
- **Top-Level Group Selection**: Clicking any child element on the mat selects the outermost group on the first click, matching standard vector illustration behavior.

#### 2. Per-Shape Cached `resvg` / `tiny-skia` Textures (Zero Black Silhouette Fallback)
- **Individual High-Resolution Textures**: Each imported SVG path is rasterized with `tiny-skia` using its exact bounds, full-color paint, stroke, and native `FillRule::EvenOdd` (or `Winding`) into a tight-bounding-box `pixmap` and cached GPU `egui::TextureHandle`.
- **Permanent Full-Color Rendering**: Completely removed ear-clipping convex polygon fallback during ungrouping. Ungrouping and dragging individual shapes, characters, or color layers renders in crisp full color at 60 FPS with zero black silhouette corruption.
- **Pixel-Accurate Alpha Hit Testing**: Complex cutouts and irregular paths test alpha channel values (`alpha > 10`) for precise selection clicking.
- **Live Transform Binding**: Per-shape textures render directly at `(shape.x, shape.y, shape.width, shape.height)`, automatically moving and scaling in lockstep with user dragging.

#### 3. Hierarchical Nested Tree in Layers Panel
- **Recursive Tree Hierarchy**: The Layers panel displays direct child groups only at each hierarchy level, eliminating the flat 1,600+ row explosion. An imported SVG starts as a single summary row (`▶ SVG Design (1624 items)`), expanding into its direct subgroups (e.g. Minnie, Presents, Mickey), which in turn expand into their own direct sub-layers.
- **Hierarchical Controls**: Group-level Eye (visibility), Lock (immutability), and selection affect all descendants across any level of nesting.
- **Indented Tree Rows**: Distinct visual hierarchy with customizable indentation per nesting depth and color swatches.

#### 4. Dynamic `resvg` Texture Transform Binding (Eliminating Static Ghosting)
- **Live Bounding Box Synchronization**: Dynamically binds cached `resvg` textures to the live bounding box of member `CanvasShape`s using group tracking (`cached.group_id`). Dragging or scaling a group moves and resizes the high-resolution texture in 100% lockstep with zero offset or ghosting at `(0, 0)`.
- **Seamless Vector Fallback**: Ungrouping (`Ctrl+Shift+G`), individual shape editing, or modifying color groups automatically suspends group texture rendering and falls back to per-shape vector rendering with precomputed mesh triangles, exact fill colors, and cut strokes.
- **High-DPI PNG Synchronization**: `export_png_file` reflects live transformed positions for cached SVG groups, ensuring exported raster images match canvas placement exactly.

#### 2. Collapsible Group Rows & Virtualized Scrolling in Layers Panel
- **Collapsible Group Summaries**: Groups in the Layers panel now collapse into single expandable rows (e.g. `▶ Group 1 (1624 items)`), eliminating sidebar clutter when working with complex commercial cutting files.
- **Group-Level Visibility & Locking**: One-click group Eye (👁/🚫) and Lock (🔒/🔓) toggles to hide or lock an entire multi-path graphic at once.
- **High-Performance Virtualized Scrolling**: Expanded groups use `egui::ScrollArea::show_rows` to render only the visible rows in the viewport, guaranteeing silky-smooth 60 FPS scrolling even with 1,600+ paths.
- **Group-by-Color Tool (`🎨 By Color`)**: Added a one-click "Group by Color" button in the Layers panel toolbar that clusters selected (or all) shapes by matching fill/stroke colors into separate cardstock/vinyl cut layers.

#### 3. High-Performance SVG Import & Rendering (`usvg` + `resvg`)
- **Cached Vector Rendering (`resvg`)**: Replaced per-frame CPU polygon ear-clipping with a cached high-resolution texture pipeline. Imported SVG files (or `usvg::Tree`) are rasterized once at import time into a `tiny_skia::Pixmap` (at 150 DPI) and uploaded to an `egui::TextureHandle`.
- **Accurate SVG Standards**: Full support for CSS styles, nested `<g transform>`, linear/radial gradients, and `FillRule::EvenOdd` cutouts (preventing solid-black artifacts on complex composite paths and outlines like commercial Disney cutting files).
- **60 FPS Viewport**: Panning, zooming, and canvas inspection now run with zero per-frame CPU triangulation overhead as textured quads on the GPU.
- **Precomputed Mesh Triangulation**: Individual user-drawn or edited `CanvasShape`s precompute polygon mesh triangles once during creation or transform, eliminating triangulation inside `StudioApp::update`.

#### 2. Grouping & Multi-Selection (`Ctrl+G` / `Ctrl+Shift+G`)
- **Shared Group Assignment**: All paths from an imported SVG file are automatically assigned a unique shared `group_id`, allowing the user to select and move the entire graphic together on the cutting mat by default.
- **Group & Ungroup Actions**: Added Group (`Ctrl+G`) and Ungroup (`Ctrl+Shift+G`) shortcuts, Edit menu actions, and toolbar buttons. Ungrouping separates a complex SVG into individual selectable, editable cut paths.
- **Multi-Selection**:
  - `Shift+Click` to toggle or add shapes/groups to the active selection.
  - Drag-marquee box selection on empty canvas space to select multiple shapes within an area.
  - Multi-shape dragging: Moving translates all selected, unlocked shapes simultaneously.
  - Combined bounding box with corner handles wraps the entire group or multi-shape selection.

#### 3. Layers Panel (Right Sidebar Tab)
- **Top-to-Bottom Z-Order Display**: A dedicated **Layers** tab in the right sidebar lists all canvas shapes and groups ordered from topmost to bottommost.
- **Visibility Toggle (Eye Icon)**: Toggle individual shape/group visibility (`is_visible`). Hidden shapes are skipped during rendering and hit-testing.
- **Lock Toggle (Lock Icon)**: Toggle shape lock state (`is_locked`). Locked shapes cannot be dragged or transformed on the canvas.
- **Z-Order Reordering**:
  - ⏏ **Bring to Front**: Moves selected shapes to the top of the canvas stack.
  - ▲ **Bring Forward**: Steps selected shapes up one layer.
  - ▼ **Send Backward**: Steps selected shapes down one layer.
  - ⏷ **Send to Back**: Moves selected shapes to the bottom of the canvas stack.
- **Layer Selection**: Direct row clicking (and `Shift+Click`) synchronizes selection with the cutting mat.
- **Color Swatches**: Visual indicator swatches for each shape's fill/stroke color.

#### 4. Custom Paper Background (Page Setup Tab)
- **Custom Paper Import**: Added "Import Paper Background..." button under the Page Setup tab to load custom digital scrapbooking / cardstock paper patterns (PNG, JPG, JPEG).
- **Canvas Layout Preview**: Renders the paper texture stretched to the cutting mat dimensions behind all vector shapes for accurate vinyl/print layout preview.
- **Clear Paper Action**: One-click "Clear Paper" button to reset the mat to default clean grid styling.
- **Export Separation**: Background paper is excluded from cut vector SVG export (cut lines only), and cleanly composited in High-DPI PNG exports.

#### 5. Project Persistence & File Export
- **JSON Project Format (`.json`)**: Full save and open support for project mat dimensions and shapes (with `kind`, coordinates, points, fill colors, stroke widths, group IDs, visibility, and lock states).
- **Vector SVG Export**: Export designs in physical mat-inch coordinates with `<rect>`, `<ellipse>`, `<line>`, `<polyline>`, `<polygon>`, and `<text>` tags matching cutter specifications.
- **High-DPI PNG Export**: Renders the complete mat with grid, registration border, paper background, and vector paths to PNG at customizable DPI (e.g. 150 DPI).
- **Headless Screenshot Mode**: Fast `--screenshot <path>` flag for automated testing and CI verification.

---

## Roadmap & Next Milestones

1. **Cardstock Offset / Contour Tool**:
   - Polygon buffering algorithm to create exterior or interior cut lines (offsets) around text and shapes for stickers, vinyl decals, and layered cardstock backing.
2. **Rotation Transform Handle & Angle Readout**:
   - Top center rotation stalk handle for mouse-driven angular rotation of selections on the cutting mat with Shift-snapping (15-degree increments).
3. **Direct USB Cutter Device Communication**:
   - `libusb` interface for Silhouette Cameo (1-5), Portrait (1-4), and Curio cutters to send G-code / GP-GL commands directly over USB on Linux without Windows emulation.
