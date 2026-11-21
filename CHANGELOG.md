# Changelog & Session Handoff

All notable changes, architectural implementations, and roadmap milestones for the Silhouette Studio Linux rebuild (`ss_rebuild`).

---

## [Unreleased] - 2026-11-21

### Completed Tonight

#### 1. Silhouette Studio Binary Format Parser & Converter (`.studio` / `.studio3`)
- **Dedicated Parser Module (`app/src/studio_format.rs`)**:
  - Full reverse-engineered binary container decoding for both legacy `.studio` (v2-v4) and modern `.studio3` (v4-v5) proprietary formats without requiring Windows or Silhouette Studio.
- **Binary Header & Stream Detection**:
  - `.studio` (`silhouette04;`): Serialized object stream detected directly at byte offset `0x0000`.
  - `.studio3` (`silhouette05;`): 1,024-byte (`0x0400`) header with section offset table, followed by serialized `silhouette05;` object stream at `0x0400` (or stream offset designated in header table).
- **Document Metadata & Units**:
  - Parses mat dimensions (`12x12`, `letter`, `12x24`, `portrait`, `A4` in points: 72 pt = 1 in) and automatically configures the canvas mat size.
  - Extracts semantic layer names (`Default Layer`, `Layer 1`, etc.) and maps them directly into `StudioApp::group_names`.
- **Cubic Bézier Path Vertex Records (`01 21 00 00 00`)**:
  - Decodes `01 21 00 00 00` marker records containing anchor coordinates `(x, y)` and incoming/outgoing control handles `(cx1, cy1)`, `(cx2, cy2)` as little-endian IEEE-754 `f32` values.
  - Handles 2D affine transform matrices `[a, b, tx, c, d, ty]` applied to vertices.
  - Preserves RGBA stroke colors (defaulting to standard `#FF0000` cut lines) and fill colors.
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
