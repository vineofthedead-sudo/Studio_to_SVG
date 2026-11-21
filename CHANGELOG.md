# Changelog & Session Handoff

All notable changes, architectural implementations, and roadmap milestones for the Silhouette Studio Linux rebuild (`ss_rebuild`).

---

## [Unreleased] - 2026-11-21

### Completed Tonight

#### 1. SVG Group Hierarchy Preservation & Multi-Level Ungrouping (Inkscape Parity)
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

1. **`.studio` / `.studio3` Legacy Format Support**:
   - Reverse-engineer and parse proprietary Silhouette Studio v2/v3/v4 binary container formats (encrypted ZIP / zlib streams containing proprietary shape vectors, text styles, and cut settings) to allow opening existing library files directly on Linux.
2. **Interactive Corner Transform Handles**:
   - Interactive bounding box drag handles for proportional and non-proportional mouse-driven scaling and rotation directly on the cutting mat.
3. **Cardstock Offset / Contour Tool**:
   - Polygon buffering algorithm to create exterior or interior cut lines (offsets) around text and shapes for stickers, vinyl decals, and layered cardstock backing.
4. **Undo / Redo Stack**:
   - Command-pattern or snapshot-based state management (`Ctrl+Z` / `Ctrl+Y`) for seamless rollback of canvas edits, movements, deletions, and layer reordering.
5. **Direct USB Cutter Device Communication**:
   - `libusb` interface for Silhouette Cameo (1-5), Portrait (1-4), and Curio cutters to send G-code / GP-GL commands directly over USB on Linux without Windows emulation.
