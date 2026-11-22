# Changelog & Session Handoff

All notable changes, architectural implementations, and roadmap milestones for the Studio to SVG Linux application (`studio-to-svg`).

---

## [0.4.9] - 2026-11-22

### Added & Improved
- **Prevented OOM / Memory Explosion on Large `.studio3` Files (`app/src/main.rs`, `app/src/studio_format.rs`)**:
  - Replaced owned `Option<tiny_skia::Pixmap>` in `CanvasShape.pixmap` with `Option<std::sync::Arc<tiny_skia::Pixmap>>`. Cloning `CanvasShape` on undo stack pushes (`undo_stack.push(self.shapes.clone())`) now clones 8-byte atomic pointers instead of deep-copying tens of megabytes of raw RGBA pixel buffers.
  - Capped per-shape raster dimensions to max `1024×1024` with proportional downscaling (`(1024.0 / raw_w.max(raw_h)).min(1.0)`) in `create_canvas_shape_from_subpaths` and `merge_compound_paths`, preventing unbounded bitmap allocations on large files.
  - Reduced default tiny_skia raster resolution from 150 DPI to 96 DPI for vector path cache shapes, slashing memory usage by >50% (Judith Unicorn 20 MB file total pixmap RAM dropped from 36.85 MB to 18.21 MB).
  - Deduplicated `egui::TextureHandle` allocations in `StudioApp::update` using `Arc::as_ptr` pointer mapping across shapes that share identical pixmaps or pattern swatches, preventing GPU memory exhaustion.
- **Fixed White / Clipped Shape Visibility and Pattern Selective Fills (`app/src/studio_format.rs`, `app/src/main.rs`)**:
  - In `main.rs` canvas rendering, vector cut outlines (`stroke_to_draw` / `#ff2626`) are now drawn on top of textures instead of early-exiting with `continue;`, ensuring cutlines are always clearly visible over pattern-filled shapes and images.
  - Ensured white-filled (`#ffffff`) or unstroked shapes receive visible `#ff2626` cut outlines in both `studio_format.rs` and the egui canvas fallback renderer.
  - Restricted `pattern_swatch` application to white/light or uncolored vector shapes (`is_white_or_light || fill_color.a() == 0`), preventing authentic solid colors (e.g. Mickey/Minnie clothes, face, black contours) from being overwritten by pattern swatches.
- **Integration Testing & Test Suite Verification**:
  - Added `test_judith_unicorn_large_file_parsing` verifying that 20 MB `JUDITH UNICORN AND DINOSAUR.studio3` parses smoothly with 646 shapes in ~7 seconds, all vector shapes are capped to <= 1024×1024, total pixmap RAM stays < 50 MB, and white shapes have `#ff2626` cutlines.
  - Added `test_disney_hearts_pattern_clipping_and_visible_cutlines` asserting pattern swatch extraction, visible red cut outlines on heart shapes, solid color preservation on character shapes, and correct SVG export without unclipped background layers.
  - All 64 unit and integration tests passing (`cargo test`); release binary verified (`cargo build --release`).

---

## [0.4.8] - 2026-11-22

### Added & Improved
- **Preserved Multi-Subpath Contours in Compound Path Merging (`app/src/studio_format.rs`)**:
  - Fixed root cause of cross-subpath diagonal bridge lines in complex multi-contour cutouts (e.g. `PL ZOO PAPER.studio3` grass and reed cutouts).
  - Previously, `merge_compound_paths` flattened `outer.points` into a single polyline joined with `" L "`, obliterating original `M` (`MoveTo`) and `Z` boundaries and connecting 86+ separate grass blades with straight diagonal lines.
  - Updated `merge_compound_paths` to retain and concatenate `outer.text` and `inner.text` SVG path commands, preserving all subpath boundaries (`M ... Z M ... Z`).
  - Updated `tiny_skia::PathBuilder` rendering inside `merge_compound_paths` to iterate per-subpath (`compound_d.split('M')`), issuing discrete `move_to` and `close` commands with `FillRule::EvenOdd` so every blade and island renders as an isolated cut contour without diagonal bridging lines.
- **Multi-Subpath Freehand Canvas Rendering (`app/src/main.rs`)**:
  - Enhanced `tiny_skia` freehand path builder in `main.rs` to parse multi-subpath `shape.text` (`shape.is_svg_path && shape.text.matches('M').count() > 1`) across individual `move_to` / `line_to` / `close` segments with `FillRule::EvenOdd`, guaranteeing zero diagonal bridge lines even when pixmap caches are regenerated.
- **Pattern Swatch & Red Cut Outlines Verified for `PL ZOO PAPER.studio3` (`app/src/studio_format.rs`)**:
  - Verified 512×512 square pattern swatch extraction from Section 2, mapped as clipped/tiled shader inside vector cut shapes with visible `#ff2626` cut outlines.
- **Integration Testing & Test Suite Verification**:
  - Added `test_pl_zoo_paper_grass_cutouts_no_diagonal_bridge_lines` asserting that:
    - Multi-subpath grass cutout shapes retain separate `M` MoveTo commands (>= 86 subpaths) and zero diagonal bridge lines (`!text.contains("Z L")`).
    - Every subpath cleanly terminates with `Z`.
    - Pre-rendered `tiny_skia` pixmaps have rendered pixels and `#ff2626` cut outlines.
    - Exported SVG contains `<pattern id="swatch_pattern">`, `fill-rule="evenodd"`, and visible red cut strokes without diagonal lines.
  - All 62 unit and integration tests passing (`cargo test`); release binary verified (`cargo build --release`).

---

## [0.4.7] - 2026-11-22

### Added & Improved
- **Multi-PNG Stream Discovery & Pattern Swatch Extraction (`app/src/studio_format.rs`)**:
  - Upgraded `extract_embedded_png` to scan all embedded PNG rasters across the entire binary stream instead of halting at the first thumbnail image.
  - Accurately classifies square power-of-two texture tiles (`w == h && (w & (w - 1)) == 0 && w >= 64`, e.g. 512×512) as repeating `pattern_swatch` assets.
  - Distinguishes high-resolution Print & Cut rasters (`w >= 400 && h >= 400 && !is_square`) from pattern swatches and low-res preview thumbnails (e.g. 225×225, 263×196), preventing preview thumbnails from ever spawning as full-mat background layers.
- **Vector Shape Pattern-Fill Tiling & Clipping (`app/src/studio_format.rs`)**:
  - Reordered `parse_studio_data` so `extract_embedded_png` runs before path parsing and compound-path merging, passing `pattern_swatch.as_ref()` into `parse_paths`, `create_canvas_shape_from_subpaths`, and `merge_compound_paths`.
  - Implemented tiled `tiny_skia::Pattern` shaders with `SpreadMode::Repeat`, `FilterQuality::Bilinear`, and 2.0-inch tile scale matching SVG `<pattern width="2.000" height="2.000">` output.
  - Pattern fills are rendered with `tiny_skia::FillRule::EvenOdd` and strictly clipped inside their referencing vector shapes, eliminating full-mat `#1 Print Background` overlays.
- **Guaranteed Vector Cut-Line Visibility (`#ff2626`) (`app/src/studio_format.rs`, `app/src/main.rs`)**:
  - In `create_canvas_shape_from_subpaths` and `merge_compound_paths`, if `stroke_color` is absent/transparent or the shape has a white fill (`#ffffff`) or pattern fill, automatically assign default cut-line stroke `egui::Color32::from_rgb(255, 38, 38)` (`#ff2626`) with stroke width `>= 0.02` in.
  - In `convert_studio_to_svg` and `export_svg_string`, emit `stroke="#ff2626" stroke-width="0.020"` for pattern-filled and white shapes with stroke `none`, ensuring all cut contours remain clearly visible against the cutting mat grid.
- **Integration Testing & Test Suite Verification**:
  - Added `test_studio3_pattern_fill_and_cut_outline_visibility` verifying swatch extraction, pattern clipping, visible `#ff2626` cut lines, pre-rendered non-transparent pixmaps, and SVG pattern export across `DISNEY HEARTS.studio3`, `1 DISNEY PEOPLE.studio3`, and `1 HEADS.studio3`.
  - Hardened `detect_format_and_stream` and `parse_studio_data` bounds checking against truncated or synthetic streams.
  - All 61 tests passing cleanly (`cargo test`); release binary verified (`cargo build --release`).

---

## [0.4.6] - 2026-11-22

### Added & Improved
- **Complete Rebrand to Studio to SVG (`studio-to-svg`)**:
  - Full repo-wide rebranding across all layers: package name `studio-to-svg` in `Cargo.toml`, binary executable `studio-to-svg`, main window title `"Studio to SVG"`, CLI `--help` text, and README. Zero legacy references to `Craft Anything` or `ss_rebuild`.
  - Renamed desktop assets and MIME configurations:
    - `craft-anything.desktop` -> `studio-to-svg.desktop` (`Name=Studio to SVG`, `Exec=studio-to-svg %F`, `Icon=studio-to-svg`, `StartupWMClass=studio-to-svg`).
    - `craft-anything-mime.xml` -> `studio-to-svg-mime.xml`.
    - `assets/craft-anything.svg` / `.png` -> `assets/studio-to-svg.svg` / `.png`.
  - Updated `install_desktop.sh` to install `studio-to-svg` binary, icons, desktop launcher, and default MIME associations, with automatic cleanup of legacy `craft-anything` files.
  - Maintained backward compatibility for `.craft` project files, deserializing legacy projects seamlessly.
- **Hierarchical Affine Transform Resolution for `.studio3` (`silhouette05;`) (`app/src/studio_format.rs`)**:
  - Identified and decoded explicit Studio V5 `Group` records (`05 00 00 00 Group`) and macro group container records across the object stream.
  - Implemented multi-level affine matrix multiplication (`layer_group * row_group * local_transform`), properly mapping row offsets (`ty`) and macro layer offsets (`tx`).
  - Fixed the collapsed coordinate bug in multi-shape `.studio3` designs (such as `DISNEY HEARTS.studio3`), properly distributing 435+ vector shapes across the entire 12"×12" mat (spanning > 5" in width and height) rather than flattening onto a 0.5" horizontal line.
- **Decoupled Pattern Swatches from Full-Mat Raster Overlays (`app/src/studio_format.rs`)**:
  - Differentiated square power-of-two texture tiles (such as the 512×512 swatch in Sec 2 of `DISNEY HEARTS.studio3`) from positioned Print & Cut document raster images.
  - Eliminated the giant opaque 12"×12" white background rectangle that previously obscured the grid mat and cut lines.
  - Added `pattern_swatch: Option<tiny_skia::Pixmap>` to `ParsedStudioDocument`.
  - Updated `convert_studio_to_svg` to export `<pattern id="swatch_pattern" ...>` with base64 embedded image data, rendering clean cut outlines (`stroke="#ff2626"`) directly visible on top.
- **Automated Verification & Tests (`app/src/studio_format.rs`)**:
  - Updated `test_real_sample_files_verification` asserting `doc2.pattern_swatch` extraction (512×512 pixmap) and verifying vector shape bounding span > 4.0 inches across both mat dimensions.
  - All 60 unit and integration tests passing (`cargo test`), release binary cleanly compiled.

---

## [0.4.5] - 2026-11-22

### Added & Improved
- **Multi-Tab Document Workspaces (`app/src/main.rs`, `app/src/project_file.rs`)**:
  - Implemented `DocumentTab` encapsulating isolated per-tab document states: `title`, `source_path: Option<PathBuf>`, `project_path: Option<PathBuf>`, `is_dirty: bool`, `shapes: Vec<CanvasShape>`, `cached_svgs`, `mat_width_in`, `mat_height_in`, `registration_marks`, `paper_background`, `zoom`, `pan`, and `undo_stack` / `redo_stack`.
  - Added sleek document tab bar directly above canvas (and below the top toolbar) showing each open document's title, a `•` dirty indicator when unsaved edits exist, an `×` close button per tab, and a `+` new tab button (`Ctrl+T` / `File -> New Design Tab`) opening a fresh blank `12.0" × 12.0"` default grid tab.
  - Implemented `sync_active_tab_to_storage` and `sync_active_tab_from_storage` ensuring zero borrow-checker conflicts and 100% backward compatibility with all existing test suites.
- **Smart Open / Import Into Tabs & Unsaved Changes Confirmation Modal (`app/src/main.rs`)**:
  - When opening or importing a `.studio`, `.studio3`, `.svg`, or `.craft` file (`File -> Open / Import...`, `Ctrl+O`, or CLI arguments), files open into a new tab titled with the file's original stem/filename and record `source_path`. If the initial tab is blank and unmodified, it is replaced directly without leaving an empty tab behind.
  - CLI argument handling in `main()` loads multiple input files across separate tabs sequentially.
  - Closing a modified tab (`is_dirty == true`, via `×` or `Ctrl+W`) triggers an **"Unsaved Changes"** modal dialog: *"Save changes to '<tab_title>' before closing?"* with **`Save`**, **`Don't Save`**, and **`Cancel`** buttons.
- **Automatic Source Filename & Directory Retention on Export / Save (`app/src/main.rs`, `app/src/project_file.rs`)**:
  - Track original filename stems and directories across document lifecycles.
  - Pre-populates `File -> Export SVG...` with `<original_stem>.svg`, `File -> Export PNG (High-DPI)...` with `<original_stem>.png`, and `File -> Save Project As (.craft)` with `<original_stem>.craft`.
  - Sets the initial directory of save/export dialogs to the source file's parent directory when valid, falling back to user Documents or Home directory.
  - Added optional `title` and `source_path` fields (`#[serde(default)]`) to `CraftProject` in `.craft` JSON projects, preserving source provenance across save and reload.
- **Unit Testing & Verification (`app/src/main.rs`, `app/src/project_file.rs`)**:
  - Added `test_tab_creation_and_defaults`: verifies 12x12 default dimensions, tab count, initial clean state, and switching.
  - Added `test_open_multiple_files_into_separate_tabs_with_stems`: verifies opening `Peeker Bears.studio` and `BD CATS.studio` into separate tabs, preserving titles, source paths, and independent shape counts.
  - Added `test_tab_dirty_flag_tracking_and_confirmation`: verifies dirty tracking on modification, close confirmation triggering, and reset.
  - Added `test_default_export_filename_generation`: verifies smart export stems for `.svg`, `.png`, and `.craft` exports.
  - All 60 unit and integration tests passing (`cargo test`), release binary cleanly compiled.

---

## [0.4.4] - 2026-11-21

### Added & Improved
- **Disconnected Subpath Segmentation & Explicit SVG `MoveTo` Boundaries (`app/src/studio_format.rs`)**:
  - Identified the exact 10-byte subpath contour header structure (`01 [len: u32] 01 [vertex_count: u32]`) separating disconnected islands within Studio shape records (e.g. 262 subpaths across the 5 cats in `BD CATS.studio`).
  - Introduced `StudioSubpath` and `StudioShapeCluster` segmentation in `parse_paths`, breaking vertices into subpaths when `gap > 4` (nominal consecutive vertex gap is 0, contour header gap is 10 bytes).
  - Emitted explicit SVG `M` (`MoveTo`) and `Z` (`ClosePath`) commands per subpath (`M x0 y0 L ... Z M x0' y0' ... Z`) instead of flattening disconnected islands into a single continuous wire. Completely eliminates stray diagonal connecting lines cutting across silhouettes and counters.
  - Pre-rendered `tiny_skia::Pixmap` using `PathBuilder` with per-subpath `pb.move_to(...)` and `pb.close()`, filled with `tiny_skia::FillRule::EvenOdd` and stroked with clean path caps and joins.
- **Canvas & Transform Lockstep (`app/src/studio_format.rs`, `app/src/main.rs`)**:
  - Implemented `transform_svg_path_data` to ensure SVG path coordinates scale and translate in 100% lockstep with vertex points during mat normalization and UI shape translations.
  - Updated `CanvasShape::precompute_triangles` to prevent polygon ear-clipping from creating bridge triangles across multi-contour compound paths.
- **Regression Testing & Verification (`app/src/studio_format.rs`)**:
  - Added `test_bd_cats_multi_subpath_segmentation_no_diagonal_artifacts` verifying that all 5 cats in `BD CATS.studio` parse as multi-contour SVG paths (`M count > 1`, up to 81 subpaths per cat) with pre-rendered EvenOdd pixmaps and zero straight-line `Z L` bridge artifacts.
  - All 56 tests passing with 0 warnings or failures. Release build verified.

---

## [0.4.3] - 2026-11-21

### Added & Improved
- **Legacy `.studio` (`silhouette04;`) Full Vector Geometry & Corner Vertex Support (`app/src/studio_format.rs`)**:
  - Added support for 0-handle straight-line / corner vertex records (`BEZIER_VERTEX_MARKER_0H = [0x01, 0x11, 0x00, 0x00, 0x00]`).
  - Added `parse_bezier_vertex_0h` extracting anchor `(ax, ay)` coordinates and setting incoming/outgoing control handles to match anchor positions with bounded candidate length detection (`[13, 17, 21, 25, 29]`).
  - Completely eliminates broken contours and contour fragmentation in files containing straight segments (e.g. 148 straight line segments in `BD CATS.studio`, 31 in `beverly high school mass.studio`, and 1 in `PP WHALE TAIL.studio`).
- **Complete Solid Fill & Material Color Extraction Across All Object Variants (`app/src/studio_format.rs`)**:
  - Replaced hardcoded single-length marker search with generalized scanning for Studio material/style color records (`01 [len_u32] 00 01 [r, g, b, a]` and direct `00 01 [r, g, b, a]`).
  - Pre-scans style records to associate each vector object and group with its exact cardstock color, reliably extracting authentic colors like Beverly Panthers Orange (`#f56700`), Red (`#ff0000`), Grey (`#c4c4c4`), BD Cats Brown (`#79472c`), and Whale Tail Black (`#000000`).
- **Exact Hierarchical Group & Shape Transforms (`parent * child`) (`app/src/studio_format.rs`)**:
  - Implemented group-aware 2D affine matrix resolution where group-level container transforms are discovered within object intervals and multiplied down to child paths (`parent.combine(&child)`).
  - Preserves exact relative design alignment so multi-part logos and paper-piecing layers stay in their authentic relative positions on the mat instead of separating, scattering, or collapsing.
  - Retained strict zero per-shape or per-group artificial clamping during import.
- **Multi-Hole Compound Path Cutouts (`app/src/studio_format.rs`)**:
  - Upgraded `merge_compound_paths` to collect all interior holes contained within an outer boundary shape sharing the same solid fill color (e.g., letters with multiple counters, emblem cutouts, concentric wheels), generating a unified SVG path with `FillRule::EvenOdd` and rendering with `tiny_skia`.
- **Comprehensive Integration Tests (`app/src/studio_format.rs`)**:
  - Added `test_new_sample_files_verification` covering `beverly high school mass.studio` (23 shapes, all filled, Panthers orange present, valid bounds), `BD CATS.studio` (5 filled cats, brown fill color present, valid non-overlapping bounds), and `PP WHALE TAIL.studio` (1 filled shape, black color, valid bounds).
  - Maintained 100% pass rate across the full test suite (55 tests passing, 0 failures, 0 warnings).

---

## [0.4.2] - 2026-11-21

### Added & Improved
- **Shift+Click Multi-Select (Canvas & Layers Panel) + Group Drill-Down (`app/src/main.rs`)**:
  - Canvas multi-selection: holding `Shift` (or `Ctrl`/`Cmd`) while clicking adds or toggles shapes and groups in `selected_indices` without deselecting currently selected objects.
  - Double-click drill-down (`select_shape_exact`): double-clicking a grouped shape on the canvas bypasses top-level group expansion to select only the exact child shape underneath the cursor, allowing individual piece recoloring and nudging without ungrouping.
  - Layers panel row selection: clicking any layer/group row selects it; `Shift+Click` and `Ctrl+Click` on rows multi-selects multiple layers directly from the list.
  - Arrow key nudging: keyboard arrow keys (`ArrowLeft`, `ArrowRight`, `ArrowUp`, `ArrowDown`) nudge all selected shapes (or drilled-down children) by `0.02"` (or `0.10"` with `Shift`), with full undo/redo history.
- **One-Click Paper-Piecing "Stack / Center" & "Spread / Explode" (`app/src/vector_ops.rs`, `app/src/main.rs`)**:
  - **"🎯 Stack / Center"**: Added in both the `Layers` panel and `Transform` panel to concentrically align bounding-box centers (`cx, cy`) of all selected shapes/groups onto their shared average center point, re-assembling exploded cardstock layers into a stacked paper-piecing character with one click.
  - **"⊞ Spread / Explode"**: Added in both `Layers` and `Transform` panels to arrange selected layers into a clean, non-overlapping multi-row cutting grid across the mat (with 0.5" margin and 0.5" gaps) so pieces cut separately by color without colliding.
  - Entity-aware movement: shapes sharing a top-level group move together as a single cohesive unit, while standalone or drilled-down shapes move independently.
- **Inline Layer & Group Renaming (`app/src/main.rs`, `app/src/project_file.rs`)**:
  - Double-clicking or clicking the small `✏` rename icon on any row in the `Layers` panel allows inline text editing of group and shape names (e.g., `"Yellow Shoes"`, `"Base Silhouette"`, `"Face & Eyes"`).
  - Pressing `Enter` or clicking outside commits the rename with Undo support; pressing `Esc` cancels.
  - Custom names are persisted in `.craft` projects via `CraftShape.name` and `CraftProject.group_names` with 100% roundtrip fidelity.
- **Verification & Unit Tests**:
  - Added unit tests for concentric alignment offsets (`test_stack_center_offsets`), explode grid layout (`test_explode_grid_offsets`), double-click group drill-down (`test_drill_down_and_shift_multi_select`), paper-piecing stack/center and explode with undo (`test_paper_piecing_stack_center_and_explode_with_undo`), and inline renaming persistence (`test_inline_renaming_and_craft_persistence`).
  - Total test suite expanded to 54 passing tests (0 failures).

---

## [0.4.1] - 2026-11-21

### Added & Improved
- **True Polygon Boolean Union (`app/src/vector_ops.rs`)**:
  - Integrated `i_overlay v9.2.0` for exact 2D polygon boolean operations with `OverlayRule::Union` and `FillRule::EvenOdd`.
  - Replaced the convex-hull `Weld` placeholder with `boolean_weld_shapes` and `boolean_weld_polygons`, merging overlapping shapes, polygons, and cursive script letters along their true outer contours while preserving interior holes (`WeldedShape` with outer boundaries and inner holes).
  - Added canvas and SVG/PNG support for multi-subpath rendering without artificial bridging segments between outer contours and holes.
- **Registration Marks Rendering Fix & "Fit to View" Zoom (`app/src/main.rs`)**:
  - Moved Print & Cut registration marks rendering to draw cleanly and visibly on top of the mat grid, paper background, and canvas shapes inside the configured `Inset (in)` bounds on canvas, SVG, and high-DPI PNG exports.
  - Added `Fit` zoom button in the top toolbar next to `Zoom: 100%`, plus `Fit to View (Ctrl+0)` in the View menu, auto-calculating optimal zoom so the full 12"x12" cutting mat fits comfortably inside split-screen windows with pan reset.
  - Ensured mouse-wheel zoom and middle/secondary drag panning work smoothly across Wayland and X11 environments.
- **Polished Text Tool Workflow (`app/src/main.rs`)**:
  - Canvas clicks with the `Text` tool now place the text shape, immediately switch the active tool back to `Select`, switch the right inspector tab to `Text`, and automatically focus the text edit input.
  - Typing immediately updates the placed text object without spawning unwanted duplicate `"Sample Design"` boxes.
- **Verification & Unit Tests**:
  - Added unit tests for true non-convex polygon union (`test_boolean_weld_non_convex_overlap`, 8 vertices), hole preservation (`test_boolean_weld_preserves_interior_holes`), disjoint shape separation (`test_boolean_weld_disjoint`), Fit to View zoom calculation (`test_fit_to_view_zoom`), and the polished Text tool workflow (`test_text_tool_workflow`).
  - Total test suite expanded to 49 passing tests (0 failures).

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
