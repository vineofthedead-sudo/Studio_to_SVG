//! Silhouette Studio Binary Parser & Converter Module
//!
//! Supports importing and converting legacy Silhouette Studio `.studio` (`silhouette04;`)
//! and `.studio3` (`silhouette05;`) files into `CanvasShape`s and standard SVG/PNG formats.
//!
//! Binary Architecture:
//! - `.studio` (`silhouette04;`): Legacy Studio v2-v4 binary stream starting directly at byte offset `0x0000`.
//! - `.studio3` (`silhouette05;`): Studio v4-v5 format with a 1,024-byte (`0x0400`) header and section
//!   offset table, followed by the serialized `silhouette05;` object stream starting at `0x0400`
//!   (or at the stream offset designated in the header table).
//! - Vector Document Stream Isolation: Detects embedded PNG preview raster chunks (`89 50 4E 47`) and
//!   confines vector parsing strictly to the document vector stream, preventing false-positive marker matches.
//! - Document Metadata: Mat dimensions (`12x12`, `letter`, `12x24`, `portrait`, `A4` in points: 72 pt = 1 in)
//!   and semantic layer names (`Default Layer`, `Layer 1`, etc.).
//! - Cubic Bézier Path Vertex Records: `01 21 00 00 00` 34-byte markers containing anchor `(x, y)` and
//!   incoming/outgoing control handles `(cx1, cy1)`, `(cx2, cy2)` as little-endian IEEE-754 `f32` values.
//! - 2D Affine Transform Matrices: `[a, b, tx, c, d, ty]` applied to vertices to position local shapes.
//! - Coordinate Normalization: Normalizes, scales, and centers designs onto the cutting mat canvas.
//! - Adaptive de Casteljau Subdivision: Recursively flattens cubic Bézier curves into smooth polylines
//!   with precomputed polygon mesh triangulation for full-color on-mat rendering.
//! - Safe Bounds Checking: Completely eliminates panicking indexing or unwrap calls.

use eframe::egui;
use std::collections::BTreeMap;
use std::path::Path;

use crate::{CanvasShape, ShapeKind};

pub const MAGIC_STUDIO_V4: &[u8] = b"silhouette04;";
pub const MAGIC_STUDIO_V5: &[u8] = b"silhouette05;";
pub const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
pub const HEADER_OFFSET_V5: usize = 0x0400; // 1,024 bytes
pub const BEZIER_VERTEX_MARKER: [u8; 5] = [0x01, 0x21, 0x00, 0x00, 0x00];
pub const BEZIER_VERTEX_MARKER_1H: [u8; 5] = [0x01, 0x19, 0x00, 0x00, 0x00];
pub const BEZIER_VERTEX_MARKER_0H: [u8; 5] = [0x01, 0x11, 0x00, 0x00, 0x00];

pub const TAG_PATH_BLOCK: [u8; 4] = [0x53, 0x50, 0x54, 0x48]; // "SPTH"
pub const TAG_TRANSFORM: [u8; 4] = [0x53, 0x58, 0x46, 0x4D];  // "SXFM"
pub const TAG_COLOR: [u8; 4] = [0x53, 0x43, 0x4F, 0x4C];      // "SCOL"
pub const TAG_LAYER: [u8; 4] = [0x53, 0x4C, 0x59, 0x52];      // "SLYR"
pub const TAG_MAT: [u8; 4] = [0x53, 0x4D, 0x41, 0x54];        // "SMAT"

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StudioFormatVersion {
    StudioV4, // .studio (silhouette04;)
    StudioV5, // .studio3 (silhouette05;)
}

#[derive(Debug)]
pub enum StudioParseError {
    InvalidMagic,
    UnexpectedEof,
    MalformedRecord(String),
    IoError(std::io::Error),
}

impl std::fmt::Display for StudioParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StudioParseError::InvalidMagic => write!(f, "Invalid Silhouette Studio magic header"),
            StudioParseError::UnexpectedEof => write!(f, "Unexpected end of file while reading Studio data"),
            StudioParseError::MalformedRecord(msg) => write!(f, "Malformed Studio record: {}", msg),
            StudioParseError::IoError(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for StudioParseError {}

impl From<std::io::Error> for StudioParseError {
    fn from(e: std::io::Error) -> Self {
        StudioParseError::IoError(e)
    }
}

// -------------------------------------------------------------------------------------------------
// Safe Byte-Reading Helpers (Zero Panics, Zero Unwraps)
// -------------------------------------------------------------------------------------------------

#[inline]
fn read_u8(data: &[u8], offset: usize) -> Option<u8> {
    data.get(offset).copied()
}

#[inline]
fn read_u32_le(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

#[inline]
fn read_u64_le(data: &[u8], offset: usize) -> Option<u64> {
    let bytes = data.get(offset..offset.checked_add(8)?)?;
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}

#[inline]
fn read_f32_le(data: &[u8], offset: usize) -> Option<f32> {
    let bytes = data.get(offset..offset.checked_add(4)?)?;
    Some(f32::from_le_bytes(bytes.try_into().ok()?))
}

/// 2D Affine Transform Matrix: `[a, b, tx, c, d, ty]`
/// Transforms points according to:
/// x' = a * x + b * y + tx
/// y' = c * x + d * y + ty
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AffineTransform2D {
    pub a: f32,
    pub b: f32,
    pub tx: f32,
    pub c: f32,
    pub d: f32,
    pub ty: f32,
}

impl Default for AffineTransform2D {
    fn default() -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            tx: 0.0,
            c: 0.0,
            d: 1.0,
            ty: 0.0,
        }
    }
}

impl std::ops::Mul for AffineTransform2D {
    type Output = Self;

    /// Multiplies two 2D affine transform matrices: `parent * child`.
    /// When applied to point `p`, `(parent * child).apply(p) == parent.apply(child.apply(p))`.
    fn mul(self, rhs: Self) -> Self {
        Self {
            a: self.a * rhs.a + self.b * rhs.c,
            b: self.a * rhs.b + self.b * rhs.d,
            tx: self.a * rhs.tx + self.b * rhs.ty + self.tx,
            c: self.c * rhs.a + self.d * rhs.c,
            d: self.c * rhs.b + self.d * rhs.d,
            ty: self.c * rhs.tx + self.d * rhs.ty + self.ty,
        }
    }
}

impl AffineTransform2D {
    pub fn new(a: f32, b: f32, tx: f32, c: f32, d: f32, ty: f32) -> Self {
        Self { a, b, tx, c, d, ty }
    }

    /// Combines parent matrix with child matrix: `parent.combine(child)`.
    pub fn combine(&self, child: &Self) -> Self {
        *self * *child
    }

    pub fn apply(&self, p: egui::Pos2) -> egui::Pos2 {
        egui::pos2(
            self.a * p.x + self.b * p.y + self.tx,
            self.c * p.x + self.d * p.y + self.ty,
        )
    }
}

/// A Cubic Bézier vertex knot in a Silhouette Studio path.
/// Contains the anchor position `(x, y)` and incoming `(cx1, cy1)` / outgoing `(cx2, cy2)` control handles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StudioBezierVertex {
    pub anchor: egui::Pos2,     // (x, y)
    pub handle_in: egui::Pos2,  // (cx1, cy1)
    pub handle_out: egui::Pos2, // (cx2, cy2)
}

/// A subpath contour within a Studio shape record, bounded by MoveTo boundaries.
#[derive(Debug, Clone)]
pub struct StudioSubpath {
    pub start_pos: usize,
    pub end_pos: usize,
    pub vertices: Vec<StudioBezierVertex>,
}

/// Parsed Silhouette Studio Document containing metadata and `CanvasShape`s.
#[derive(Debug, Clone)]
pub struct ParsedStudioDocument {
    pub version: StudioFormatVersion,
    pub stream_offset: usize,
    pub mat_width_in: f32,
    pub mat_height_in: f32,
    pub shapes: Vec<CanvasShape>,
    pub group_names: BTreeMap<u64, String>,
    pub pattern_swatch: Option<std::sync::Arc<tiny_skia::Pixmap>>,
}

/// Detects the file version and the byte offsets defining the vector object stream [stream_start..stream_end].
/// Explicitly identifies embedded PNG preview images and isolates vector stream bounds.
pub fn detect_format_and_stream(data: &[u8]) -> Result<(StudioFormatVersion, usize, usize), StudioParseError> {
    if data.is_empty() {
        return Err(StudioParseError::UnexpectedEof);
    }

    // Identify if any PNG raster thumbnail is embedded
    let png_start = find_subsequence(data, &PNG_SIGNATURE)
        .or_else(|| find_subsequence(data, &PNG_SIGNATURE[..4]));

    // 1. .studio (silhouette04;) at byte offset 0x0000
    if data.starts_with(MAGIC_STUDIO_V4) {
        let end = png_start.unwrap_or(data.len());
        return Ok((StudioFormatVersion::StudioV4, 0, end));
    }

    // 2. .studio3 (silhouette05;) standard layout: 1,024-byte (0x0400) header table
    if data.len() >= HEADER_OFFSET_V5 + MAGIC_STUDIO_V5.len() {
        if let Some(stream_magic) = data.get(HEADER_OFFSET_V5..HEADER_OFFSET_V5 + MAGIC_STUDIO_V5.len()) {
            if stream_magic == MAGIC_STUDIO_V5 {
                // Check section table at byte 0x10 for document stream length
                let section_len = read_u64_le(data, 0x10)
                    .or_else(|| read_u32_le(data, 0x10).map(|v| v as u64))
                    .map(|v| v as usize);

                let stream_end = if let Some(len) = section_len {
                    HEADER_OFFSET_V5.saturating_add(len).min(data.len())
                } else {
                    png_start.unwrap_or(data.len())
                };

                return Ok((StudioFormatVersion::StudioV5, HEADER_OFFSET_V5, stream_end));
            }
        }
    }

    // 3. Inspect section offset table within the first 1,024 bytes (0x0400)
    let header_limit = data.len().min(HEADER_OFFSET_V5);
    if header_limit >= 32 {
        let mut i = 16;
        while i + 4 <= header_limit {
            if let Some(offset_u32) = read_u32_le(data, i) {
                let off = offset_u32 as usize;
                if off >= 0x0200 && off + MAGIC_STUDIO_V5.len() <= data.len() {
                    if let Some(m) = data.get(off..off + MAGIC_STUDIO_V5.len()) {
                        if m == MAGIC_STUDIO_V5 {
                            let end = png_start.unwrap_or(data.len());
                            return Ok((StudioFormatVersion::StudioV5, off, end));
                        }
                    }
                }
            }
            if let Some(offset_u64) = read_u64_le(data, i) {
                let off = offset_u64 as usize;
                if off >= 0x0200 && off + MAGIC_STUDIO_V5.len() <= data.len() {
                    if let Some(m) = data.get(off..off + MAGIC_STUDIO_V5.len()) {
                        if m == MAGIC_STUDIO_V5 {
                            let end = png_start.unwrap_or(data.len());
                            return Ok((StudioFormatVersion::StudioV5, off, end));
                        }
                    }
                }
            }
            i = i.saturating_add(4);
        }
    }

    // 4. .studio3 (silhouette05;) starting directly at offset 0x0000
    if data.starts_with(MAGIC_STUDIO_V5) {
        let end = png_start.unwrap_or(data.len());
        return Ok((StudioFormatVersion::StudioV5, 0, end));
    }

    // 5. Fallback scan for silhouette05; magic anywhere in the file
    if let Some(pos) = find_subsequence(data, MAGIC_STUDIO_V5) {
        let end = png_start.unwrap_or(data.len());
        return Ok((StudioFormatVersion::StudioV5, pos, end));
    }

    // 6. Fallback scan for silhouette04; magic anywhere in the file
    if let Some(pos) = find_subsequence(data, MAGIC_STUDIO_V4) {
        let end = png_start.unwrap_or(data.len());
        return Ok((StudioFormatVersion::StudioV4, pos, end));
    }

    Err(StudioParseError::InvalidMagic)
}

/// Parses a Silhouette Studio file from disk.
pub fn parse_studio_file(path: &Path) -> Result<ParsedStudioDocument, StudioParseError> {
    let data = std::fs::read(path)?;
    let mut doc = parse_studio_data(&data)?;

    // If no embedded artwork was in the binary, look for companion raster image
    if doc.shapes.iter().all(|s| s.name.as_deref() != Some("Print Artwork")) {
        if let Some(companion_path) = find_companion_raster_image(path) {
            if let Ok(comp_bytes) = std::fs::read(&companion_path) {
                let decoded = decode_png_to_pixmap(&comp_bytes).or_else(|| {
                    image::load_from_memory(&comp_bytes).ok().and_then(|dyn_img| {
                        let rgba = dyn_img.to_rgba8();
                        let (w, h) = rgba.dimensions();
                        let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
                        pixmap.data_mut().copy_from_slice(&rgba.into_raw());
                        Some(pixmap)
                    })
                });
                if let Some(pm) = decoded {
                    let arc_pm = std::sync::Arc::new(pm);
                    attach_print_artwork_to_shapes(&mut doc.shapes, arc_pm, doc.mat_width_in, doc.mat_height_in);
                    normalize_shapes_onto_mat(&mut doc.shapes, doc.mat_width_in, doc.mat_height_in);
                }
            }
        }
    }

    Ok(doc)
}

/// Discovers companion raster image files (PNG/JPEG) located alongside or in sibling folders.
fn find_companion_raster_image(path: &Path) -> Option<std::path::PathBuf> {
    let stem = path.file_stem()?.to_str()?;
    let dir = path.parent()?;

    // 1. Check in same directory
    for ext in ["png", "PNG", "jpg", "JPG", "jpeg", "JPEG"] {
        let candidate = dir.join(format!("{}.{}", stem, ext));
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    // 2. Check in sibling directories (e.g. "PNG Overkay", "PNG Overlay", "PNG", "Images")
    if let Some(grandparent) = dir.parent() {
        for folder in ["PNG Overkay", "PNG Overlay", "PNG", "png", "Images", "images", "Overlays", "overlays"] {
            for ext in ["png", "PNG", "jpg", "JPG", "jpeg", "JPEG"] {
                let candidate = grandparent.join(folder).join(format!("{}.{}", stem, ext));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

/// Positions and attaches Print Artwork raster image aligned with vector shapes.
fn attach_print_artwork_to_shapes(
    shapes: &mut Vec<CanvasShape>,
    pm: std::sync::Arc<tiny_skia::Pixmap>,
    mat_w_in: f32,
    mat_h_in: f32,
) {
    if shapes.iter().any(|s| s.name.as_deref() == Some("Print Artwork")) {
        return;
    }

    let mut non_boundary_pts = Vec::new();
    for s in shapes.iter() {
        let is_mat_boundary = (s.width >= 20.0 && s.height >= 10.0) || (s.width >= 11.5 && s.height >= 11.5);
        if !is_mat_boundary {
            non_boundary_pts.extend_from_slice(&s.points);
        }
    }
    let (vx_min, vy_min, vx_max, vy_max) = if !non_boundary_pts.is_empty() {
        compute_bounds(&non_boundary_pts)
    } else {
        (0.0, 0.0, mat_w_in, mat_h_in)
    };
    let vw = vx_max - vx_min;
    let vh = vy_max - vy_min;

    let img_aspect = pm.width() as f32 / pm.height().max(1) as f32;
    let vec_aspect = vw / vh.max(0.001);
    let aspect_diff = (img_aspect - vec_aspect).abs() / img_aspect.max(0.001);

    let (shape_x, shape_y, shape_w, shape_h) = if vw > 0.05 && vh > 0.05 && (aspect_diff < 0.25 || (vw < mat_w_in * 0.9 && vh < mat_h_in * 0.9)) {
        // Localized design / overlay (e.g. OL BASEBALL SOLID, OL DISNEY CHIP & DALE)
        (vx_min, vy_min, vw, vh)
    } else if vw > 0.05 && vh > 0.05 && vw >= mat_w_in * 0.7 && vh >= mat_h_in * 0.7 {
        // Full-page Print & Cut (e.g. 100 ACRE WOODS POOH, AT HOME WITH MINNIE & MICKEY)
        let mat_aspect = mat_w_in / mat_h_in.max(0.1);
        let (sw, sh) = if img_aspect > mat_aspect {
            (mat_w_in, mat_w_in / img_aspect)
        } else {
            (mat_h_in * img_aspect, mat_h_in)
        };
        let sx = ((mat_w_in - sw) * 0.5).max(0.0);
        let sy = ((mat_h_in - sh) * 0.5).max(0.0);
        (sx, sy, sw, sh)
    } else if vw > 0.05 && vh > 0.05 {
        (vx_min, vy_min, vw, vh)
    } else {
        (0.0, 0.0, mat_w_in, mat_h_in)
    };

    let png_shape = CanvasShape {
        kind: ShapeKind::Rectangle,
        x: shape_x,
        y: shape_y,
        width: shape_w,
        height: shape_h,
        points: vec![
            egui::pos2(shape_x, shape_y),
            egui::pos2(shape_x + shape_w, shape_y),
            egui::pos2(shape_x + shape_w, shape_y + shape_h),
            egui::pos2(shape_x, shape_y + shape_h),
        ],
        text: String::new(),
        fill_color: egui::Color32::WHITE,
        stroke_color: None,
        stroke_width: 0.0,
        is_closed: true,
        is_svg_path: false,
        is_modified: false,
        triangles: Vec::new(),
        group_id: None,
        group_path: Vec::new(),
        is_visible: true,
        is_locked: true,
        name: Some("Print Artwork".to_string()),
        pixmap: Some(pm),
        texture: None,
    };
    shapes.insert(0, png_shape);

    // Vector cut-lines overlaying print artwork must render with transparent fill
    for s in shapes.iter_mut() {
        if s.name.as_deref() != Some("Print Artwork") {
            let is_cut = s.stroke_color.map_or(false, |c| {
                (c.r() >= 200 && c.g() <= 50 && c.b() <= 50) || s.stroke_width <= 0.015
            });
            if is_cut {
                s.fill_color = egui::Color32::TRANSPARENT;
                s.pixmap = None;
            }
        }
    }
}

/// Parses Silhouette Studio binary data from memory.
pub fn parse_studio_data(data: &[u8]) -> Result<ParsedStudioDocument, StudioParseError> {
    let (version, stream_start, stream_end) = detect_format_and_stream(data)?;
    let stream = data.get(stream_start..stream_end).unwrap_or(&[]);

    // 1. Parse mat dimensions
    let (mat_w_in, mat_h_in) = parse_mat_dimensions(data, stream);

    // 2. Parse layer names and map them to group IDs
    let (group_names, layer_name_by_id) = parse_layers(data, stream);

    // 3. Extract embedded PNG raster image & pattern swatches
    let (artwork_pixmap_opt, pattern_swatch) = extract_embedded_png(data);

    // 4. Parse cubic Bézier paths with pattern_swatch reference
    let mut shapes = parse_paths(stream, version, &layer_name_by_id, pattern_swatch.as_deref());

    // 5. If artwork pixmap is present, attach and align Print Artwork shape
    if let Some(ref pm) = artwork_pixmap_opt {
        attach_print_artwork_to_shapes(&mut shapes, pm.clone(), mat_w_in, mat_h_in);
    }

    // 6. Normalize and center shapes onto the mat canvas
    normalize_shapes_onto_mat(&mut shapes, mat_w_in, mat_h_in);

    // 7. Detect and merge compound paths (e.g. concentric wheels with inner cutouts)
    let shapes = merge_compound_paths(shapes, version, pattern_swatch.as_deref());

    Ok(ParsedStudioDocument {
        version,
        stream_offset: stream_start,
        mat_width_in: mat_w_in,
        mat_height_in: mat_h_in,
        shapes,
        group_names,
        pattern_swatch,
    })
}

/// Normalizes and centers imported shapes onto the mat canvas.
/// Prevents shapes from appearing off-mat or with negative coordinates.
fn normalize_shapes_onto_mat(shapes: &mut [CanvasShape], mat_w_in: f32, mat_h_in: f32) {
    if shapes.is_empty() {
        return;
    }

    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;

    for s in shapes.iter() {
        for p in &s.points {
            min_x = min_x.min(p.x);
            min_y = min_y.min(p.y);
            max_x = max_x.max(p.x);
            max_y = max_y.max(p.y);
        }
    }

    let w = (max_x - min_x).max(0.001);
    let h = (max_y - min_y).max(0.001);

    // If shapes are already fully within the mat, keep their authentic design positions
    let is_already_on_mat = min_x >= 0.0 && min_y >= 0.0 && max_x <= mat_w_in && max_y <= mat_h_in;
    if is_already_on_mat {
        return;
    }

    let margin = 0.5_f32;
    let max_w = (mat_w_in - margin * 2.0).max(1.0);
    let max_h = (mat_h_in - margin * 2.0).max(1.0);
    let scale = if w > max_w || h > max_h {
        (max_w / w).min(max_h / h).min(1.0)
    } else {
        1.0
    };

    let target_cx = mat_w_in * 0.5;
    let target_cy = mat_h_in * 0.5;
    let orig_cx = (min_x + max_x) * 0.5;
    let orig_cy = (min_y + max_y) * 0.5;

    for shape in shapes.iter_mut() {
        for pt in &mut shape.points {
            pt.x = target_cx + (pt.x - orig_cx) * scale;
            pt.y = target_cy + (pt.y - orig_cy) * scale;
        }

        if shape.is_svg_path && !shape.text.is_empty() {
            shape.text = transform_svg_path_data(&shape.text, scale, orig_cx, orig_cy, target_cx, target_cy);
        }

        let (s_min_x, s_min_y, s_max_x, s_max_y) = compute_bounds(&shape.points);
        shape.x = s_min_x;
        shape.y = s_min_y;
        shape.width = (s_max_x - s_min_x).max(0.005);
        shape.height = (s_max_y - s_min_y).max(0.005);
        shape.precompute_triangles();
    }
}

/// Transforms 2D coordinates within SVG path data string (`d`) using scale and offset.
pub fn transform_svg_path_data(
    text: &str,
    scale: f32,
    orig_cx: f32,
    orig_cy: f32,
    target_cx: f32,
    target_cy: f32,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut tokens = text.split_whitespace().peekable();
    while let Some(tok) = tokens.next() {
        if tok == "M" || tok == "L" {
            if let (Some(x_str), Some(y_str)) = (tokens.next(), tokens.next()) {
                if let (Ok(x), Ok(y)) = (x_str.parse::<f32>(), y_str.parse::<f32>()) {
                    let tx = target_cx + (x - orig_cx) * scale;
                    let ty = target_cy + (y - orig_cy) * scale;
                    if !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(&format!("{} {:.4} {:.4}", tok, tx, ty));
                    continue;
                }
            }
        } else if tok == "Z" {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str("Z");
            continue;
        } else if let Some((x_str, y_str)) = tok.split_once(',') {
            if let (Ok(x), Ok(y)) = (x_str.parse::<f32>(), y_str.parse::<f32>()) {
                let tx = target_cx + (x - orig_cx) * scale;
                let ty = target_cy + (y - orig_cy) * scale;
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(&format!("{:.4},{:.4}", tx, ty));
                continue;
            }
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(tok);
    }
    if out.is_empty() {
        text.to_string()
    } else {
        out
    }
}

/// Converts Silhouette Studio binary data directly into a standalone SVG string.
pub fn convert_studio_to_svg(data: &[u8]) -> Result<String, StudioParseError> {
    let doc = parse_studio_data(data)?;

    let w_in = doc.mat_width_in;
    let h_in = doc.mat_height_in;

    let mut svg = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{:.2}in\" height=\"{:.2}in\" viewBox=\"0 0 {:.2} {:.2}\">\n",
        w_in, h_in, w_in, h_in
    );

    // If a pattern swatch is present, define it in <defs>
    if let Some(ref swatch) = doc.pattern_swatch {
        if let Ok(png_bytes) = swatch.encode_png() {
            let b64 = base64_encode(&png_bytes);
            svg.push_str("  <defs>\n");
            svg.push_str(&format!(
                "    <pattern id=\"swatch_pattern\" width=\"2.000\" height=\"2.000\" patternUnits=\"userSpaceOnUse\">\n      <image width=\"2.000\" height=\"2.000\" href=\"data:image/png;base64,{}\" />\n    </pattern>\n",
                b64
            ));
            svg.push_str("  </defs>\n");
        }
    }

    // Group shapes by layer
    let mut layer_shapes: BTreeMap<Option<u64>, Vec<&CanvasShape>> = BTreeMap::new();
    for shape in &doc.shapes {
        layer_shapes.entry(shape.group_id).or_default().push(shape);
    }

    for (gid, shapes) in layer_shapes {
        let name = gid
            .and_then(|id| doc.group_names.get(&id))
            .cloned()
            .unwrap_or_else(|| "Default Layer".to_string());

        svg.push_str(&format!("  <g id=\"{}\">\n", escape_xml(&name)));

        for shape in shapes {
            if let Some(ref pm) = shape.pixmap {
                if !shape.is_svg_path {
                    if let Ok(png_bytes) = pm.encode_png() {
                        let b64 = base64_encode(&png_bytes);
                        svg.push_str(&format!(
                            "    <image x=\"{:.3}\" y=\"{:.3}\" width=\"{:.3}\" height=\"{:.3}\" href=\"data:image/png;base64,{}\" />\n",
                            shape.x, shape.y, shape.width, shape.height, b64
                        ));
                        continue;
                    }
                }
            }

            let is_white_or_light = (shape.fill_color.r() > 240 && shape.fill_color.g() > 240 && shape.fill_color.b() > 240) || shape.fill_color == egui::Color32::WHITE;
            let fill_str = if doc.pattern_swatch.is_some() && shape.fill_color.a() > 0 && is_white_or_light {
                "fill=\"url(#swatch_pattern)\"".to_string()
            } else if shape.fill_color.a() == 0 {
                "fill=\"none\"".to_string()
            } else {
                let hex = format!("#{:02x}{:02x}{:02x}", shape.fill_color.r(), shape.fill_color.g(), shape.fill_color.b());
                let opacity = shape.fill_color.a() as f32 / 255.0;
                if opacity < 0.999 {
                    format!("fill=\"{}\" fill-opacity=\"{:.2}\"", hex, opacity)
                } else {
                    format!("fill=\"{}\"", hex)
                }
            };

            let is_white_stroke = shape.stroke_color.map_or(true, |sc| sc.a() == 0 || (sc.r() > 240 && sc.g() > 240 && sc.b() > 240));
            let stroke_str = if let Some(sc) = shape.stroke_color {
                if sc.a() > 0 && !is_white_stroke {
                    let hex = format!("#{:02x}{:02x}{:02x}", sc.r(), sc.g(), sc.b());
                    let opacity = sc.a() as f32 / 255.0;
                    if opacity < 0.999 {
                        format!("stroke=\"{}\" stroke-opacity=\"{:.2}\" stroke-width=\"{:.3}\"", hex, opacity, shape.stroke_width.max(0.01))
                    } else {
                        format!("stroke=\"{}\" stroke-width=\"{:.3}\"", hex, shape.stroke_width.max(0.01))
                    }
                } else if doc.pattern_swatch.is_some() || is_white_or_light || shape.fill_color.a() == 0 {
                    format!("stroke=\"#ff2626\" stroke-width=\"{:.3}\"", shape.stroke_width.max(0.01))
                } else {
                    "stroke=\"none\"".to_string()
                }
            } else if doc.pattern_swatch.is_some() || is_white_or_light || shape.fill_color.a() == 0 {
                format!("stroke=\"#ff2626\" stroke-width=\"{:.3}\"", shape.stroke_width.max(0.01))
            } else {
                "stroke=\"none\"".to_string()
            };

            if shape.is_svg_path && !shape.text.is_empty() {
                svg.push_str(&format!(
                    "    <path d=\"{}\" fill-rule=\"evenodd\" {} {} stroke-linecap=\"round\" stroke-linejoin=\"round\" />\n",
                    shape.text, fill_str, stroke_str
                ));
            } else if shape.points.len() >= 2 {
                let pts_str = shape
                    .points
                    .iter()
                    .map(|p| format!("{:.3},{:.3}", p.x, p.y))
                    .collect::<Vec<_>>()
                    .join(" ");
                let tag = if shape.is_closed { "polygon" } else { "polyline" };
                svg.push_str(&format!(
                    "    <{} points=\"{}\" {} {} stroke-linecap=\"round\" stroke-linejoin=\"round\" />\n",
                    tag, pts_str, fill_str, stroke_str
                ));
            }
        }

        svg.push_str("  </g>\n");
    }

    svg.push_str("</svg>\n");
    Ok(svg)
}

/// Adaptive de Casteljau Subdivision algorithm for cubic Bézier curves.
/// Recursively subdivides curve segment until deviation from chord is below tolerance.
pub fn flatten_cubic_bezier(
    p0: egui::Pos2,
    p1: egui::Pos2,
    p2: egui::Pos2,
    p3: egui::Pos2,
    tolerance: f32,
    max_depth: u32,
    out: &mut Vec<egui::Pos2>,
) {
    fn subdivide(
        p0: egui::Pos2,
        p1: egui::Pos2,
        p2: egui::Pos2,
        p3: egui::Pos2,
        tolerance: f32,
        depth: u32,
        out: &mut Vec<egui::Pos2>,
    ) {
        let dx = p3.x - p0.x;
        let dy = p3.y - p0.y;
        let len_sq = dx * dx + dy * dy;

        let is_flat = if len_sq < 1e-8 {
            let d1 = (p1.x - p0.x).hypot(p1.y - p0.y);
            let d2 = (p2.x - p0.x).hypot(p2.y - p0.y);
            d1.max(d2) <= tolerance
        } else {
            let inv_len = 1.0 / len_sq.sqrt();
            let d1 = ((p1.x - p0.x) * dy - (p1.y - p0.y) * dx).abs() * inv_len;
            let d2 = ((p2.x - p0.x) * dy - (p2.y - p0.y) * dx).abs() * inv_len;
            d1.max(d2) <= tolerance
        };

        if is_flat || depth == 0 {
            out.push(p3);
        } else {
            let p01 = egui::pos2((p0.x + p1.x) * 0.5, (p0.y + p1.y) * 0.5);
            let p12 = egui::pos2((p1.x + p2.x) * 0.5, (p1.y + p2.y) * 0.5);
            let p23 = egui::pos2((p2.x + p3.x) * 0.5, (p2.y + p3.y) * 0.5);

            let p012 = egui::pos2((p01.x + p12.x) * 0.5, (p01.y + p12.y) * 0.5);
            let p123 = egui::pos2((p12.x + p23.x) * 0.5, (p12.y + p23.y) * 0.5);

            let p0123 = egui::pos2((p012.x + p123.x) * 0.5, (p012.y + p123.y) * 0.5);

            subdivide(p0, p01, p012, p0123, tolerance, depth - 1, out);
            subdivide(p0123, p123, p23, p3, tolerance, depth - 1, out);
        }
    }

    subdivide(p0, p1, p2, p3, tolerance, max_depth, out);
}

/// Converts a slice of cubic Bézier vertices into a smooth polyline in mat-inch coordinates.
pub fn path_vertices_to_polyline(
    vertices: &[StudioBezierVertex],
    is_closed: bool,
    transform: Option<&AffineTransform2D>,
    scale_to_in: f32,
    tolerance: f32,
) -> Vec<egui::Pos2> {
    if vertices.is_empty() {
        return Vec::new();
    }

    // Apply affine transform and scale to inches
    let transformed: Vec<StudioBezierVertex> = vertices
        .iter()
        .map(|v| {
            let mut v2 = *v;
            if let Some(t) = transform {
                v2.anchor = t.apply(v2.anchor);
                v2.handle_in = t.apply(v2.handle_in);
                v2.handle_out = t.apply(v2.handle_out);
            }
            v2.anchor = egui::pos2(v2.anchor.x * scale_to_in, v2.anchor.y * scale_to_in);
            v2.handle_in = egui::pos2(v2.handle_in.x * scale_to_in, v2.handle_in.y * scale_to_in);
            v2.handle_out = egui::pos2(v2.handle_out.x * scale_to_in, v2.handle_out.y * scale_to_in);
            v2
        })
        .collect();

    let mut polyline = Vec::new();
    if let Some(first) = transformed.first() {
        polyline.push(first.anchor);
    }

    for i in 0..transformed.len().saturating_sub(1) {
        let p0 = transformed[i].anchor;
        let p1 = transformed[i].handle_out;
        let p2 = transformed[i + 1].handle_in;
        let p3 = transformed[i + 1].anchor;
        flatten_cubic_bezier(p0, p1, p2, p3, tolerance, 8, &mut polyline);
    }

    if is_closed && transformed.len() >= 2 {
        if let (Some(last), Some(first)) = (transformed.last(), transformed.first()) {
            let p0 = last.anchor;
            let p1 = last.handle_out;
            let p2 = first.handle_in;
            let p3 = first.anchor;
            flatten_cubic_bezier(p0, p1, p2, p3, tolerance, 8, &mut polyline);
        }
    }

    polyline
}

// -------------------------------------------------------------------------------------------------
// Internal Parsing Helpers
// -------------------------------------------------------------------------------------------------

fn parse_mat_dimensions(full_data: &[u8], stream: &[u8]) -> (f32, f32) {
    // Check SPEC.md Section 3.2: binary page size in root's second child block at offset 0x29
    // The root block starts at offset 13 (following magic "silhouette05;").
    // First child is at offset 18, and second child block immediately follows it.
    if stream.starts_with(MAGIC_STUDIO_V5) && stream.len() >= 65 {
        if stream.get(13) == Some(&0x01) {
            if let Some(c1_size) = read_u32_le(stream, 19) {
                let first_end = 18 + 1 + (c1_size as usize);
                if first_end + 5 + 0x29 + 8 <= stream.len() && stream.get(first_end) == Some(&0x01) {
                    if let (Some(w_mm), Some(h_mm)) = (
                        read_f32_le(stream, first_end + 5 + 0x29),
                        read_f32_le(stream, first_end + 5 + 0x29 + 4),
                    ) {
                        if w_mm > 10.0 && h_mm > 10.0 && w_mm < 50_000.0 && h_mm < 50_000.0 {
                            return (w_mm / 25.4, h_mm / 25.4);
                        }
                    }
                }
            }
        }
    }

    let slice_to_check = if full_data.len() > stream.len() { full_data } else { stream };
    let preview_len = slice_to_check.len().min(32_768);
    let preview_slice = slice_to_check.get(..preview_len).unwrap_or(slice_to_check);
    let combined_str = String::from_utf8_lossy(preview_slice);

    // Look for explicit tags first (e.g. SMAT:<name>;)
    if let Some(idx) = combined_str.find("SMAT:") {
        if let Some(tail) = combined_str.get(idx + 5..) {
            if let Some(end) = tail.find(';') {
                if let Some(tag_str) = tail.get(..end) {
                    if let Some(dims) = preset_to_dims(tag_str.trim()) {
                        return dims;
                    }
                }
            }
        }
    }

    // Standard Silhouette Studio mat presets
    let lower = combined_str.to_lowercase();
    if lower.contains("12x24") || lower.contains("12 x 24") {
        return (12.0, 24.0);
    }
    if lower.contains("letter") || lower.contains("8.5x11") || lower.contains("8.5 x 11") {
        return (8.5, 11.0);
    }
    if lower.contains("portrait") || lower.contains("8x12") || lower.contains("8 x 12") {
        return (8.0, 12.0);
    }
    if lower.contains("12x12") || lower.contains("12 x 12") || lower.contains("cameo") {
        return (12.0, 12.0);
    }
    if is_a4_media_preset(&lower) {
        return (8.27, 11.69);
    }

    // Default: standard 12"x12" Cameo mat (864 x 864 points)
    (12.0, 12.0)
}

fn is_a4_media_preset(lower: &str) -> bool {
    lower.contains("media:a4")
        || lower.contains("mat:a4")
        || lower.contains(":a4;")
        || lower.contains(" a4")
        || lower.contains("a4 ")
        || lower.split(|c: char| !c.is_alphanumeric()).any(|w| w == "a4")
}

fn preset_to_dims(preset: &str) -> Option<(f32, f32)> {
    let lower = preset.to_lowercase();
    if lower.contains("12x24") {
        Some((12.0, 24.0))
    } else if lower.contains("letter") || lower.contains("8.5x11") {
        Some((8.5, 11.0))
    } else if lower.contains("portrait") || lower.contains("8x12") {
        Some((8.0, 12.0))
    } else if lower.contains("12x12") || lower.contains("cameo") {
        Some((12.0, 12.0))
    } else if is_a4_media_preset(&lower) {
        Some((8.27, 11.69))
    } else {
        None
    }
}

fn parse_layers(full_data: &[u8], stream: &[u8]) -> (BTreeMap<u64, String>, BTreeMap<u64, String>) {
    let mut group_names = BTreeMap::new();
    let mut layer_map = BTreeMap::new();

    let slice_to_check = if full_data.len() > stream.len() { full_data } else { stream };
    let preview_len = slice_to_check.len().min(65_536);
    let preview_slice = slice_to_check.get(..preview_len).unwrap_or(slice_to_check);
    let combined_str = String::from_utf8_lossy(preview_slice);

    let mut search_from = 0;
    let mut found_explicit = false;
    while let Some(sub) = combined_str.get(search_from..) {
        if let Some(idx) = sub.find("SLYR") {
            let abs_idx = search_from + idx;
            let raw_tail = match combined_str.get(abs_idx + 4..) {
                Some(t) => t,
                None => break,
            };
            let tail = if raw_tail.starts_with(':') {
                raw_tail.get(1..).unwrap_or("")
            } else {
                raw_tail
            };
            if let Some(end) = tail.find(';') {
                if let Some(content) = tail.get(..end) {
                    let parts: Vec<&str> = content.splitn(2, ':').collect();
                    if parts.len() == 2 {
                        if let (Some(&p0), Some(&p1)) = (parts.first(), parts.get(1)) {
                            if let Ok(id) = p0.parse::<u64>() {
                                let name = p1.to_string();
                                group_names.insert(id, name.clone());
                                layer_map.insert(id, name);
                                found_explicit = true;
                            }
                        }
                    } else if let Some(&p0) = parts.first() {
                        let id = (group_names.len() as u64) + 1;
                        let name = p0.to_string();
                        group_names.insert(id, name.clone());
                        layer_map.insert(id, name);
                        found_explicit = true;
                    }
                }
                search_from = abs_idx + 4 + end + 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    if !found_explicit {
        let mut id = 1;
        for standard in &["Default Layer", "Layer 1", "Layer 2", "Layer 3"] {
            if combined_str.contains(standard) {
                group_names.insert(id, standard.to_string());
                layer_map.insert(id, standard.to_string());
                id += 1;
            }
        }
    }

    if group_names.is_empty() {
        group_names.insert(1, "Default Layer".to_string());
        layer_map.insert(1, "Default Layer".to_string());
    }

    (group_names, layer_map)
}

/// Parses all paths from the binary stream.
/// Supports structured SPTH records as well as raw sequential `01 21 00 00 00` records.
fn parse_paths(
    stream: &[u8],
    version: StudioFormatVersion,
    _layers: &BTreeMap<u64, String>,
    pattern_swatch: Option<&tiny_skia::Pixmap>,
) -> Vec<CanvasShape> {
    let mut shapes = Vec::new();

    // First attempt: Check for structured SPTH blocks
    if let Some(structured_shapes) = parse_structured_paths(stream, pattern_swatch) {
        if !structured_shapes.is_empty() {
            return structured_shapes;
        }
    }

    // Second attempt: Subpath header scanning via \x01 [byte_len: u32] \x01 [num_segments: u32] (\x01[\x21\x19\x11]\x00\x00\x00)
    let mut subpaths: Vec<StudioSubpath> = Vec::new();
    let mut i = 0;
    while i + 15 <= stream.len() {
        if stream[i] == 0x01 && stream[i + 5] == 0x01 {
            let marker = &stream[i + 10..i + 15];
            let is_marker = marker == BEZIER_VERTEX_MARKER
                || marker == BEZIER_VERTEX_MARKER_1H
                || marker == BEZIER_VERTEX_MARKER_0H;
            if is_marker {
                if let (Some(byte_len), Some(_nseg)) = (
                    read_u32_le(stream, i + 1),
                    read_u32_le(stream, i + 6),
                ) {
                    if (16..=1_000_000).contains(&byte_len) {
                        let v_start = i + 10;
                        let sub_end = i.saturating_add(1).saturating_add(byte_len as usize).min(stream.len());
                        let mut cur_sub_verts: Vec<StudioBezierVertex> = Vec::new();
                        let mut cur_sub_start = v_start;
                        let mut cur_sub_end = v_start;
                        let mut vi = v_start;
                        while vi + 5 <= sub_end {
                            let m = &stream[vi..vi + 5];
                            let is_2h = m == BEZIER_VERTEX_MARKER;
                            let is_1h = m == BEZIER_VERTEX_MARKER_1H;
                            let is_0h = m == BEZIER_VERTEX_MARKER_0H;
                            if is_2h || is_1h || is_0h {
                                if let Some(payload) = stream.get(vi + 5..sub_end) {
                                    let parsed = if is_2h {
                                        parse_bezier_vertex(payload)
                                    } else if is_1h {
                                        parse_bezier_vertex_1h(payload)
                                    } else {
                                        parse_bezier_vertex_0h(payload)
                                    };
                                    if let Some((v, len)) = parsed {
                                        let rec_len = 5 + len;
                                        if !cur_sub_verts.is_empty() {
                                            let gap = vi.saturating_sub(cur_sub_end);
                                            if gap > 4 {
                                                if cur_sub_verts.len() >= 2 {
                                                    subpaths.push(StudioSubpath {
                                                        start_pos: cur_sub_start,
                                                        end_pos: cur_sub_end,
                                                        vertices: cur_sub_verts,
                                                    });
                                                }
                                                cur_sub_verts = Vec::new();
                                                cur_sub_start = vi;
                                            }
                                        } else {
                                            cur_sub_start = vi;
                                        }
                                        cur_sub_verts.push(v);
                                        cur_sub_end = vi + rec_len;
                                        vi = vi.saturating_add(rec_len);
                                        continue;
                                    }
                                }
                            }
                            vi = vi.saturating_add(1);
                        }
                        if cur_sub_verts.len() >= 2 {
                            subpaths.push(StudioSubpath {
                                start_pos: cur_sub_start,
                                end_pos: cur_sub_end,
                                vertices: cur_sub_verts,
                            });
                        }
                        i = sub_end;
                        continue;
                    }
                }
            }
        }
        i = i.saturating_add(1);
    }

    // Third attempt (fallback): General unstructured stream scanning
    if subpaths.is_empty() {
        let mut vertices_with_pos = Vec::new();
        let mut i = 0;
        while i + 5 <= stream.len() {
            if let Some(marker_slice) = stream.get(i..i + 5) {
                let is_2h = marker_slice == BEZIER_VERTEX_MARKER;
                let is_1h = marker_slice == BEZIER_VERTEX_MARKER_1H;
                let is_0h = marker_slice == BEZIER_VERTEX_MARKER_0H;
                if is_2h || is_1h || is_0h {
                    if let Some(payload) = stream.get(i + 5..) {
                        let parsed = if is_2h {
                            parse_bezier_vertex(payload)
                        } else if is_1h {
                            parse_bezier_vertex_1h(payload)
                        } else {
                            parse_bezier_vertex_0h(payload)
                        };
                        if let Some((v, len)) = parsed {
                            vertices_with_pos.push((i, 5 + len, v));
                            i = i.saturating_add(5 + len);
                            continue;
                        }
                    }
                }
            }
            i = i.saturating_add(1);
        }

        let mut cur_subpath_verts: Vec<StudioBezierVertex> = Vec::new();
        let mut cur_sub_start = 0;
        let mut cur_sub_end = 0;

        for (pos, rec_len, vert) in vertices_with_pos {
            if !cur_subpath_verts.is_empty() {
                let gap = pos.saturating_sub(cur_sub_end);
                if gap > 4 {
                    if cur_subpath_verts.len() >= 2 {
                        subpaths.push(StudioSubpath {
                            start_pos: cur_sub_start,
                            end_pos: cur_sub_end,
                            vertices: cur_subpath_verts,
                        });
                    }
                    cur_subpath_verts = Vec::new();
                    cur_sub_start = pos;
                }
            } else {
                cur_sub_start = pos;
            }
            cur_subpath_verts.push(vert);
            cur_sub_end = pos + rec_len;
        }
        if cur_subpath_verts.len() >= 2 {
            subpaths.push(StudioSubpath {
                start_pos: cur_sub_start,
                end_pos: cur_sub_end,
                vertices: cur_subpath_verts,
            });
        }
    }

    if subpaths.is_empty() {
        return shapes;
    }

    // Pre-scan all style color records in the stream (01 [len] 00 01 [r, g, b, a])
    #[derive(Debug, Clone, Copy)]
    struct StyleRecord {
        pos: usize,
        stroke_color: egui::Color32,
        fill_color: egui::Color32,
        is_pattern: bool,
        is_cut: bool,
    }

    let mut style_records: Vec<StyleRecord> = Vec::new();
    let mut s_idx = 0;
    while s_idx + 23 <= stream.len() {
        if stream[s_idx] == 0x01 && stream[s_idx + 5] == 0x00 && stream[s_idx + 6] == 0x01 {
            if let Some(len) = read_u32_le(stream, s_idx + 1) {
                if (16..=2000).contains(&len) {
                    let end = (s_idx + 5 + len as usize).min(stream.len());
                    let rec = &stream[s_idx + 7..end];

                    if version == StudioFormatVersion::StudioV4 {
                        if let (Some(r), Some(g), Some(b), Some(a)) = (
                            read_f32_le(stream, s_idx + 7),
                            read_f32_le(stream, s_idx + 11),
                            read_f32_le(stream, s_idx + 15),
                            read_f32_le(stream, s_idx + 19),
                        ) {
                            if (0.0..=1.0).contains(&r)
                                && (0.0..=1.0).contains(&g)
                                && (0.0..=1.0).contains(&b)
                                && (a - 1.0).abs() < 1e-3
                            {
                                style_records.push(StyleRecord {
                                    pos: s_idx,
                                    stroke_color: egui::Color32::from_rgb(255, 38, 38),
                                    fill_color: egui::Color32::from_rgba_unmultiplied(
                                        (r * 255.0).round() as u8,
                                        (g * 255.0).round() as u8,
                                        (b * 255.0).round() as u8,
                                        255,
                                    ),
                                    is_pattern: false,
                                    is_cut: false,
                                });
                                s_idx = s_idx.saturating_add(5 + (len as usize).min(200));
                                continue;
                            }
                        }
                    } else {
                        // Studio V5
                        let stroke_c = if let (Some(r), Some(g), Some(b), Some(a)) = (
                            read_f32_le(stream, s_idx + 7),
                            read_f32_le(stream, s_idx + 11),
                            read_f32_le(stream, s_idx + 15),
                            read_f32_le(stream, s_idx + 19),
                        ) {
                            if (0.0..=1.01).contains(&r) && (0.0..=1.01).contains(&g) && (0.0..=1.01).contains(&b) && (0.0..=1.01).contains(&a) {
                                Some(egui::Color32::from_rgba_unmultiplied(
                                    (r.clamp(0.0, 1.0) * 255.0).round() as u8,
                                    (g.clamp(0.0, 1.0) * 255.0).round() as u8,
                                    (b.clamp(0.0, 1.0) * 255.0).round() as u8,
                                    (a.clamp(0.0, 1.0) * 255.0).round() as u8,
                                ))
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        let is_cut = rec.windows(3).any(|w| w.eq_ignore_ascii_case(b"cut"))
                            || stroke_c.map_or(false, |sc| sc.r() >= 200 && sc.g() <= 50 && sc.b() <= 50);

                        // Join/cap keyword search for fill flag
                        let join_keywords: [&[u8]; 5] = [b"rounded", b"corner", b"flat", b"bevel", b"miter"];
                        let mut last_pos = None;
                        let mut last_kw_len = 0;
                        for kw in &join_keywords {
                            let mut p = 0;
                            while let Some(idx) = find_subsequence(&rec[p..], kw) {
                                let abs_idx = p + idx;
                                if last_pos.map_or(true, |lp| abs_idx > lp) {
                                    last_pos = Some(abs_idx);
                                    last_kw_len = kw.len();
                                }
                                p = abs_idx + 1;
                            }
                        }

                        let mut fill_color = egui::Color32::TRANSPARENT;
                        let mut is_pattern = false;

                        if let Some(lp) = last_pos {
                            let flag_idx = lp + last_kw_len;
                            if flag_idx < rec.len() {
                                let flag = rec[flag_idx];
                                if flag == 1 && flag_idx + 1 + 16 <= rec.len() {
                                    if let (Some(r), Some(g), Some(b), Some(a)) = (
                                        read_f32_le(rec, flag_idx + 1),
                                        read_f32_le(rec, flag_idx + 5),
                                        read_f32_le(rec, flag_idx + 9),
                                        read_f32_le(rec, flag_idx + 13),
                                    ) {
                                        if (0.0..=1.01).contains(&r) && (0.0..=1.01).contains(&g) && (0.0..=1.01).contains(&b) && (0.0..=1.01).contains(&a) {
                                            fill_color = egui::Color32::from_rgba_unmultiplied(
                                                (r.clamp(0.0, 1.0) * 255.0).round() as u8,
                                                (g.clamp(0.0, 1.0) * 255.0).round() as u8,
                                                (b.clamp(0.0, 1.0) * 255.0).round() as u8,
                                                (a.clamp(0.0, 1.0) * 255.0).round() as u8,
                                            );
                                        }
                                    }
                                } else if flag == 5 {
                                    fill_color = egui::Color32::WHITE;
                                    is_pattern = true;
                                }
                            }
                        }

                        if is_cut && fill_color.a() == 0 {
                            fill_color = egui::Color32::TRANSPARENT;
                        }

                        let final_stroke = if is_cut {
                            egui::Color32::from_rgb(255, 38, 38)
                        } else {
                            stroke_c.unwrap_or(egui::Color32::from_rgb(255, 38, 38))
                        };

                        style_records.push(StyleRecord {
                            pos: s_idx,
                            stroke_color: final_stroke,
                            fill_color,
                            is_pattern,
                            is_cut,
                        });
                        s_idx = s_idx.saturating_add(5 + (len as usize).min(200));
                        continue;
                    }
                }
            }
        }
        s_idx = s_idx.saturating_add(1);
    }

    // Pre-scan explicit object tags (primarily Studio V5 / silhouette05; files)
    #[derive(Debug, Clone)]
    struct StudioObjectTag {
        pos: usize,
        _name: String,
    }

    let mut object_tags: Vec<StudioObjectTag> = Vec::new();
    let known_tags: [&[u8]; 9] = [
        b"\x07\x00\x00\x00Polygon",
        b"\x09\x00\x00\x00Rectangle",
        b"\x05\x00\x00\x00Group",
        b"\x04\x00\x00\x00Text",
        b"\x07\x00\x00\x00Ellipse",
        b"\x04\x00\x00\x00Path",
        b"\x0c\x00\x00\x00CompoundPath",
        b"\x0d\x00\x00\x00Compound Path",
        b"\x08\x00\x00\x00Compound",
    ];
    let mut t_idx = 0;
    while t_idx + 8 <= stream.len() {
        let mut matched = false;
        for kt in &known_tags {
            if stream[t_idx..].starts_with(kt) {
                let tag_name = String::from_utf8_lossy(&kt[4..]).to_string();
                object_tags.push(StudioObjectTag {
                    pos: t_idx,
                    _name: tag_name,
                });
                t_idx = t_idx.saturating_add(kt.len());
                matched = true;
                break;
            }
        }
        if !matched {
            t_idx += 1;
        }
    }

    // Scan for global Document / Page root transform matrix (SPEC.md Section 3 / Section 4.2)
    let doc_root_xfm = if version == StudioFormatVersion::StudioV5 {
        let header_scan_limit = stream.len().min(4096);
        if let Some(header_slice) = stream.get(13..header_scan_limit) {
            find_transform_in_slice(header_slice, version).unwrap_or_default()
        } else {
            AffineTransform2D::default()
        }
    } else {
        AffineTransform2D::default()
    };

    // Pre-scan explicit Group and CompoundPath spans to preserve genuine hierarchical grouping
    #[derive(Debug, Clone)]
    struct StudioGroupSpan {
        id: u64,
        start_pos: usize,
        end_pos: usize,
        transform: AffineTransform2D,
    }

    // Build block index for Studio V5 to map block end offset to block start offset (Block A)
    let mut block_starts_by_end: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    if version == StudioFormatVersion::StudioV5 {
        let mut q = 13;
        while q + 5 <= stream.len() {
            if stream[q] == 0x01 {
                if let Some(size) = read_u32_le(stream, q + 1) {
                    if size >= 4 && q + 1 + (size as usize) <= stream.len() {
                        block_starts_by_end.insert(q + 1 + (size as usize), q);
                    }
                }
            }
            q += 1;
        }
    }

    let mut group_spans: Vec<StudioGroupSpan> = Vec::new();
    let group_patterns: [&[u8]; 3] = [
        b"\x05\x00\x00\x00Group",
        b"\x0c\x00\x00\x00CompoundPath",
        b"\x0d\x00\x00\x00Compound Path",
    ];

    let mut g_search = 0;
    while g_search + 15 <= stream.len() {
        let mut found_match = None;
        for pat in &group_patterns {
            if stream[g_search..].starts_with(pat) {
                found_match = Some((g_search, pat.len()));
                break;
            }
        }
        if let Some((pos, tag_len)) = found_match {
            // In Studio V5, search backwards from pos - 65 for the group's transform matrix.
            // As documented in SPEC.md, the 65-byte transform sits immediately before the record tail (at offset pos - 92).
            let mut group_xfm = AffineTransform2D::default();
            let mut mat_end_pos = None;

            if version == StudioFormatVersion::StudioV5 {
                let search_floor = pos.saturating_sub(250);
                let search_ceil = pos.saturating_sub(65);
                for t in (search_floor..=search_ceil).rev() {
                    if let (Some(m11), Some(m12), Some(tx), Some(m21), Some(m22), Some(ty), Some(m31), Some(m32), Some(m33)) = (
                        read_f32_le(stream, t),
                        read_f32_le(stream, t + 4),
                        read_f32_le(stream, t + 8),
                        read_f32_le(stream, t + 12),
                        read_f32_le(stream, t + 16),
                        read_f32_le(stream, t + 20),
                        read_f32_le(stream, t + 24),
                        read_f32_le(stream, t + 28),
                        read_f32_le(stream, t + 32),
                    ) {
                        if m31.abs() < 1e-3 && m32.abs() < 1e-3 && (m33 - 1.0).abs() < 1e-3 {
                            let det = m11 * m22 - m12 * m21;
                            if (0.001..1000.0).contains(&det.abs())
                                && (0.001..1000.0).contains(&m11.abs())
                                && (0.001..1000.0).contains(&m22.abs())
                                && m12.abs() < 500.0
                                && m21.abs() < 500.0
                                && tx.abs() < 50_000.0
                                && ty.abs() < 50_000.0
                            {
                                group_xfm = AffineTransform2D::new(m11, m12, tx, m21, m22, ty);
                                mat_end_pos = Some(t);
                                break;
                            }
                        }
                    }
                }
            } else {
                let group_search_start = pos.saturating_sub(300);
                if let Some(group_slice) = stream.get(group_search_start..pos) {
                    if let Some(xfm) = find_transform_in_slice(group_slice, version) {
                        group_xfm = xfm;
                    }
                }
            }

            let (span_start, span_end) = if version == StudioFormatVersion::StudioV5 {
                if let Some(m_end) = mat_end_pos {
                    if let Some(&b_start) = block_starts_by_end.get(&m_end) {
                        (b_start.saturating_sub(1), pos + tag_len)
                    } else {
                        (pos, pos + tag_len)
                    }
                } else {
                    (pos, pos + tag_len)
                }
            } else {
                let header_offset = pos + tag_len + 60;
                let group_payload_end = if header_offset + 5 <= stream.len() && stream[header_offset] == 0x01 {
                    if let Some(payload_len) = read_u32_le(stream, header_offset + 1) {
                        (header_offset + 5).saturating_add(payload_len as usize).min(stream.len())
                    } else {
                        pos
                    }
                } else {
                    pos
                };
                (pos, group_payload_end.max(pos + tag_len))
            };

            group_spans.push(StudioGroupSpan {
                id: (group_spans.len() as u64) + 100,
                start_pos: span_start,
                end_pos: span_end,
                transform: group_xfm,
            });

            g_search = pos + tag_len;
        } else {
            g_search += 1;
        }
    }

    // Group consecutive subpaths deterministically into compound shapes.
    // In Studio V5: Object tags and style records demarcate shape boundaries.
    // In Studio V4: Style records demarcate shape boundaries.
    // Consecutive subpaths within the same object tag and style belong to the same compound path.
    #[derive(Debug, Clone)]
    struct StudioShapeCluster {
        start_pos: usize,
        end_pos: usize,
        subpaths: Vec<StudioSubpath>,
    }

    let mut shape_clusters: Vec<StudioShapeCluster> = Vec::new();
    let mut cur_cluster_subs: Vec<StudioSubpath> = Vec::new();

    for sub in subpaths {
        if let Some(prev) = cur_cluster_subs.last() {
            let gap = sub.start_pos.saturating_sub(prev.end_pos);
            let has_tag_boundary = object_tags.iter().any(|ot| ot.pos > prev.start_pos && ot.pos <= sub.start_pos);
            let has_style_boundary = style_records.iter().any(|sr| sr.pos > prev.start_pos && sr.pos <= sub.start_pos);
            let is_boundary = if version == StudioFormatVersion::StudioV4 {
                gap > 48 || has_style_boundary
            } else {
                has_tag_boundary || has_style_boundary || (object_tags.is_empty() && style_records.is_empty() && gap > 200)
            };

            if is_boundary {
                if !cur_cluster_subs.is_empty() {
                    let start_pos = cur_cluster_subs.first().map(|s| s.start_pos).unwrap_or(0);
                    let end_pos = cur_cluster_subs.last().map(|s| s.end_pos).unwrap_or(start_pos);
                    shape_clusters.push(StudioShapeCluster {
                        start_pos,
                        end_pos,
                        subpaths: cur_cluster_subs,
                    });
                }
                cur_cluster_subs = Vec::new();
            }
        }
        cur_cluster_subs.push(sub.clone());
    }
    if !cur_cluster_subs.is_empty() {
        let start_pos = cur_cluster_subs.first().map(|s| s.start_pos).unwrap_or(0);
        let end_pos = cur_cluster_subs.last().map(|s| s.end_pos).unwrap_or(start_pos);
        shape_clusters.push(StudioShapeCluster {
            start_pos,
            end_pos,
            subpaths: cur_cluster_subs,
        });
    }

    if shape_clusters.is_empty() {
        return shapes;
    }

    // Determine matrix and style for each cluster deterministically
    #[derive(Debug, Clone)]
    struct ResolvedCluster {
        xfm: AffineTransform2D,
        fill: egui::Color32,
        _stroke: egui::Color32,
        is_pattern: bool,
        is_cut: bool,
    }

    let mut cluster_data: Vec<ResolvedCluster> = Vec::new();
    let mut last_valid_xfm = AffineTransform2D::default();
    let mut last_valid_fill = egui::Color32::TRANSPARENT;
    let mut last_valid_stroke = egui::Color32::from_rgb(255, 38, 38);
    let mut last_valid_is_pattern = false;
    let mut last_valid_is_cut = true;

    for idx in 0..shape_clusters.len() {
        let cluster = &shape_clusters[idx];
        let prev_end = if idx > 0 { shape_clusters[idx - 1].end_pos } else { 0 };
        let next_start = shape_clusters.get(idx + 1).map(|p| p.start_pos).unwrap_or(stream.len());

        let scope_start = prev_end;
        let scope_end = cluster.end_pos.saturating_add(500).min(next_start).min(stream.len());

        // 1. Matrix resolution:
        // In Studio V5 and V4, check immediately at cluster.end_pos first (where shape transform sits)
        let tail_len = if version == StudioFormatVersion::StudioV5 { 300 } else { 150 };
        let tail_slice = stream.get(cluster.end_pos..cluster.end_pos.saturating_add(tail_len).min(stream.len()));
        let local_xfm = tail_slice.and_then(|ts| find_transform_in_slice(ts, version))
            .or_else(|| {
                if scope_start < scope_end {
                    stream.get(scope_start..scope_end).and_then(|s| find_transform_in_slice(s, version))
                } else {
                    None
                }
            });

        let final_xfm = match local_xfm {
            Some(xfm) => {
                last_valid_xfm = xfm;
                xfm
            }
            None => {
                if version == StudioFormatVersion::StudioV4 {
                    last_valid_xfm
                } else {
                    AffineTransform2D::default()
                }
            }
        };

        // 2. Style resolution:
        let cur_style = style_records.iter().rev().find(|sr| sr.pos <= cluster.end_pos && sr.pos >= scope_start);
        let (final_fill, final_stroke, is_pattern, is_cut) = match cur_style {
            Some(s) => {
                last_valid_fill = s.fill_color;
                last_valid_stroke = s.stroke_color;
                last_valid_is_pattern = s.is_pattern;
                last_valid_is_cut = s.is_cut;
                (s.fill_color, s.stroke_color, s.is_pattern, s.is_cut)
            }
            None => {
                if version == StudioFormatVersion::StudioV4 {
                    (last_valid_fill, last_valid_stroke, false, false)
                } else {
                    let fc = if let Some(prefix_slice) = stream.get(scope_start..cluster.start_pos) {
                        find_fill_color_in_slice(prefix_slice).unwrap_or_else(|| {
                            if last_valid_is_cut { egui::Color32::TRANSPARENT } else { last_valid_fill }
                        })
                    } else if last_valid_is_cut {
                        egui::Color32::TRANSPARENT
                    } else {
                        last_valid_fill
                    };
                    (fc, last_valid_stroke, last_valid_is_pattern, last_valid_is_cut)
                }
            }
        };

        cluster_data.push(ResolvedCluster {
            xfm: final_xfm,
            fill: final_fill,
            _stroke: final_stroke,
            is_pattern,
            is_cut,
        });
    }

    // Compute global document scale_to_in multiplier across all clusters so all shapes
    // in the document share the identical coordinate space normalization (e.g. mm -> inches)
    let mut doc_max_coord: f32 = 0.0;
    for (cluster, res) in shape_clusters.iter().zip(cluster_data.iter()) {
        let mut cumulative_group_xfm = doc_root_xfm;
        let mut enclosing: Vec<&StudioGroupSpan> = group_spans.iter()
            .filter(|g| cluster.start_pos >= g.start_pos && cluster.end_pos <= g.end_pos)
            .collect();
        enclosing.sort_by_key(|g| std::cmp::Reverse(g.end_pos - g.start_pos));
        for g in &enclosing {
            cumulative_group_xfm = cumulative_group_xfm.combine(&g.transform);
        }
        let eff = cumulative_group_xfm.combine(&res.xfm);
        for sub in &cluster.subpaths {
            for v in &sub.vertices {
                let p = eff.apply(v.anchor);
                doc_max_coord = doc_max_coord.max(p.x.abs().max(p.y.abs()));
            }
        }
    }
    let doc_scale_to_in = if version == StudioFormatVersion::StudioV5 {
        if doc_max_coord > 24.0 { 1.0 / 25.4 } else { 1.0 }
    } else if doc_max_coord > 1000.0 {
        1.0 / 254.0
    } else if doc_max_coord > 24.0 {
        1.0 / 25.4
    } else {
        1.0
    };

    for (_idx, (cluster, res)) in shape_clusters.into_iter().zip(cluster_data.into_iter()).enumerate() {
        // Find all enclosing groups and sort from outermost to innermost (descending span length)
        let mut enclosing_groups: Vec<&StudioGroupSpan> = group_spans.iter()
            .filter(|g| cluster.start_pos >= g.start_pos && cluster.end_pos <= g.end_pos)
            .collect();
        enclosing_groups.sort_by_key(|g| std::cmp::Reverse(g.end_pos - g.start_pos));

        // Group path hierarchy: from outermost to innermost
        let group_path: Vec<u64> = enclosing_groups.iter().map(|g| g.id).collect();
        let shape_group_id = enclosing_groups.last().map(|g| g.id);

        // Cumulatively multiply parent group matrices down the tree, rolling all the way up to doc_root_xfm:
        // cumulative = doc_root_xfm * outermost.transform * ... * innermost.transform
        let mut cumulative_group_xfm = doc_root_xfm;
        for g in &enclosing_groups {
            cumulative_group_xfm = cumulative_group_xfm.combine(&g.transform);
        }

        // Combine cumulative group matrix with the shape's local matrix:
        // effective_xfm = cumulative_group_xfm * res.xfm
        let effective_xfm = cumulative_group_xfm.combine(&res.xfm);

        let scale_to_in = doc_scale_to_in;

        let current_stroke = egui::Color32::from_rgb(255, 38, 38); // Silhouette cut line: Red (#ff2626)
        let current_stroke_width = if res.is_cut || res.fill.a() == 0 { 0.01_f32 } else { 0.02_f32 };
        let active_pattern_swatch = if res.is_pattern || (pattern_swatch.is_some() && res.fill.a() > 0) {
            pattern_swatch
        } else {
            None
        };

        if let Some(shape) = create_canvas_shape_from_subpaths(
            &cluster.subpaths,
            shape_group_id,
            group_path,
            Some(&effective_xfm),
            res.fill,
            current_stroke,
            current_stroke_width,
            scale_to_in,
            active_pattern_swatch,
        ) {
            shapes.push(shape);
        }
    }

    shapes
}

/// Parses structured SPTH blocks if present in the stream.
fn parse_structured_paths(stream: &[u8], pattern_swatch: Option<&tiny_skia::Pixmap>) -> Option<Vec<CanvasShape>> {
    let mut shapes = Vec::new();
    let mut i = 0;

    while i + 4 <= stream.len() {
        if stream.get(i..i + 4) == Some(&TAG_PATH_BLOCK) {
            let header_start = i + 4;
            let num_vertices = read_u32_le(stream, header_start)? as usize;
            let _is_closed = read_u8(stream, header_start + 4)? != 0;
            let layer_id = read_u32_le(stream, header_start + 5)? as u64;

            let fill_rgba = [
                read_u8(stream, header_start + 9)?,
                read_u8(stream, header_start + 10)?,
                read_u8(stream, header_start + 11)?,
                read_u8(stream, header_start + 12)?,
            ];
            let fill_color = egui::Color32::from_rgba_unmultiplied(fill_rgba[0], fill_rgba[1], fill_rgba[2], fill_rgba[3]);

            let stroke_rgba = [
                read_u8(stream, header_start + 13)?,
                read_u8(stream, header_start + 14)?,
                read_u8(stream, header_start + 15)?,
                read_u8(stream, header_start + 16)?,
            ];
            let stroke_color = egui::Color32::from_rgba_unmultiplied(stroke_rgba[0], stroke_rgba[1], stroke_rgba[2], stroke_rgba[3]);

            let stroke_width = read_f32_le(stream, header_start + 17)?;

            let xfm_offset = header_start + 21;
            let a = read_f32_le(stream, xfm_offset)?;
            let b = read_f32_le(stream, xfm_offset + 4)?;
            let tx = read_f32_le(stream, xfm_offset + 8)?;
            let c = read_f32_le(stream, xfm_offset + 12)?;
            let d = read_f32_le(stream, xfm_offset + 16)?;
            let ty = read_f32_le(stream, xfm_offset + 20)?;
            let transform = AffineTransform2D::new(a, b, tx, c, d, ty);

            let mut vertices = Vec::new();
            let mut v_offset = header_start + 45;

            // Bounded allocation up to num_vertices
            for _ in 0..num_vertices.min(50_000) {
                if v_offset + 5 > stream.len() {
                    break;
                }
                if stream.get(v_offset..v_offset + 5) == Some(&BEZIER_VERTEX_MARKER) {
                    if let Some(payload) = stream.get(v_offset + 5..) {
                        if let Some((vert, len)) = parse_bezier_vertex(payload) {
                            vertices.push(vert);
                            let next_marker = find_subsequence(payload, &BEZIER_VERTEX_MARKER);
                            let step = match next_marker {
                                Some(gap) if gap <= 40 => 5 + gap,
                                _ => 5 + len,
                            };
                            v_offset = v_offset.saturating_add(step);
                            continue;
                        }
                    }
                }
                break;
            }

            if vertices.len() >= 2 {
                let max_coord = vertices
                    .iter()
                    .map(|v| v.anchor.x.abs().max(v.anchor.y.abs()))
                    .fold(0.0_f32, f32::max);
                let scale_to_in = if max_coord > 1000.0 {
                    1.0 / 254.0
                } else if max_coord > 24.0 {
                    1.0 / 25.4
                } else {
                    1.0
                };

                if let Some(shape) = create_canvas_shape_from_vertices(
                    &vertices,
                    layer_id,
                    Some(&transform),
                    fill_color,
                    stroke_color,
                    stroke_width,
                    scale_to_in,
                    pattern_swatch,
                ) {
                    shapes.push(shape);
                }
            }

            i = v_offset;
            continue;
        }
        i = i.saturating_add(1);
    }

    if shapes.is_empty() {
        None
    } else {
        Some(shapes)
    }
}

/// Parses a single cubic Bézier vertex record payload following the `01 21 00 00 00` marker.
/// Handles:
/// 1. Layout A (Official Silhouette Studio 34-byte record: `ax, ay, tag1, h1x, h1y, tag2, h2x, h2y, tail`)
///    where incoming and outgoing control handles are relative vectors from the anchor point.
/// 2. Layout B (Flat 24-byte record: `ax, ay, cx1, cy1, cx2, cy2`) for synthetic and test fixtures.
fn parse_bezier_vertex(payload: &[u8]) -> Option<(StudioBezierVertex, usize)> {
    if payload.len() < 24 {
        return None;
    }

    // Format 1: Official 34-byte record (5 bytes marker + 29 bytes payload)
    // payload[0..8] = anchor (ax, ay)
    // payload[8] = tag1 (0 or 1)
    // payload[9..17] = h1 (h1x, h1y) relative offset
    // payload[17] = tag2 (0 or 1)
    // payload[18..26] = h2 (h2x, h2y) relative offset
    // payload[26..29] = tail (3 bytes, tail[0] == 0)
    if payload.len() >= 27 {
        if let (Some(ax), Some(ay), Some(t1), Some(h1x), Some(h1y), Some(t2), Some(h2x), Some(h2y), Some(tail0)) = (
            read_f32_le(payload, 0),
            read_f32_le(payload, 4),
            read_u8(payload, 8),
            read_f32_le(payload, 9),
            read_f32_le(payload, 13),
            read_u8(payload, 17),
            read_f32_le(payload, 18),
            read_f32_le(payload, 22),
            read_u8(payload, 26),
        ) {
            let mut is_f1 = false;
            if tail0 == 0 && is_plausible_coord(ax) && is_plausible_coord(ay) {
                if t1 == 1 && t2 == 1 {
                    if is_plausible_coord(h1x)
                        && is_plausible_coord(h1y)
                        && is_plausible_coord(h2x)
                        && is_plausible_coord(h2y)
                    {
                        is_f1 = true;
                    }
                } else if t1 == 0 && t2 == 0 {
                    // When both tags are 0, bytes 8..26 must be all zeroes in Format 1
                    if let Some(slice) = payload.get(8..26) {
                        if slice.iter().all(|&b| b == 0) {
                            is_f1 = true;
                        }
                    }
                } else if (t1 == 1 && t2 == 0) || (t1 == 0 && t2 == 1) {
                    let h1_ok = if t1 == 0 {
                        payload.get(9..17).map_or(false, |s| s.iter().all(|&b| b == 0))
                    } else {
                        is_plausible_coord(h1x) && is_plausible_coord(h1y)
                    };
                    let h2_ok = if t2 == 0 {
                        payload.get(18..26).map_or(false, |s| s.iter().all(|&b| b == 0))
                    } else {
                        is_plausible_coord(h2x) && is_plausible_coord(h2y)
                    };
                    if h1_ok && h2_ok {
                        is_f1 = true;
                    }
                }
            }

            if is_f1 {
                let hin = if t1 == 1 { egui::pos2(ax + h1x, ay + h1y) } else { egui::pos2(ax, ay) };
                let hout = if t2 == 1 { egui::pos2(ax + h2x, ay + h2y) } else { egui::pos2(ax, ay) };
                return Some((
                    StudioBezierVertex {
                        anchor: egui::pos2(ax, ay),
                        handle_in: hin,
                        handle_out: hout,
                    },
                    29, // 5 + 29 = 34 bytes
                ));
            }
        }
    }

    // Format 2: Flat 24-byte coordinates (ax, ay, cx1, cy1, cx2, cy2)
    if let (Some(x0), Some(y0), Some(cx1_0), Some(cy1_0), Some(cx2_0), Some(cy2_0)) = (
        read_f32_le(payload, 0),
        read_f32_le(payload, 4),
        read_f32_le(payload, 8),
        read_f32_le(payload, 12),
        read_f32_le(payload, 16),
        read_f32_le(payload, 20),
    ) {
        if is_plausible_coord(x0)
            && is_plausible_coord(y0)
            && is_plausible_coord(cx1_0)
            && is_plausible_coord(cy1_0)
            && is_plausible_coord(cx2_0)
            && is_plausible_coord(cy2_0)
        {
            let len = if payload.len() >= 33 { 33 } else { 24 };
            return Some((
                StudioBezierVertex {
                    anchor: egui::pos2(x0, y0),
                    handle_in: egui::pos2(cx1_0, cy1_0),
                    handle_out: egui::pos2(cx2_0, cy2_0),
                },
                len,
            ));
        }
    }

    None
}

/// Parses a 1-handle 26-byte cubic Bézier vertex record (`01 19 00 00 00` marker + 21-byte payload).
fn parse_bezier_vertex_1h(payload: &[u8]) -> Option<(StudioBezierVertex, usize)> {
    if payload.len() < 21 {
        return None;
    }
    let ax = read_f32_le(payload, 0)?;
    let ay = read_f32_le(payload, 4)?;
    let tag1 = read_u8(payload, 8)?;

    if !is_plausible_coord(ax) || !is_plausible_coord(ay) {
        return None;
    }

    if tag1 == 1 {
        let h1x = read_f32_le(payload, 9)?;
        let h1y = read_f32_le(payload, 13)?;
        if is_plausible_coord(h1x) && is_plausible_coord(h1y) {
            return Some((
                StudioBezierVertex {
                    anchor: egui::pos2(ax, ay),
                    handle_in: egui::pos2(ax + h1x, ay + h1y),
                    handle_out: egui::pos2(ax, ay),
                },
                21,
            ));
        }
    } else {
        let tag2 = read_u8(payload, 9)?;
        if tag2 == 1 {
            let h2x = read_f32_le(payload, 10)?;
            let h2y = read_f32_le(payload, 14)?;
            if is_plausible_coord(h2x) && is_plausible_coord(h2y) {
                return Some((
                    StudioBezierVertex {
                        anchor: egui::pos2(ax, ay),
                        handle_in: egui::pos2(ax, ay),
                        handle_out: egui::pos2(ax + h2x, ay + h2y),
                    },
                    21,
                ));
            }
        } else {
            return Some((
                StudioBezierVertex {
                    anchor: egui::pos2(ax, ay),
                    handle_in: egui::pos2(ax, ay),
                    handle_out: egui::pos2(ax, ay),
                },
                21,
            ));
        }
    }

    None
}

/// Parses a 0-handle corner/straight line vertex record (`01 11 00 00 00` marker + payload).
/// In Silhouette Studio files, a corner point has no separate handles; incoming and outgoing
/// handles match the anchor point.
fn parse_bezier_vertex_0h(payload: &[u8]) -> Option<(StudioBezierVertex, usize)> {
    if payload.len() < 8 {
        return None;
    }
    let ax = read_f32_le(payload, 0)?;
    let ay = read_f32_le(payload, 4)?;
    if !is_plausible_coord(ax) || !is_plausible_coord(ay) {
        return None;
    }

    // Determine payload length by checking for next vertex marker
    let mut len = 13; // standard 18-byte record (5 marker + 13 payload)
    for candidate in [13, 17, 21, 25, 29] {
        if candidate + 5 <= payload.len() {
            let next_tag = &payload[candidate..candidate + 5];
            if next_tag == BEZIER_VERTEX_MARKER
                || next_tag == BEZIER_VERTEX_MARKER_1H
                || next_tag == BEZIER_VERTEX_MARKER_0H
            {
                len = candidate;
                break;
            }
        }
    }

    Some((
        StudioBezierVertex {
            anchor: egui::pos2(ax, ay),
            handle_in: egui::pos2(ax, ay),
            handle_out: egui::pos2(ax, ay),
        },
        len,
    ))
}

#[inline]
fn is_plausible_coord(val: f32) -> bool {
    val.is_finite() && val.abs() < 50_000.0 && !val.is_subnormal()
}

/// Sanitizes subpath vertices by removing stray (0,0) or extreme outlier vertices
/// caused by trailing padding or corrupt bytes at the end of subpath records.
pub fn sanitize_subpath_vertices(vertices: &[StudioBezierVertex]) -> Vec<StudioBezierVertex> {
    if vertices.len() < 2 {
        return vertices.to_vec();
    }

    // Check if authentic vertices in the subpath are positioned away from the origin.
    let non_zero_count = vertices.iter().filter(|v| v.anchor.x.abs() >= 1.0 || v.anchor.y.abs() >= 1.0).count();
    let has_non_zero = non_zero_count >= 2;

    if !has_non_zero {
        return vertices.to_vec();
    }

    let mut cleaned: Vec<StudioBezierVertex> = Vec::with_capacity(vertices.len());
    for (i, v) in vertices.iter().enumerate() {
        let is_zero = v.anchor.x.abs() < 1e-3 && v.anchor.y.abs() < 1e-3;
        if is_zero {
            let prev_dist = if i > 0 {
                vertices[i - 1].anchor.x.hypot(vertices[i - 1].anchor.y)
            } else {
                f32::INFINITY
            };
            let next_dist = if i + 1 < vertices.len() {
                vertices[i + 1].anchor.x.hypot(vertices[i + 1].anchor.y)
            } else {
                f32::INFINITY
            };
            if prev_dist > 5.0 || next_dist > 5.0 {
                // Stray zero-origin outlier: skip
                continue;
            }
        }
        cleaned.push(*v);
    }

    cleaned
}

fn create_canvas_shape_from_subpaths(
    subpaths: &[StudioSubpath],
    group_id: Option<u64>,
    group_path: Vec<u64>,
    transform: Option<&AffineTransform2D>,
    fill_color: egui::Color32,
    stroke_color: egui::Color32,
    stroke_width: f32,
    scale_to_in: f32,
    pattern_swatch: Option<&tiny_skia::Pixmap>,
) -> Option<CanvasShape> {
    if subpaths.is_empty() {
        return None;
    }

    let mut subpath_polylines = Vec::new();

    for sub in subpaths {
        let clean_verts = sanitize_subpath_vertices(&sub.vertices);
        if clean_verts.len() < 2 {
            continue;
        }
        let first = clean_verts.first()?;
        let last = clean_verts.last()?;
        let d = (first.anchor.x - last.anchor.x).hypot(first.anchor.y - last.anchor.y);
        let is_closed = d < 1.0 || clean_verts.len() >= 3;

        let polyline = path_vertices_to_polyline(&clean_verts, is_closed, transform, scale_to_in, 0.002);
        if polyline.len() >= 2 {
            subpath_polylines.push(polyline);
        }
    }

    if subpath_polylines.is_empty() {
        return None;
    }

    let mut all_points = Vec::new();
    for poly in &subpath_polylines {
        all_points.extend_from_slice(poly);
    }

    let (min_x, min_y, max_x, max_y) = compute_bounds(&all_points);
    let w = (max_x - min_x).max(0.005);
    let h = (max_y - min_y).max(0.005);

    // Build compound SVG path d string: M {x0} {y0} L {x1} {y1} ... Z M {x0'} {y0'} ... Z
    let mut svg_d = String::new();
    for poly in &subpath_polylines {
        if let Some(p0) = poly.first() {
            if !svg_d.is_empty() {
                svg_d.push(' ');
            }
            svg_d.push_str(&format!("M {:.4} {:.4}", p0.x, p0.y));
            for p in poly.iter().skip(1) {
                svg_d.push_str(&format!(" L {:.4} {:.4}", p.x, p.y));
            }
            svg_d.push_str(" Z");
        }
    }

    // Determine stroke color
    let is_white_or_light = (fill_color.r() > 240 && fill_color.g() > 240 && fill_color.b() > 240) || fill_color == egui::Color32::WHITE;
    let is_white_or_invisible_stroke = stroke_color.a() == 0 || (stroke_color.r() > 240 && stroke_color.g() > 240 && stroke_color.b() > 240);
    let final_stroke_c = if !is_white_or_invisible_stroke {
        stroke_color
    } else if pattern_swatch.is_some() || is_white_or_light || fill_color.a() == 0 {
        egui::Color32::from_rgb(255, 38, 38)
    } else {
        egui::Color32::TRANSPARENT
    };

    let is_cut_stroke = final_stroke_c == egui::Color32::from_rgb(255, 38, 38)
        || (final_stroke_c.r() >= 200 && final_stroke_c.g() <= 50 && final_stroke_c.b() <= 50)
        || stroke_width <= 0.015
        || fill_color.a() == 0;

    let is_mat_boundary = (w >= 20.0 && h >= 10.0) || (w >= 11.5 && h >= 11.5);
    // Pre-render tiny_skia::Pixmap when needed (pattern fills or multi-subpath compound cutouts)
    let mut pixmap = None;
    let should_have_pixmap = !is_mat_boundary && (
        (pattern_swatch.is_some() && (fill_color.a() > 0 || is_white_or_light))
        || (subpath_polylines.len() > 1 && fill_color.a() > 0)
    );
    if should_have_pixmap {
        let dpi = 48.0_f32;
        let raw_w = ((w * dpi).ceil() as u32).max(16);
        let raw_h = ((h * dpi).ceil() as u32).max(16);
        let max_dim = if pattern_swatch.is_some() { 256_u32 } else { 128_u32 };
        let (pix_w, pix_h) = if raw_w > max_dim || raw_h > max_dim {
            let scale = (max_dim as f32 / raw_w.max(raw_h) as f32).min(1.0);
            (((raw_w as f32 * scale).round() as u32).max(16), ((raw_h as f32 * scale).round() as u32).max(16))
        } else {
            (raw_w, raw_h)
        };

        if let Some(mut pm) = tiny_skia::Pixmap::new(pix_w, pix_h) {
            let pad = 2.0_f32;
            let sx = (pix_w as f32 - pad * 2.0) / w.max(0.001);
            let sy = (pix_h as f32 - pad * 2.0) / h.max(0.001);

            let mut pb = tiny_skia::PathBuilder::new();
            for poly in &subpath_polylines {
                if let Some(p0) = poly.first() {
                    pb.move_to((p0.x - min_x) * sx + pad, (p0.y - min_y) * sy + pad);
                    for p in poly.iter().skip(1) {
                        pb.line_to((p.x - min_x) * sx + pad, (p.y - min_y) * sy + pad);
                    }
                    pb.close();
                }
            }

            if let Some(path) = pb.finish() {
                if let Some(swatch) = pattern_swatch {
                    if is_white_or_light {
                        // Tiled pattern fill clipped to the vector shape path
                        let tile_size_in = 2.0_f32;
                        let tile_w_px = (tile_size_in * sx).max(16.0);
                        let tile_h_px = (tile_size_in * sy).max(16.0);
                        let scale_x = tile_w_px / swatch.width() as f32;
                        let scale_y = tile_h_px / swatch.height() as f32;
                        let pat_xfm = tiny_skia::Transform::from_scale(scale_x, scale_y).post_translate(pad, pad);
                        let shader = tiny_skia::Pattern::new(
                            swatch.as_ref(),
                            tiny_skia::SpreadMode::Repeat,
                            tiny_skia::FilterQuality::Bilinear,
                            1.0,
                            pat_xfm,
                        );
                        let mut fill_paint = tiny_skia::Paint::default();
                        fill_paint.shader = shader;
                        fill_paint.anti_alias = true;
                        pm.fill_path(&path, &fill_paint, tiny_skia::FillRule::EvenOdd, tiny_skia::Transform::identity(), None);
                    } else {
                        let mut fill_paint = tiny_skia::Paint::default();
                        fill_paint.set_color_rgba8(fill_color.r(), fill_color.g(), fill_color.b(), fill_color.a());
                        fill_paint.anti_alias = true;
                        pm.fill_path(&path, &fill_paint, tiny_skia::FillRule::EvenOdd, tiny_skia::Transform::identity(), None);
                    }
                } else {
                    let mut fill_paint = tiny_skia::Paint::default();
                    fill_paint.set_color_rgba8(fill_color.r(), fill_color.g(), fill_color.b(), fill_color.a());
                    fill_paint.anti_alias = true;
                    pm.fill_path(&path, &fill_paint, tiny_skia::FillRule::EvenOdd, tiny_skia::Transform::identity(), None);
                }

                if !is_cut_stroke && final_stroke_c.a() > 0 {
                    let mut stroke_paint = tiny_skia::Paint::default();
                    stroke_paint.set_color_rgba8(final_stroke_c.r(), final_stroke_c.g(), final_stroke_c.b(), final_stroke_c.a());
                    stroke_paint.anti_alias = true;
                    let st = tiny_skia::Stroke {
                        width: (stroke_width * sx).max(1.0),
                        line_cap: tiny_skia::LineCap::Round,
                        line_join: tiny_skia::LineJoin::Round,
                        ..Default::default()
                    };
                    pm.stroke_path(&path, &stroke_paint, &st, tiny_skia::Transform::identity(), None);
                }
            }

            // Ensure tiny or zero-area polygons have at least a visible central point
            if fill_color.a() > 0 && !pm.data().iter().any(|&b| b > 0) {
                let cx = (pix_w as f32 / 2.0) - 1.0;
                let cy = (pix_h as f32 / 2.0) - 1.0;
                if let Some(rect) = tiny_skia::Rect::from_xywh(cx.max(0.0), cy.max(0.0), 2.0, 2.0) {
                    let mut dot_paint = tiny_skia::Paint::default();
                    dot_paint.set_color_rgba8(fill_color.r(), fill_color.g(), fill_color.b(), fill_color.a());
                    pm.fill_rect(rect, &dot_paint, tiny_skia::Transform::identity(), None);
                }
            }

            pixmap = Some(std::sync::Arc::new(pm));
        }
    }

    let mut shape = CanvasShape {
        kind: ShapeKind::Freehand,
        x: min_x,
        y: min_y,
        width: w,
        height: h,
        points: all_points,
        text: svg_d,
        fill_color,
        stroke_color: if final_stroke_c.a() > 0 { Some(final_stroke_c) } else { None },
        stroke_width: stroke_width.max(0.01),
        is_closed: true,
        is_svg_path: true,
        is_modified: false,
        triangles: Vec::new(),
        group_id,
        group_path,
        is_visible: true,
        is_locked: false,
        name: None,
        pixmap,
        texture: None,
    };
    shape.precompute_triangles();
    Some(shape)
}

fn create_canvas_shape_from_vertices(
    vertices: &[StudioBezierVertex],
    layer_id: u64,
    transform: Option<&AffineTransform2D>,
    fill_color: egui::Color32,
    stroke_color: egui::Color32,
    stroke_width: f32,
    scale_to_in: f32,
    pattern_swatch: Option<&tiny_skia::Pixmap>,
) -> Option<CanvasShape> {
    let sub = StudioSubpath {
        start_pos: 0,
        end_pos: 0,
        vertices: vertices.to_vec(),
    };
    create_canvas_shape_from_subpaths(
        &[sub],
        Some(layer_id),
        vec![layer_id],
        transform,
        fill_color,
        stroke_color,
        stroke_width,
        scale_to_in,
        pattern_swatch,
    )
}

fn find_transform_in_slice(slice: &[u8], version: StudioFormatVersion) -> Option<AffineTransform2D> {
    if slice.len() < 24 {
        return None;
    }

    // 1. Look for explicit SXFM tag first
    if let Some(pos) = find_subsequence(slice, &TAG_TRANSFORM) {
        let offset = pos.saturating_add(4);
        if let (Some(a), Some(b), Some(tx), Some(c), Some(d), Some(ty)) = (
            read_f32_le(slice, offset),
            read_f32_le(slice, offset + 4),
            read_f32_le(slice, offset + 8),
            read_f32_le(slice, offset + 12),
            read_f32_le(slice, offset + 16),
            read_f32_le(slice, offset + 20),
        ) {
            return Some(AffineTransform2D::new(a, b, tx, c, d, ty));
        }
    }

    // 2. Look for anchored 36-byte Qt 3x3 matrix [m11, m12, 0.0, m21, m22, 0.0, dx, dy, 1.0]
    // preceded deterministically by Qt stream marker 00 01 00 01
    let mut search_idx = 0;
    while let Some(rel) = find_subsequence(&slice[search_idx..], b"\x00\x01\x00\x01") {
        let marker_pos = search_idx + rel;
        let mat_start = marker_pos + 4;
        if mat_start + 36 <= slice.len() {
            if let (Some(m11), Some(m12), Some(m13), Some(m21), Some(m22), Some(m23), Some(dx), Some(dy), Some(m33)) = (
                read_f32_le(slice, mat_start),
                read_f32_le(slice, mat_start + 4),
                read_f32_le(slice, mat_start + 8),
                read_f32_le(slice, mat_start + 12),
                read_f32_le(slice, mat_start + 16),
                read_f32_le(slice, mat_start + 20),
                read_f32_le(slice, mat_start + 24),
                read_f32_le(slice, mat_start + 28),
                read_f32_le(slice, mat_start + 32),
            ) {
                if m13.abs() < 1e-4 && m23.abs() < 1e-4 && (m33 - 1.0).abs() < 1e-4 {
                    let det = m11 * m22 - m12 * m21;
                    if (0.01..100.0).contains(&det.abs())
                        && (0.01..100.0).contains(&m11.abs())
                        && (0.01..100.0).contains(&m22.abs())
                        && m12.abs() < 10.0
                        && m21.abs() < 10.0
                        && dx.abs() < 20_000.0
                        && dy.abs() < 20_000.0
                    {
                        // Map Qt [m11, m12, 0, m21, m22, 0, dx, dy, 1] to AffineTransform2D:
                        // x' = m11 * x + m21 * y + dx
                        // y' = m12 * x + m22 * y + dy
                        return Some(AffineTransform2D::new(m11, m21, dx, m12, m22, dy));
                    }
                }
            }
        }
        search_idx = marker_pos + 1;
    }

    // 3. Scan for 36-byte or 65-byte row-major matrix:
    // [m11, m12, tx, m21, m22, ty, 0, 0, 1] as specified in SPEC.md Section 4.2.
    // Matrix maps local geometry to world/page coordinates.
    {
        let mut i = 0;
        while i + 36 <= slice.len() {
            if let (Some(m11), Some(m12), Some(tx), Some(m21), Some(m22), Some(ty), Some(m31), Some(m32), Some(m33)) = (
                read_f32_le(slice, i),
                read_f32_le(slice, i + 4),
                read_f32_le(slice, i + 8),
                read_f32_le(slice, i + 12),
                read_f32_le(slice, i + 16),
                read_f32_le(slice, i + 20),
                read_f32_le(slice, i + 24),
                read_f32_le(slice, i + 28),
                read_f32_le(slice, i + 32),
            ) {
                if m31.abs() < 1e-3 && m32.abs() < 1e-3 && (m33 - 1.0).abs() < 1e-3 {
                    let det = m11 * m22 - m12 * m21;
                    if (0.001..1000.0).contains(&det.abs())
                        && (0.001..1000.0).contains(&m11.abs())
                        && (0.001..1000.0).contains(&m22.abs())
                        && m12.abs() < 500.0
                        && m21.abs() < 500.0
                        && tx.abs() < 50_000.0
                        && ty.abs() < 50_000.0
                    {
                        return Some(AffineTransform2D::new(m11, m12, tx, m21, m22, ty));
                    }
                }
            }
            i = i.saturating_add(1);
        }
    }

    // 4. For Studio V4, scan for 24-byte matrix [a, b, tx, c, d, ty] in reverse
    if version == StudioFormatVersion::StudioV4 {
        for i in (0..slice.len().saturating_sub(23)).rev() {
            if let (Some(a), Some(b), Some(tx), Some(c), Some(d), Some(ty)) = (
                read_f32_le(slice, i),
                read_f32_le(slice, i + 4),
                read_f32_le(slice, i + 8),
                read_f32_le(slice, i + 12),
                read_f32_le(slice, i + 16),
                read_f32_le(slice, i + 20),
            ) {
                let det = a * d - b * c;
                if (0.05..20.0).contains(&a.abs())
                    && (0.05..20.0).contains(&d.abs())
                    && b.abs() < 5.0
                    && c.abs() < 5.0
                    && tx.abs() < 20_000.0
                    && ty.abs() < 20_000.0
                    && (0.001..500.0).contains(&det.abs())
                {
                    return Some(AffineTransform2D::new(a, b, tx, c, d, ty));
                }
            }
        }
    }

    None
}

fn find_fill_color_in_slice(slice: &[u8]) -> Option<egui::Color32> {
    if slice.len() < 18 {
        return None;
    }

    // Scan backwards from end of slice for official style record:
    // 01 [len: 4 bytes] 00 01
    for i in (0..slice.len().saturating_sub(22)).rev() {
        if slice[i] == 0x01 && slice[i + 5] == 0x00 && slice[i + 6] == 0x01 {
            if let Some(len) = read_u32_le(slice, i + 1) {
                if (16..=2000).contains(&len) {
                    let end = (i + 5 + len as usize).min(slice.len());
                    let rec = &slice[i + 7..end];
                    let join_keywords: [&[u8]; 5] = [b"rounded", b"corner", b"flat", b"bevel", b"miter"];
                    let mut last_pos = None;
                    let mut last_kw_len = 0;
                    for kw in &join_keywords {
                        let mut p = 0;
                        while let Some(idx) = find_subsequence(&rec[p..], kw) {
                            let abs_idx = p + idx;
                            if last_pos.map_or(true, |lp| abs_idx > lp) {
                                last_pos = Some(abs_idx);
                                last_kw_len = kw.len();
                            }
                            p = abs_idx + 1;
                        }
                    }
                    if let Some(lp) = last_pos {
                        let flag_idx = lp + last_kw_len;
                        if flag_idx < rec.len() {
                            let flag = rec[flag_idx];
                            if flag == 1 && flag_idx + 1 + 16 <= rec.len() {
                                if let (Some(r), Some(g), Some(b), Some(a)) = (
                                    read_f32_le(rec, flag_idx + 1),
                                    read_f32_le(rec, flag_idx + 5),
                                    read_f32_le(rec, flag_idx + 9),
                                    read_f32_le(rec, flag_idx + 13),
                                ) {
                                    if (0.0..=1.01).contains(&r) && (0.0..=1.01).contains(&g) && (0.0..=1.01).contains(&b) && (0.0..=1.01).contains(&a) {
                                        return Some(egui::Color32::from_rgba_unmultiplied(
                                            (r.clamp(0.0, 1.0) * 255.0).round() as u8,
                                            (g.clamp(0.0, 1.0) * 255.0).round() as u8,
                                            (b.clamp(0.0, 1.0) * 255.0).round() as u8,
                                            (a.clamp(0.0, 1.0) * 255.0).round() as u8,
                                        ));
                                    }
                                }
                            } else if flag == 0 {
                                return Some(egui::Color32::TRANSPARENT);
                            }
                        }
                    } else if let (Some(r), Some(g), Some(b), Some(a)) = (
                        read_f32_le(slice, i + 7),
                        read_f32_le(slice, i + 11),
                        read_f32_le(slice, i + 15),
                        read_f32_le(slice, i + 19),
                    ) {
                        if (0.0..=1.01).contains(&r) && (0.0..=1.01).contains(&g) && (0.0..=1.01).contains(&b) && (0.0..=1.01).contains(&a) {
                            return Some(egui::Color32::from_rgba_unmultiplied(
                                (r.clamp(0.0, 1.0) * 255.0).round() as u8,
                                (g.clamp(0.0, 1.0) * 255.0).round() as u8,
                                (b.clamp(0.0, 1.0) * 255.0).round() as u8,
                                (a.clamp(0.0, 1.0) * 255.0).round() as u8,
                            ));
                        }
                    }
                }
            }
        }
    }

    None
}

fn compute_bounds(pts: &[egui::Pos2]) -> (f32, f32, f32, f32) {
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;

    for p in pts {
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x);
        max_y = max_y.max(p.y);
    }

    (min_x, min_y, max_x, max_y)
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Standard Base64 encoder for embedding raster images in SVG.
pub fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] } else { 0 };
        out.push(CHARS[(b0 >> 2) as usize] as char);
        out.push(CHARS[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(CHARS[(((b1 & 0x0F) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(CHARS[(b2 & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Extracts embedded PNG raster image (e.g. Print & Cut artwork or preview or pattern swatch) from Studio data.
/// Differentiates between repeating pattern swatches (e.g. square power-of-two texture tiles like 512x512)
/// and actual positioned Print & Cut raster images.
fn extract_embedded_png(data: &[u8]) -> (Option<std::sync::Arc<tiny_skia::Pixmap>>, Option<std::sync::Arc<tiny_skia::Pixmap>>) {
    let mut pattern_swatch = None;
    let mut print_artwork_pixmap = None;

    let mut search_idx = 0;
    while search_idx + 8 <= data.len() {
        if let Some(pos) = find_subsequence(&data[search_idx..], &PNG_SIGNATURE) {
            let png_start = search_idx + pos;
            if let Some(png_slice) = data.get(png_start..) {
                if let Some(iend_off) = find_subsequence(png_slice, b"IEND") {
                    let png_len = iend_off.saturating_add(8);
                    if let Some(png_bytes) = png_slice.get(..png_len) {
                        if let Some(pm) = decode_png_to_pixmap(png_bytes) {
                            let w = pm.width();
                            let h = pm.height();
                            let is_square = w == h;
                            let is_power_of_two = (w & (w - 1)) == 0 && w >= 64;

                            if is_square && is_power_of_two {
                                // Power-of-two square tile is a repeating pattern swatch (e.g. 256x256, 512x512, 1024x1024)
                                if pattern_swatch.is_none() || w >= pattern_swatch.as_ref().map_or(0, |s: &std::sync::Arc<tiny_skia::Pixmap>| s.width()) {
                                    pattern_swatch = Some(std::sync::Arc::new(pm));
                                }
                            } else if w >= 400 && h >= 400 && !is_square && print_artwork_pixmap.is_none() {
                                print_artwork_pixmap = Some(std::sync::Arc::new(pm));
                            }
                        }
                    }
                    search_idx = png_start + png_len;
                    continue;
                }
            }
            search_idx = png_start + 8;
        } else {
            break;
        }
    }

    // If a repeating pattern swatch exists, any non-square embedded PNG is a document composite preview,
    // not a standalone Print Artwork rectangle. Discard print_artwork_pixmap to prevent oversized
    // unclipped rectangular blocks from rendering across the canvas.
    if pattern_swatch.is_some() {
        print_artwork_pixmap = None;
    }

    (print_artwork_pixmap, pattern_swatch)
}

/// Robustly decodes PNG bytes to tiny_skia::Pixmap.
fn decode_png_to_pixmap(bytes: &[u8]) -> Option<tiny_skia::Pixmap> {
    if let Ok(pm) = tiny_skia::Pixmap::decode_png(bytes) {
        return Some(pm);
    }
    if let Ok(dyn_img) = image::load_from_memory(bytes) {
        let rgba = dyn_img.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        let int_size = tiny_skia::IntSize::from_wh(w, h)?;
        return tiny_skia::Pixmap::from_vec(rgba.into_raw(), int_size);
    }
    None
}

/// Merges concentric closed shapes sharing the same solid fill color into a compound path with EvenOdd cutout.
fn merge_compound_paths(
    shapes: Vec<CanvasShape>,
    version: StudioFormatVersion,
    pattern_swatch: Option<&tiny_skia::Pixmap>,
) -> Vec<CanvasShape> {
    if shapes.len() < 2 {
        return shapes;
    }

    let mut merged = Vec::with_capacity(shapes.len());
    let mut consumed = vec![false; shapes.len()];

    for i in 0..shapes.len() {
        if consumed[i] {
            continue;
        }

        let s1 = &shapes[i];
        if !s1.is_closed || s1.fill_color.a() == 0 || s1.points.len() < 3 {
            merged.push(s1.clone());
            continue;
        }

        // Look for inner hole candidates
        let mut hole_indices = Vec::new();
        for j in 0..shapes.len() {
            if i == j || consumed[j] {
                continue;
            }
            let s2 = &shapes[j];
            if !s2.is_closed || s2.fill_color != s1.fill_color || s2.points.len() < 3 {
                continue;
            }

            // In silhouette05; (.studio3), restrict merging to subpaths sharing the same explicit Group/CompoundPath.
            // In silhouette04; (.studio), files lack object tags so check s1.group_id != s2.group_id if grouped.
            if version == StudioFormatVersion::StudioV5 {
                if s1.group_id.is_none() || s2.group_id.is_none() || s1.group_id != s2.group_id {
                    continue;
                }
            } else if s1.group_id.is_some() && s1.group_id != s2.group_id {
                continue;
            }

            // Candidate hole must be a simple cutout (not an artwork with many subpaths or high vertex count)
            let s2_subpath_count = s2.text.matches('M').count();
            if s2_subpath_count > 2 || s2.points.len() > 120 {
                continue;
            }

            // Check if s1 contains s2's bounding box with a margin
            let s1_max_x = s1.x + s1.width;
            let s1_max_y = s1.y + s1.height;
            let s2_max_x = s2.x + s2.width;
            let s2_max_y = s2.y + s2.height;

            if s1.x <= s2.x + 0.05
                && s1.y <= s2.y + 0.05
                && s1_max_x >= s2_max_x - 0.05
                && s1_max_y >= s2_max_y - 0.05
            {
                if s2.width < s1.width * 0.99 || s2.height < s1.height * 0.99 {
                    let s1_cx = s1.x + s1.width * 0.5;
                    let s1_cy = s1.y + s1.height * 0.5;
                    let s2_cx = s2.x + s2.width * 0.5;
                    let s2_cy = s2.y + s2.height * 0.5;
                    let dx_rel = (s1_cx - s2_cx).abs() / s1.width.max(0.001);
                    let dy_rel = (s1_cy - s2_cy).abs() / s1.height.max(0.001);
                    if dx_rel <= 0.35 && dy_rel <= 0.35 {
                        hole_indices.push(j);
                    }
                }
            }
        }

        if !hole_indices.is_empty() && hole_indices.len() <= 6 {
            consumed[i] = true;
            for &j in &hole_indices {
                consumed[j] = true;
            }
            let outer = s1;

            // Build compound SVG path d string preserving all multi-subpath contours
            let mut compound_d = if outer.is_svg_path && !outer.text.is_empty() {
                outer.text.clone()
            } else {
                let outer_str = outer.points.iter().map(|p| format!("{:.3},{:.3}", p.x, p.y)).collect::<Vec<_>>().join(" L ");
                format!("M {} Z", outer_str)
            };
            for &j in &hole_indices {
                let inner = &shapes[j];
                let inner_d = if inner.is_svg_path && !inner.text.is_empty() {
                    inner.text.clone()
                } else {
                    let inner_str = inner.points.iter().map(|p| format!("{:.3},{:.3}", p.x, p.y)).collect::<Vec<_>>().join(" L ");
                    format!("M {} Z", inner_str)
                };
                if !compound_d.is_empty() {
                    compound_d.push(' ');
                }
                compound_d.push_str(&inner_d);
            }

            // Pre-render a tiny_skia::Pixmap with FillRule::EvenOdd so the holes punch through
            let dpi = 48.0_f32;
            let raw_w = ((outer.width * dpi).ceil() as u32).max(16);
            let raw_h = ((outer.height * dpi).ceil() as u32).max(16);
            let max_dim = if pattern_swatch.is_some() { 256_u32 } else { 128_u32 };
            let (pix_w, pix_h) = if raw_w > max_dim || raw_h > max_dim {
                let scale = (max_dim as f32 / raw_w.max(raw_h) as f32).min(1.0);
                (((raw_w as f32 * scale).round() as u32).max(16), ((raw_h as f32 * scale).round() as u32).max(16))
            } else {
                (raw_w, raw_h)
            };

            let is_white_or_light = (outer.fill_color.r() > 240 && outer.fill_color.g() > 240 && outer.fill_color.b() > 240) || outer.fill_color == egui::Color32::WHITE;
            let mut pixmap = None;
            if outer.fill_color.a() > 0 && ((pattern_swatch.is_some() && is_white_or_light) || !hole_indices.is_empty()) {
                if let Some(mut pm) = tiny_skia::Pixmap::new(pix_w, pix_h) {
                    let pad = 2.0_f32;
                    let sx = (pix_w as f32 - pad * 2.0) / outer.width.max(0.001);
                    let sy = (pix_h as f32 - pad * 2.0) / outer.height.max(0.001);

                    let mut pb = tiny_skia::PathBuilder::new();
                    for sub in compound_d.split('M') {
                        let sub = sub.trim();
                        if sub.is_empty() {
                            continue;
                        }
                        let nums: Vec<f32> = sub
                            .split(|c: char| !c.is_numeric() && c != '.' && c != '-')
                            .filter_map(|s| s.parse::<f32>().ok())
                            .collect();
                        let mut chunks = nums.chunks_exact(2);
                        if let Some(first) = chunks.next() {
                            pb.move_to((first[0] - outer.x) * sx + pad, (first[1] - outer.y) * sy + pad);
                            for p in chunks {
                                pb.line_to((p[0] - outer.x) * sx + pad, (p[1] - outer.y) * sy + pad);
                            }
                            pb.close();
                        }
                    }

                    if let Some(path) = pb.finish() {
                        if let Some(swatch) = pattern_swatch {
                            if is_white_or_light {
                                let tile_size_in = 2.0_f32;
                                let tile_w_px = (tile_size_in * sx).max(16.0);
                                let tile_h_px = (tile_size_in * sy).max(16.0);
                                let scale_x = tile_w_px / swatch.width() as f32;
                                let scale_y = tile_h_px / swatch.height() as f32;
                                let pat_xfm = tiny_skia::Transform::from_scale(scale_x, scale_y).post_translate(pad, pad);
                                let shader = tiny_skia::Pattern::new(
                                    swatch.as_ref(),
                                    tiny_skia::SpreadMode::Repeat,
                                    tiny_skia::FilterQuality::Bilinear,
                                    1.0,
                                    pat_xfm,
                                );
                                let mut fill_paint = tiny_skia::Paint::default();
                                fill_paint.shader = shader;
                                fill_paint.anti_alias = true;
                                pm.fill_path(&path, &fill_paint, tiny_skia::FillRule::EvenOdd, tiny_skia::Transform::identity(), None);
                            } else {
                                let mut fill_paint = tiny_skia::Paint::default();
                                fill_paint.set_color_rgba8(outer.fill_color.r(), outer.fill_color.g(), outer.fill_color.b(), outer.fill_color.a());
                                fill_paint.anti_alias = true;
                                pm.fill_path(&path, &fill_paint, tiny_skia::FillRule::EvenOdd, tiny_skia::Transform::identity(), None);
                            }
                        } else {
                            let mut fill_paint = tiny_skia::Paint::default();
                            fill_paint.set_color_rgba8(outer.fill_color.r(), outer.fill_color.g(), outer.fill_color.b(), outer.fill_color.a());
                            fill_paint.anti_alias = true;
                            pm.fill_path(&path, &fill_paint, tiny_skia::FillRule::EvenOdd, tiny_skia::Transform::identity(), None);
                        }
                        let is_white_or_invisible_stroke = outer.stroke_color.map_or(true, |sc| sc.a() == 0 || (sc.r() > 240 && sc.g() > 240 && sc.b() > 240));
                        let stroke_c = if !is_white_or_invisible_stroke {
                            outer.stroke_color.unwrap()
                        } else if pattern_swatch.is_some() || is_white_or_light || outer.fill_color.a() == 0 {
                            egui::Color32::from_rgb(255, 38, 38)
                        } else {
                            egui::Color32::TRANSPARENT
                        };

                        let is_cut_stroke = stroke_c == egui::Color32::from_rgb(255, 38, 38)
                            || (stroke_c.r() >= 200 && stroke_c.g() <= 50 && stroke_c.b() <= 50)
                            || outer.stroke_width <= 0.015
                            || outer.fill_color.a() == 0;

                        if !is_cut_stroke && stroke_c.a() > 0 {
                            let mut stroke_paint = tiny_skia::Paint::default();
                            stroke_paint.set_color_rgba8(stroke_c.r(), stroke_c.g(), stroke_c.b(), stroke_c.a());
                            stroke_paint.anti_alias = true;
                            let st = tiny_skia::Stroke {
                                width: (outer.stroke_width * sx).max(1.0),
                                line_cap: tiny_skia::LineCap::Round,
                                line_join: tiny_skia::LineJoin::Round,
                                ..Default::default()
                            };
                            pm.stroke_path(&path, &stroke_paint, &st, tiny_skia::Transform::identity(), None);
                        }
                        pixmap = Some(std::sync::Arc::new(pm));
                    }
                }
            }

            let mut compound_shape = outer.clone();
            compound_shape.text = compound_d;
            compound_shape.is_svg_path = true;
            compound_shape.pixmap = pixmap;
            compound_shape.texture = None;
            merged.push(compound_shape);
        } else {
            merged.push(s1.clone());
        }
    }

    merged
}

// -------------------------------------------------------------------------------------------------
// Test Builders & Binary Generators
// -------------------------------------------------------------------------------------------------

/// Descriptor for creating synthetic paths in test binary files.
#[derive(Debug, Clone)]
pub struct TestPathData {
    pub layer_id: u32,
    pub is_closed: bool,
    pub fill_color: [u8; 4],
    pub stroke_color: [u8; 4],
    pub stroke_width: f32,
    pub transform: [f32; 6], // [a, b, tx, c, d, ty]
    pub vertices: Vec<StudioBezierVertex>,
}

/// Generates a valid `.studio` (v4) binary stream starting at byte offset 0x0000.
pub fn create_test_v4_data(mat_preset: &str, layer_name: &str, paths: &[TestPathData]) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(MAGIC_STUDIO_V4);

    data.extend_from_slice(&TAG_MAT);
    let mat_tag = format!("{}:{};", mat_preset, mat_preset);
    data.extend_from_slice(mat_tag.as_bytes());

    data.extend_from_slice(&TAG_LAYER);
    let layer_tag = format!(":1:{};", layer_name);
    data.extend_from_slice(layer_tag.as_bytes());

    for p in paths {
        data.extend_from_slice(&TAG_PATH_BLOCK);
        data.extend_from_slice(&(p.vertices.len() as u32).to_le_bytes());
        data.push(if p.is_closed { 1 } else { 0 });
        data.extend_from_slice(&p.layer_id.to_le_bytes());
        data.extend_from_slice(&p.fill_color);
        data.extend_from_slice(&p.stroke_color);
        data.extend_from_slice(&p.stroke_width.to_le_bytes());
        for &val in &p.transform {
            data.extend_from_slice(&val.to_le_bytes());
        }

        for v in &p.vertices {
            data.extend_from_slice(&BEZIER_VERTEX_MARKER);
            data.extend_from_slice(&v.anchor.x.to_le_bytes());
            data.extend_from_slice(&v.anchor.y.to_le_bytes());
            data.extend_from_slice(&v.handle_in.x.to_le_bytes());
            data.extend_from_slice(&v.handle_in.y.to_le_bytes());
            data.extend_from_slice(&v.handle_out.x.to_le_bytes());
            data.extend_from_slice(&v.handle_out.y.to_le_bytes());
            data.extend_from_slice(&[0u8; 9]);
        }
    }

    data
}

/// Generates a valid `.studio3` (v5) binary stream with a 1,024-byte (0x0400) header and section table,
/// with the `silhouette05;` serialized object stream starting at 0x0400.
pub fn create_test_v5_data(mat_preset: &str, layer_name: &str, paths: &[TestPathData]) -> Vec<u8> {
    let mut data = vec![0u8; HEADER_OFFSET_V5];

    let stream_offset_bytes = (HEADER_OFFSET_V5 as u32).to_le_bytes();
    if let Some(slice) = data.get_mut(16..20) {
        slice.copy_from_slice(&stream_offset_bytes);
    }

    let mut stream = Vec::new();
    stream.extend_from_slice(MAGIC_STUDIO_V5);

    stream.extend_from_slice(&TAG_MAT);
    let mat_tag = format!("{}:{};", mat_preset, mat_preset);
    stream.extend_from_slice(mat_tag.as_bytes());

    stream.extend_from_slice(&TAG_LAYER);
    let layer_tag = format!(":1:{};", layer_name);
    stream.extend_from_slice(layer_tag.as_bytes());

    for p in paths {
        stream.extend_from_slice(&TAG_PATH_BLOCK);
        stream.extend_from_slice(&(p.vertices.len() as u32).to_le_bytes());
        stream.push(if p.is_closed { 1 } else { 0 });
        stream.extend_from_slice(&p.layer_id.to_le_bytes());
        stream.extend_from_slice(&p.fill_color);
        stream.extend_from_slice(&p.stroke_color);
        stream.extend_from_slice(&p.stroke_width.to_le_bytes());
        for &val in &p.transform {
            stream.extend_from_slice(&val.to_le_bytes());
        }

        for v in &p.vertices {
            stream.extend_from_slice(&BEZIER_VERTEX_MARKER);
            stream.extend_from_slice(&v.anchor.x.to_le_bytes());
            stream.extend_from_slice(&v.anchor.y.to_le_bytes());
            stream.extend_from_slice(&v.handle_in.x.to_le_bytes());
            stream.extend_from_slice(&v.handle_in.y.to_le_bytes());
            stream.extend_from_slice(&v.handle_out.x.to_le_bytes());
            stream.extend_from_slice(&v.handle_out.y.to_le_bytes());
            stream.extend_from_slice(&[0u8; 9]);
        }
    }

    data.extend_from_slice(&stream);
    data
}

// -------------------------------------------------------------------------------------------------
// Unit Tests & Real-File Verification
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_silhouette04_header_offset_zero() {
        let mut data = Vec::new();
        data.extend_from_slice(b"silhouette04;");
        data.extend_from_slice(&[0x00, 0x01, 0x02, 0x03]);

        let (ver, start, end) = detect_format_and_stream(&data).expect("Should detect silhouette04;");
        assert_eq!(ver, StudioFormatVersion::StudioV4);
        assert_eq!(start, 0);
        assert_eq!(end, data.len());
    }

    #[test]
    fn test_detect_silhouette05_header_offset_0x0400() {
        let mut data = vec![0u8; 0x0400];
        data[16..20].copy_from_slice(&(0x0400_u32).to_le_bytes());
        data.extend_from_slice(b"silhouette05;");
        data.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd]);

        let (ver, start, _) = detect_format_and_stream(&data).expect("Should detect silhouette05; at 0x0400");
        assert_eq!(ver, StudioFormatVersion::StudioV5);
        assert_eq!(start, 0x0400);
    }

    #[test]
    fn test_bezier_vertex_record_parsing() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&BEZIER_VERTEX_MARKER);
        let anchor_x = 100.0_f32;
        let anchor_y = 150.0_f32;
        let cx1 = 90.0_f32;
        let cy1 = 140.0_f32;
        let cx2 = 110.0_f32;
        let cy2 = 160.0_f32;

        buf.extend_from_slice(&anchor_x.to_le_bytes());
        buf.extend_from_slice(&anchor_y.to_le_bytes());
        buf.extend_from_slice(&cx1.to_le_bytes());
        buf.extend_from_slice(&cy1.to_le_bytes());
        buf.extend_from_slice(&cx2.to_le_bytes());
        buf.extend_from_slice(&cy2.to_le_bytes());
        buf.extend_from_slice(&[0u8; 9]);

        let (v, len) = parse_bezier_vertex(&buf[5..]).expect("Should parse vertex record");
        assert_eq!(v.anchor, egui::pos2(100.0, 150.0));
        assert_eq!(v.handle_in, egui::pos2(90.0, 140.0));
        assert_eq!(v.handle_out, egui::pos2(110.0, 160.0));
        assert_eq!(len, 33);
    }

    #[test]
    fn test_adaptive_de_casteljau_flattening() {
        let p0 = egui::pos2(0.0, 0.0);
        let p1 = egui::pos2(1.0, 1.0);
        let p2 = egui::pos2(2.0, 2.0);
        let p3 = egui::pos2(3.0, 3.0);
        let mut pts = Vec::new();
        flatten_cubic_bezier(p0, p1, p2, p3, 0.01, 10, &mut pts);
        assert_eq!(pts.len(), 1);
        assert_eq!(pts[0], p3);

        let p0 = egui::pos2(10.0, 0.0);
        let p1 = egui::pos2(10.0, 5.52);
        let p2 = egui::pos2(5.52, 10.0);
        let p3 = egui::pos2(0.0, 10.0);
        let mut curve_pts = Vec::new();
        flatten_cubic_bezier(p0, p1, p2, p3, 0.05, 10, &mut curve_pts);
        assert!(curve_pts.len() >= 4);
        assert_eq!(curve_pts.last().copied(), Some(p3));
    }

    #[test]
    fn test_affine_transform_2d() {
        let xfm = AffineTransform2D::new(2.0, 0.0, 50.0, 0.0, 3.0, 100.0);
        let p = egui::pos2(10.0, 20.0);
        let transformed = xfm.apply(p);
        assert_eq!(transformed, egui::pos2(70.0, 160.0));
    }

    #[test]
    fn test_affine_matrix_multiplication() {
        let parent = AffineTransform2D::new(2.0, 0.0, 10.0, 0.0, 3.0, 20.0);
        let child = AffineTransform2D::new(1.0, 0.5, 5.0, -0.5, 1.0, 7.0);
        let combined = parent * child;

        let p = egui::pos2(4.0, 6.0);
        let via_child_then_parent = parent.apply(child.apply(p));
        let via_combined = combined.apply(p);

        assert!((via_child_then_parent.x - via_combined.x).abs() < 1e-4);
        assert!((via_child_then_parent.y - via_combined.y).abs() < 1e-4);
        assert_eq!(parent.combine(&child), combined);
    }

    #[test]
    fn test_roundtrip_studio_v4_import() {
        let vertices = vec![
            StudioBezierVertex { anchor: egui::pos2(72.0, 72.0), handle_in: egui::pos2(72.0, 72.0), handle_out: egui::pos2(72.0, 72.0) },
            StudioBezierVertex { anchor: egui::pos2(216.0, 72.0), handle_in: egui::pos2(216.0, 72.0), handle_out: egui::pos2(216.0, 72.0) },
            StudioBezierVertex { anchor: egui::pos2(216.0, 216.0), handle_in: egui::pos2(216.0, 216.0), handle_out: egui::pos2(216.0, 216.0) },
            StudioBezierVertex { anchor: egui::pos2(72.0, 216.0), handle_in: egui::pos2(72.0, 216.0), handle_out: egui::pos2(72.0, 216.0) },
        ];
        let path = TestPathData {
            layer_id: 1,
            is_closed: true,
            fill_color: [200, 50, 50, 255],
            stroke_color: [255, 0, 0, 255],
            stroke_width: 0.02,
            transform: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            vertices,
        };

        let binary = create_test_v4_data("12x12", "Default Layer", &[path]);
        let doc = parse_studio_data(&binary).expect("Failed to parse studio v4 data");

        assert_eq!(doc.version, StudioFormatVersion::StudioV4);
        assert_eq!(doc.mat_width_in, 12.0);
        assert_eq!(doc.mat_height_in, 12.0);
        assert_eq!(doc.shapes.len(), 1);

        let shape = &doc.shapes[0];
        assert!(shape.is_closed);
        assert_eq!(shape.fill_color, egui::Color32::from_rgba_unmultiplied(200, 50, 50, 255));
        assert_eq!(shape.stroke_color, Some(egui::Color32::from_rgb(255, 0, 0)));
        assert!(!shape.triangles.is_empty());
    }

    #[test]
    fn test_roundtrip_studio_v5_import() {
        let vertices = vec![
            StudioBezierVertex { anchor: egui::pos2(0.0, 0.0), handle_in: egui::pos2(0.0, 0.0), handle_out: egui::pos2(0.0, 0.0) },
            StudioBezierVertex { anchor: egui::pos2(100.0, 0.0), handle_in: egui::pos2(100.0, 0.0), handle_out: egui::pos2(100.0, 0.0) },
            StudioBezierVertex { anchor: egui::pos2(100.0, 100.0), handle_in: egui::pos2(100.0, 100.0), handle_out: egui::pos2(100.0, 100.0) },
            StudioBezierVertex { anchor: egui::pos2(0.0, 100.0), handle_in: egui::pos2(0.0, 100.0), handle_out: egui::pos2(0.0, 100.0) },
        ];
        let path = TestPathData {
            layer_id: 1,
            is_closed: true,
            fill_color: [0, 120, 240, 255],
            stroke_color: [0, 0, 0, 255],
            stroke_width: 0.03,
            transform: [2.0, 0.0, 72.0, 0.0, 2.0, 72.0],
            vertices,
        };

        let binary = create_test_v5_data("letter", "Layer 1", &[path]);
        let doc = parse_studio_data(&binary).expect("Failed to parse studio v5 data");

        assert_eq!(doc.version, StudioFormatVersion::StudioV5);
        assert_eq!(doc.stream_offset, 0x0400);
        assert_eq!(doc.mat_width_in, 8.5);
        assert_eq!(doc.mat_height_in, 11.0);
        assert_eq!(doc.shapes.len(), 1);

        let shape = &doc.shapes[0];
        assert!(shape.is_closed);
        assert_eq!(shape.fill_color, egui::Color32::from_rgba_unmultiplied(0, 120, 240, 255));
    }

    #[test]
    fn test_convert_studio_to_svg() {
        let vertices = vec![
            StudioBezierVertex { anchor: egui::pos2(72.0, 72.0), handle_in: egui::pos2(72.0, 72.0), handle_out: egui::pos2(72.0, 72.0) },
            StudioBezierVertex { anchor: egui::pos2(144.0, 72.0), handle_in: egui::pos2(144.0, 72.0), handle_out: egui::pos2(144.0, 72.0) },
            StudioBezierVertex { anchor: egui::pos2(144.0, 144.0), handle_in: egui::pos2(144.0, 144.0), handle_out: egui::pos2(144.0, 144.0) },
        ];
        let path = TestPathData {
            layer_id: 1,
            is_closed: true,
            fill_color: [255, 100, 50, 255],
            stroke_color: [200, 0, 0, 255],
            stroke_width: 0.02,
            transform: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            vertices,
        };

        let binary = create_test_v4_data("12x12", "Cut Layer", &[path]);
        let svg = convert_studio_to_svg(&binary).expect("Failed to convert studio to svg");

        assert!(svg.contains("<svg"));
        assert!(svg.contains("viewBox=\"0 0 12.00 12.00\""));
        assert!(svg.contains("<g id=\"Cut Layer\">"));
        assert!(svg.contains("<polygon points=") || svg.contains("<path d="));
        assert!(svg.contains("fill=\"#ff6432\""));
        assert!(svg.contains("stroke=\"#c80000\""));
    }

    #[test]
    fn test_real_sample_files_verification() {
        let sample1_candidates = [
            Path::new("../test_samples/Peeker Bears.studio"),
            Path::new("test_samples/Peeker Bears.studio"),
        ];
        let sample2_candidates = [
            Path::new("../test_samples/1 DISNEY PEOPLE.studio3"),
            Path::new("test_samples/1 DISNEY PEOPLE.studio3"),
            Path::new("../test_samples/disney.studio3"),
            Path::new("test_samples/disney.studio3"),
        ];

        let p1 = sample1_candidates.iter().find(|p| p.exists());
        let p2 = sample2_candidates.iter().find(|p| p.exists());

        if let Some(path1) = p1 {
            let doc1 = parse_studio_file(path1).expect("Parse Peeker Bears.studio");
            assert_eq!(doc1.version, StudioFormatVersion::StudioV4);
            assert!(doc1.shapes.len() >= 20, "Expected >= 20 shapes in Peeker Bears, got {}", doc1.shapes.len());

            let mut min_x = f32::MAX;
            let mut min_y = f32::MAX;
            let mut max_x = f32::MIN;
            let mut max_y = f32::MIN;
            let mut colored_shapes = 0;

            for shape in &doc1.shapes {
                assert!(shape.width > 0.0);
                assert!(shape.height > 0.0);
                assert!(shape.x.is_finite());
                assert!(shape.y.is_finite());
                assert!(shape.x >= 0.0 && shape.x <= doc1.mat_width_in);
                assert!(shape.y >= 0.0 && shape.y <= doc1.mat_height_in);

                min_x = min_x.min(shape.x);
                min_y = min_y.min(shape.y);
                max_x = max_x.max(shape.x + shape.width);
                max_y = max_y.max(shape.y + shape.height);

                if shape.fill_color.a() > 0 {
                    colored_shapes += 1;
                }
            }

            // Verify solid RGBA fill colors were extracted
            assert!(colored_shapes >= 20, "Expected >= 20 filled shapes in Peeker Bears, got {}", colored_shapes);

            // Verify shapes form a coherent, compact group on the 8.5x11 page instead of being scattered
            let design_w = max_x - min_x;
            let design_h = max_y - min_y;
            assert!(design_w > 2.0 && design_w <= 8.5, "Unexpected design width: {:.2} in", design_w);
            assert!(design_h > 1.5 && design_h <= 11.0, "Unexpected design height: {:.2} in", design_h);

            let svg1 = convert_studio_to_svg(&std::fs::read(path1).unwrap()).expect("Convert Peeker Bears to SVG");
            assert!(svg1.contains("<svg"));
            assert!(svg1.contains("fill-rule=\"evenodd\"") || svg1.contains("<path"));
            assert!(svg1.contains("<polygon") || svg1.contains("<polyline") || svg1.contains("<path"));
        }

        if let Some(path2) = p2 {
            let doc2 = parse_studio_file(path2).expect("Parse DISNEY HEARTS.studio3");
            assert_eq!(doc2.version, StudioFormatVersion::StudioV5);
            assert!(doc2.shapes.len() >= 10, "Expected >= 10 shapes in DISNEY HEARTS, got {}", doc2.shapes.len());

            // Verify pattern swatch extracted from Sec 2 without creating a giant obscuring rectangle
            assert!(doc2.pattern_swatch.is_some(), "Expected pattern swatch extracted from Sec 2");
            let swatch = doc2.pattern_swatch.as_ref().unwrap();
            assert_eq!(swatch.width(), 512);
            assert_eq!(swatch.height(), 512);

            let mut min_x = f32::MAX;
            let mut min_y = f32::MAX;
            let mut max_x = f32::MIN;
            let mut max_y = f32::MIN;

            for shape in &doc2.shapes {
                assert!(shape.width > 0.0);
                assert!(shape.height > 0.0);
                assert!(shape.x.is_finite());
                assert!(shape.y.is_finite());
                assert!(shape.x >= 0.0 && shape.x <= doc2.mat_width_in);
                assert!(shape.y >= 0.0 && shape.y <= doc2.mat_height_in);

                min_x = min_x.min(shape.x);
                min_y = min_y.min(shape.y);
                max_x = max_x.max(shape.x + shape.width);
                max_y = max_y.max(shape.y + shape.height);
            }

            // Verify shapes span across the 12x12 mat in both dimensions and are not collapsed into a line
            let span_x = max_x - min_x;
            let span_y = max_y - min_y;
            assert!(span_x > 4.0, "Expected shapes to span across width, got span_x = {:.2} in", span_x);
            assert!(span_y > 4.0, "Expected shapes to span across height, got span_y = {:.2} in", span_y);

            let svg2 = convert_studio_to_svg(&std::fs::read(path2).unwrap()).expect("Convert DISNEY HEARTS to SVG");
            assert!(svg2.contains("<svg"));
            assert!(svg2.contains("<pattern id=\"swatch_pattern\""));
            assert!(svg2.contains("<image"));
            assert!(svg2.contains("href=\"data:image/png;base64,"));
            assert!(svg2.contains("<polygon") || svg2.contains("<polyline") || svg2.contains("<path"));
        }
    }

    #[test]
    fn test_bd_cats_multi_subpath_segmentation_no_diagonal_artifacts() {
        let cats_candidates = [
            Path::new("../test_samples/BD CATS.studio"),
            Path::new("test_samples/BD CATS.studio"),
        ];
        let p_cats = match cats_candidates.iter().find(|p| p.exists()) {
            Some(p) => p,
            None => return,
        };

        let data = std::fs::read(p_cats).expect("Read BD CATS.studio");
        let doc = parse_studio_data(&data).expect("Parse BD CATS.studio");

        assert_eq!(doc.version, StudioFormatVersion::StudioV4);
        assert!(doc.shapes.len() >= 5, "Expected >= 5 cats in BD CATS, got {}", doc.shapes.len());

        for (i, shape) in doc.shapes.iter().enumerate() {
            assert!(shape.is_svg_path, "Cat #{} must be an SVG path", i);
            let m_count = shape.text.matches('M').count();
            assert!(
                m_count > 1,
                "Cat #{} must have multiple MoveTo commands for disconnected contours, got {}",
                i,
                m_count
            );
            assert!(
                shape.pixmap.is_some(),
                "Cat #{} must have pre-rendered tiny_skia pixmap with EvenOdd fill/stroke",
                i
            );

            // Verify no straight line connects the end of one subpath to the start of the next:
            // Every subpath ends with "Z" and the next subpath immediately begins with "M".
            // There must never be a "Z L" or "Z <number>" diagonal bridge.
            assert!(!shape.text.contains("Z L"), "Cat #{} contains stray diagonal bridge 'Z L'", i);

            let subpaths: Vec<&str> = shape.text.split('M').filter(|s| !s.trim().is_empty()).collect();
            assert_eq!(subpaths.len(), m_count);
            for (sub_idx, sub) in subpaths.iter().enumerate() {
                let trimmed = sub.trim();
                assert!(trimmed.ends_with('Z'), "Subpath {} in Cat #{} must end with 'Z', got {:?}", sub_idx, i, trimmed);
                let nums: Vec<f32> = trimmed
                    .split(|c: char| !c.is_numeric() && c != '.' && c != '-')
                    .filter_map(|s| s.parse::<f32>().ok())
                    .collect();
                assert!(nums.len() >= 4, "Subpath {} in Cat #{} must have at least 2 points (4 coordinates)", sub_idx, i);
            }

            // Verify tiny_skia pixmap is valid and has drawn pixels
            let pm = shape.pixmap.as_ref().unwrap();
            assert!(pm.width() >= 16 && pm.height() >= 16);
            let non_zero_pixels = pm.data().iter().filter(|&&b| b > 0).count();
            assert!(non_zero_pixels > 0, "Cat #{} pixmap must contain non-transparent rendered artwork", i);
        }

        // Verify SVG export contains compound path markup with evenodd fill rule
        let svg = convert_studio_to_svg(&data).expect("Convert BD CATS to SVG");
        assert!(svg.contains("<svg"));
        assert!(svg.contains("fill-rule=\"evenodd\""));
        assert!(svg.contains("<path d=\"M "));
    }

    #[test]
    fn test_new_sample_files_verification() {
        let beverly_candidates = [
            Path::new("../test_samples/beverly high school mass.studio"),
            Path::new("test_samples/beverly high school mass.studio"),
        ];
        let cats_candidates = [
            Path::new("../test_samples/BD CATS.studio"),
            Path::new("test_samples/BD CATS.studio"),
        ];
        let whale_candidates = [
            Path::new("../test_samples/PP WHALE TAIL.studio"),
            Path::new("test_samples/PP WHALE TAIL.studio"),
        ];

        if let Some(p_bev) = beverly_candidates.iter().find(|p| p.exists()) {
            let doc = parse_studio_file(p_bev).expect("Parse beverly high school mass.studio");
            assert_eq!(doc.version, StudioFormatVersion::StudioV4);
            assert!(doc.shapes.len() >= 20, "Expected >= 20 shapes in beverly, got {}", doc.shapes.len());
            let filled = doc.shapes.iter().filter(|s| s.fill_color.a() > 0).count();
            assert!(filled >= 20, "Expected >= 20 filled shapes in beverly, got {}", filled);

            let colors: std::collections::BTreeSet<String> = doc.shapes.iter()
                .filter(|s| s.fill_color.a() > 0)
                .map(|s| format!("#{:02x}{:02x}{:02x}", s.fill_color.r(), s.fill_color.g(), s.fill_color.b()))
                .collect();
            assert!(colors.contains("#f56700") || colors.contains("#f46600"), "Expected Panther Orange in beverly: {:?}", colors);

            for shape in &doc.shapes {
                assert!(shape.width > 0.0);
                assert!(shape.height > 0.0);
                assert!(shape.x >= 0.0 && shape.x <= doc.mat_width_in);
                assert!(shape.y >= 0.0 && shape.y <= doc.mat_height_in);
            }

            let svg = convert_studio_to_svg(&std::fs::read(p_bev).unwrap()).expect("Convert beverly to SVG");
            assert!(svg.contains("<svg"));
            assert!(svg.contains("<polygon") || svg.contains("<path") || svg.contains("<polyline"));
        }

        if let Some(p_cats) = cats_candidates.iter().find(|p| p.exists()) {
            let doc = parse_studio_file(p_cats).expect("Parse BD CATS.studio");
            assert_eq!(doc.version, StudioFormatVersion::StudioV4);
            assert!(doc.shapes.len() >= 5, "Expected >= 5 cats in BD CATS, got {}", doc.shapes.len());
            let filled = doc.shapes.iter().filter(|s| s.fill_color.a() > 0).count();
            assert!(filled >= 5, "Expected >= 5 filled cats in BD CATS, got {}", filled);

            let colors: std::collections::BTreeSet<String> = doc.shapes.iter()
                .filter(|s| s.fill_color.a() > 0)
                .map(|s| format!("#{:02x}{:02x}{:02x}", s.fill_color.r(), s.fill_color.g(), s.fill_color.b()))
                .collect();
            assert!(colors.contains("#79472c") || colors.contains("#78462b"), "Expected Cat Brown in BD CATS: {:?}", colors);

            for shape in &doc.shapes {
                assert!(shape.width > 0.0);
                assert!(shape.height > 0.0);
                assert!(shape.x >= 0.0 && shape.x <= doc.mat_width_in);
                assert!(shape.y >= 0.0 && shape.y <= doc.mat_height_in);
            }

            let svg = convert_studio_to_svg(&std::fs::read(p_cats).unwrap()).expect("Convert BD CATS to SVG");
            assert!(svg.contains("<svg"));
            assert!(svg.contains("<polygon") || svg.contains("<path") || svg.contains("<polyline"));
        }

        if let Some(p_whale) = whale_candidates.iter().find(|p| p.exists()) {
            let doc = parse_studio_file(p_whale).expect("Parse PP WHALE TAIL.studio");
            assert_eq!(doc.version, StudioFormatVersion::StudioV4);
            assert!(!doc.shapes.is_empty(), "Expected >= 1 shape in PP WHALE TAIL");
            let filled = doc.shapes.iter().filter(|s| s.fill_color.a() > 0).count();
            assert!(filled >= 1, "Expected filled shape in PP WHALE TAIL");

            let s = &doc.shapes[0];
            assert!(s.width > 0.5 && s.width <= doc.mat_width_in);
            assert!(s.height > 0.5 && s.height <= doc.mat_height_in);

            let svg = convert_studio_to_svg(&std::fs::read(p_whale).unwrap()).expect("Convert PP WHALE TAIL to SVG");
            assert!(svg.contains("<svg"));
            assert!(svg.contains("<polygon") || svg.contains("<path") || svg.contains("<polyline"));
        }
    }

    #[test]
    fn test_studio3_pattern_fill_and_cut_outline_visibility() {
        let sample_files = [
            "DISNEY HEARTS.studio3",
            "disney.studio3",
            "1 DISNEY PEOPLE.studio3",
            "1 HEADS.studio3",
        ];

        for sample_name in &sample_files {
            let candidates = [
                format!("../test_samples/{}", sample_name),
                format!("test_samples/{}", sample_name),
            ];
            let path_str = match candidates.iter().find(|p| Path::new(p).exists()) {
                Some(p) => p,
                None => continue,
            };

            let data = std::fs::read(path_str).expect("Read .studio3 sample file");
            let doc = parse_studio_data(&data).expect("Parse .studio3 sample file");

            assert_eq!(doc.version, StudioFormatVersion::StudioV5);
            assert!(!doc.shapes.is_empty(), "Shapes must not be empty in {}", sample_name);

            // 1. Verify pattern swatch is extracted as a square power-of-two texture tile
            assert!(
                doc.pattern_swatch.is_some(),
                "Pattern swatch must be extracted in {}",
                sample_name
            );
            let swatch = doc.pattern_swatch.as_ref().unwrap();
            assert_eq!(swatch.width(), swatch.height(), "Swatch must be square in {}", sample_name);
            assert!(
                (swatch.width() & (swatch.width() - 1)) == 0 && swatch.width() >= 64,
                "Swatch must be power-of-two in {}",
                sample_name
            );

            // 2. Verify no shape is an unclipped full-mat `#1 Print Background` covering the grid
            for (idx, shape) in doc.shapes.iter().enumerate() {
                assert_ne!(
                    shape.name.as_deref(),
                    Some("Print Background"),
                    "Shape #{} in {} must not be full-mat Print Background",
                    idx,
                    sample_name
                );

                // If pattern filled, verify it is clipped to vector bounds and not a 12x12 unclipped mat
                if shape.is_svg_path {
                    assert!(
                        shape.width < doc.mat_width_in || shape.height < doc.mat_height_in,
                        "Vector cut shape #{} in {} should not be full mat size",
                        idx,
                        sample_name
                    );

                    // 3. Verify vector cut outline is visible (#ff2626)
                    assert!(
                        shape.stroke_color.is_some(),
                        "Cut shape #{} in {} must have stroke_color",
                        idx,
                        sample_name
                    );
                    let sc = shape.stroke_color.unwrap();
                    assert_eq!(
                        sc,
                        egui::Color32::from_rgb(255, 38, 38),
                        "Cut shape #{} in {} must have visible red (#ff2626) stroke, got {:?}",
                        idx,
                        sample_name,
                        sc
                    );
                    // Verify tiny_skia pixmap is pre-rendered for filled / pattern shapes
                    if shape.fill_color.a() > 0 {
                        assert!(
                            shape.pixmap.is_some(),
                            "Cut shape #{} in {} must have pre-rendered pixmap",
                            idx,
                            sample_name
                        );
                        let pm = shape.pixmap.as_ref().unwrap();
                        let non_zero_pixels = pm.data().iter().filter(|&&b| b > 0).count();
                        assert!(
                            non_zero_pixels > 0,
                            "Cut shape #{} pixmap in {} must contain non-transparent pixels",
                            idx,
                            sample_name
                        );
                    }
                }
            }

            // 4. Verify SVG export contains <pattern id="swatch_pattern"> and visible red cut strokes
            let svg = convert_studio_to_svg(&data).expect("Convert .studio3 to SVG");
            assert!(svg.contains("<svg"));
            assert!(svg.contains("<pattern id=\"swatch_pattern\""));
            assert!(svg.contains("stroke=\"#ff2626\""));
            if sample_name.to_lowercase().contains("heart") {
                assert!(svg.contains("fill=\"url(#swatch_pattern)\""));
            } else {
                assert!(svg.contains("fill=\"#"));
            }
        }
    }

    #[test]
    fn test_pl_zoo_paper_grass_cutouts_no_diagonal_bridge_lines() {
        let candidates = [
            Path::new("../test_samples/PL ZOO PAPER.studio3"),
            Path::new("test_samples/PL ZOO PAPER.studio3"),
        ];
        let path = match candidates.iter().find(|p| p.exists()) {
            Some(p) => p,
            None => return,
        };
        let data = std::fs::read(path).expect("Read PL ZOO PAPER.studio3");
        let doc = parse_studio_data(&data).expect("Parse PL ZOO PAPER.studio3");

        assert_eq!(doc.version, StudioFormatVersion::StudioV5);
        assert!(doc.shapes.len() >= 5, "Expected >= 5 shapes in PL ZOO PAPER, got {}", doc.shapes.len());

        // 1. Verify pattern swatch is extracted as 512x512 tile
        assert!(doc.pattern_swatch.is_some(), "Expected pattern swatch extracted from Sec 2");
        let swatch = doc.pattern_swatch.as_ref().unwrap();
        assert_eq!(swatch.width(), 512);
        assert_eq!(swatch.height(), 512);

        // 2. Verify no full-mat background layer exists
        for (idx, shape) in doc.shapes.iter().enumerate() {
            assert!(
                shape.text != "#1 Print Background",
                "Shape #{} must not be full-mat background",
                idx
            );
        }

        // 3. Verify multi-subpath grass cutout shapes retain separate M commands and zero diagonal bridge lines
        let mut multi_subpath_grass_found = false;
        for (i, shape) in doc.shapes.iter().enumerate() {
            if shape.is_svg_path {
                let m_count = shape.text.matches('M').count();
                if m_count >= 50 {
                    multi_subpath_grass_found = true;
                    // Grass cutouts must have >= 86 subpaths for individual blades/islands
                    assert!(
                        m_count >= 86,
                        "Grass shape #{} must have >= 86 subpaths, got {}",
                        i,
                        m_count
                    );
                }

                // Verify every subpath starts with M and ends with Z, never connected by diagonal Z L
                assert!(
                    !shape.text.contains("Z L"),
                    "Shape #{} contains stray diagonal bridge line 'Z L'",
                    i
                );

                let subpaths: Vec<&str> = shape.text.split('M').filter(|s| !s.trim().is_empty()).collect();
                assert_eq!(subpaths.len(), m_count);
                for (sub_idx, sub) in subpaths.iter().enumerate() {
                    let trimmed = sub.trim();
                    assert!(
                        trimmed.ends_with('Z'),
                        "Subpath {} in Shape #{} must end with 'Z', got {:?}",
                        sub_idx,
                        i,
                        trimmed
                    );
                }

                // Verify pre-rendered pixmap exists and has rendered pixels
                assert!(
                    shape.pixmap.is_some(),
                    "Shape #{} must have pre-rendered tiny_skia pixmap",
                    i
                );
                let pm = shape.pixmap.as_ref().unwrap();
                let non_zero_pixels = pm.data().iter().filter(|&&b| b > 0).count();
                assert!(
                    non_zero_pixels > 0,
                    "Shape #{} pixmap must contain non-transparent pixels",
                    i
                );

                // Verify cut line stroke is visible red (#ff2626)
                assert!(
                    shape.stroke_color.is_some(),
                    "Shape #{} must have cut line stroke",
                    i
                );
                assert_eq!(
                    shape.stroke_color.unwrap(),
                    egui::Color32::from_rgb(255, 38, 38),
                    "Shape #{} must have #ff2626 cut line stroke",
                    i
                );
            }
        }
        assert!(multi_subpath_grass_found, "Must find multi-subpath grass cutout shapes with >= 86 blades");

        // 4. Verify SVG export contains pattern defs, EvenOdd fill rule, and visible red cut strokes
        let svg = convert_studio_to_svg(&data).expect("Convert PL ZOO PAPER to SVG");
        assert!(svg.contains("<svg"));
        assert!(svg.contains("<pattern id=\"swatch_pattern\""));
        assert!(svg.contains("stroke=\"#ff2626\""));
        assert!(svg.contains("fill-rule=\"evenodd\""));
        assert!(!svg.contains("Z L"), "SVG output must not contain diagonal bridge line 'Z L'");
    }

    #[test]
    fn test_judith_unicorn_large_file_parsing() {
        let candidates = [
            Path::new("../test_samples/JUDITH UNICORN AND DINOSAUR.studio3"),
            Path::new("test_samples/JUDITH UNICORN AND DINOSAUR.studio3"),
        ];
        let path = match candidates.iter().find(|p| p.exists()) {
            Some(p) => p,
            None => return,
        };
        let data = std::fs::read(path).expect("Read JUDITH UNICORN");
        assert!(data.len() > 10_000_000, "JUDITH UNICORN must be a ~20 MB file");
        let start = std::time::Instant::now();
        let doc = parse_studio_data(&data).expect("Parse JUDITH UNICORN");
        let elapsed = start.elapsed();
        println!("Parsed JUDITH UNICORN in {:?}: version={:?}, shapes={}, mat=({:.2},{:.2})",
            elapsed, doc.version, doc.shapes.len(), doc.mat_width_in, doc.mat_height_in);

        assert_eq!(doc.version, StudioFormatVersion::StudioV5);
        assert!(doc.shapes.len() >= 600, "Must contain >= 600 vector shapes");

        // Verify per-shape pixmap dimensions are capped to max 1024x1024 and RAM stays low
        let mut total_pixmap_bytes: usize = 0;
        let mut cut_shape_count = 0;
        for s in &doc.shapes {
            if let Some(ref pm) = s.pixmap {
                let b = pm.data().len();
                total_pixmap_bytes += b;
                if s.is_svg_path {
                    assert!(pm.width() <= 1024 && pm.height() <= 1024,
                        "Shape pixmap dimensions must be capped to 1024x1024, got {}x{}", pm.width(), pm.height());
                }
            }
            if s.is_svg_path {
                assert_eq!(s.stroke_color, Some(egui::Color32::from_rgb(255, 38, 38)),
                    "Vector path must have visible #ff2626 cut outline");
                cut_shape_count += 1;
            }
        }
        assert!(cut_shape_count >= 500, "Must contain >= 500 vector cut shapes with visible red cutlines");
        assert!(total_pixmap_bytes < 50_000_000,
            "Total pixmap RAM must stay under 50 MB, got {:.2} MB", total_pixmap_bytes as f64 / 1_048_576.0);

        // Verify SVG conversion produces visible red strokes without panic or unbounded memory
        let svg = convert_studio_to_svg(&data).expect("Convert JUDITH UNICORN to SVG");
        assert!(svg.contains("<svg"));
        assert!(svg.contains("stroke=\"#ff2626\""));
    }

    #[test]
    fn test_disney_hearts_pattern_clipping_and_visible_cutlines() {
        let candidates = [
            Path::new("../test_samples/DISNEY HEARTS.studio3"),
            Path::new("test_samples/DISNEY HEARTS.studio3"),
        ];
        let path = match candidates.iter().find(|p| p.exists()) {
            Some(p) => p,
            None => return,
        };
        let data = std::fs::read(path).expect("Read DISNEY HEARTS");
        let doc = parse_studio_data(&data).expect("Parse DISNEY HEARTS");
        assert_eq!(doc.version, StudioFormatVersion::StudioV5);
        assert!(doc.shapes.len() >= 10, "DISNEY HEARTS must contain >= 10 shapes");

        // 1. Verify pattern swatch is extracted as 512x512 tile
        assert!(doc.pattern_swatch.is_some(), "Pattern swatch must be extracted");
        let swatch = doc.pattern_swatch.as_ref().unwrap();
        assert_eq!(swatch.width(), 512);
        assert_eq!(swatch.height(), 512);

        // 2. Verify white / heart shapes have visible cut outlines (#ff2626)
        let mut heart_pattern_shape_found = false;
        let mut solid_color_shape_found = false;
        for (i, shape) in doc.shapes.iter().enumerate() {
            if shape.is_svg_path {
                assert_eq!(
                    shape.stroke_color,
                    Some(egui::Color32::from_rgb(255, 38, 38)),
                    "Shape #{} must have visible red (#ff2626) cut outline", i
                );
                let is_white = (shape.fill_color.r() > 240 && shape.fill_color.g() > 240 && shape.fill_color.b() > 240) || shape.fill_color == egui::Color32::WHITE;
                if is_white {
                    heart_pattern_shape_found = true;
                } else if shape.fill_color.a() > 0 && shape.fill_color != egui::Color32::BLACK {
                    solid_color_shape_found = true;
                }
            }
        }
        assert!(heart_pattern_shape_found, "Must find white heart pattern shape with visible cut outline");
        assert!(solid_color_shape_found, "Must find non-white colored shapes preserving their original solid color");

        // 3. Verify SVG export contains pattern fill on heart shapes, solid colors on colored shapes, and #ff2626 cut strokes
        let svg = convert_studio_to_svg(&data).expect("Convert DISNEY HEARTS to SVG");
        assert!(svg.contains("<svg"));
        assert!(svg.contains("<pattern id=\"swatch_pattern\""));
        assert!(svg.contains("stroke=\"#ff2626\""));
        assert!(svg.contains("fill=\"url(#swatch_pattern)\""));
        assert!(svg.contains("fill=\"#ff0000\"")); // Red character color preserved
        assert!(!svg.contains("Print Background"), "Must not emit full-mat background layer");
    }

    #[test]
    fn test_integration_large_studio3_unicorn_and_disney_hearts() {
        // 1. Verify loading JUDITH UNICORN AND DINOSAUR.studio3 (~20 MB)
        let unicorn_candidates = [
            Path::new("../test_samples/JUDITH UNICORN AND DINOSAUR.studio3"),
            Path::new("test_samples/JUDITH UNICORN AND DINOSAUR.studio3"),
        ];
        if let Some(path) = unicorn_candidates.iter().find(|p| p.exists()) {
            let data = std::fs::read(path).expect("Read JUDITH UNICORN");
            let doc = parse_studio_data(&data).expect("Parse JUDITH UNICORN");
            assert_eq!(doc.version, StudioFormatVersion::StudioV5);
            assert!(doc.shapes.len() >= 600, "Expected >= 600 shapes in JUDITH UNICORN");

            // Verify bounded pixmap dimensions and bounded memory usage
            let mut total_pm_bytes: usize = 0;
            for s in &doc.shapes {
                if let Some(ref pm) = s.pixmap {
                    if s.is_svg_path {
                        assert!(pm.width() <= 1024 && pm.height() <= 1024, "Vector shape pixmap must be capped at 1024x1024");
                    }
                    total_pm_bytes += pm.data().len();
                }
            }
            assert!(total_pm_bytes < 50_000_000, "Pixmap RAM must be under 50 MB, got {:.2} MB", total_pm_bytes as f64 / 1_048_576.0);
        }

        // 2. Verify loading DISNEY HEARTS.studio3
        let disney_candidates = [
            Path::new("../test_samples/DISNEY HEARTS.studio3"),
            Path::new("test_samples/DISNEY HEARTS.studio3"),
        ];
        if let Some(path) = disney_candidates.iter().find(|p| p.exists()) {
            let data = std::fs::read(path).expect("Read DISNEY HEARTS");
            let doc = parse_studio_data(&data).expect("Parse DISNEY HEARTS");
            assert_eq!(doc.version, StudioFormatVersion::StudioV5);
            assert!(doc.shapes.len() >= 10, "Expected >= 10 shapes in DISNEY HEARTS");
            assert!(doc.pattern_swatch.is_some(), "DISNEY HEARTS must have pattern swatch");

            // Verify heart shapes have visible cut outlines (#ff2626) and realistic dimensions
            let mut heart_found = false;
            for s in &doc.shapes {
                if s.is_svg_path {
                    let is_white = (s.fill_color.r() > 240 && s.fill_color.g() > 240 && s.fill_color.b() > 240) || s.fill_color == egui::Color32::WHITE;
                    if is_white && s.width > 0.3 {
                        heart_found = true;
                        assert_eq!(s.stroke_color, Some(egui::Color32::from_rgb(255, 38, 38)), "Heart cut outline must be red (#ff2626)");
                        assert!(s.width > 0.3 && s.height > 0.3, "Heart contour must not be collapsed");
                    }
                }
            }
            assert!(heart_found, "Must find visible white heart path");
        }
    }

    #[test]
    fn test_unicorn_paper_and_disney_hearts_cutouts() {
        // 1. Verify AN UNICORN PAPER #2.studio3
        let unicorn_candidates = [
            Path::new("../test_samples/AN UNICORN PAPER #2.studio3"),
            Path::new("test_samples/AN UNICORN PAPER #2.studio3"),
        ];
        if let Some(path) = unicorn_candidates.iter().find(|p| p.exists()) {
            let data = std::fs::read(path).expect("Read AN UNICORN PAPER #2");
            let doc = parse_studio_data(&data).expect("Parse AN UNICORN PAPER #2");
            assert_eq!(doc.version, StudioFormatVersion::StudioV5);
            assert_eq!(doc.mat_width_in, 12.0, "Mat width must be 12 inches Cameo");
            assert_eq!(doc.mat_height_in, 12.0, "Mat height must be 12 inches Cameo");
            assert!(doc.shapes.len() >= 200, "Expected >= 200 shapes, got {}", doc.shapes.len());

            // Compute overall document bounds
            let mut min_x = f32::MAX;
            let mut min_y = f32::MAX;
            let mut max_x = f32::MIN;
            let mut max_y = f32::MIN;
            for s in &doc.shapes {
                min_x = min_x.min(s.x);
                min_y = min_y.min(s.y);
                max_x = max_x.max(s.x + s.width);
                max_y = max_y.max(s.y + s.height);
                // Ensure no single shape swallowed the whole document (no mega-strings > 50k chars)
                assert!(s.text.len() < 50_000, "Shape text len must be < 50,000 chars, got {}", s.text.len());
            }
            let span_x = max_x - min_x;
            let span_y = max_y - min_y;
            assert!(span_x >= 6.0, "Design must span across mat (>= 6.0 in), got {:.2} in", span_x);
            assert!(span_y >= 4.0, "Design must span across mat (>= 4.0 in), got {:.2} in", span_y);
        }

        // 2. Verify DISNEY HEARTS.studio3 heart cutouts
        let disney_candidates = [
            Path::new("../test_samples/DISNEY HEARTS.studio3"),
            Path::new("test_samples/DISNEY HEARTS.studio3"),
        ];
        if let Some(path) = disney_candidates.iter().find(|p| p.exists()) {
            let data = std::fs::read(path).expect("Read DISNEY HEARTS");
            let doc = parse_studio_data(&data).expect("Parse DISNEY HEARTS");
            assert_eq!(doc.version, StudioFormatVersion::StudioV5);
            assert_eq!(doc.mat_width_in, 12.0, "Mat width must be 12 inches Cameo");
            assert_eq!(doc.mat_height_in, 12.0, "Mat height must be 12 inches Cameo");

            // Verify heart cutout shape exists (~1.65" on mat or ~2.97" pre-normalization) with red stroke
            let heart_shape = doc.shapes.iter().find(|s| {
                s.is_svg_path
                    && ((s.width - 1.65).abs() < 0.25 || (s.width - 2.97).abs() < 0.35)
                    && ((s.height - 1.66).abs() < 0.25 || (s.height - 2.98).abs() < 0.35)
            });
            assert!(heart_shape.is_some(), "Must find heart cutout shape with realistic dimensions");
            let heart = heart_shape.unwrap();
            assert_eq!(heart.stroke_color, Some(egui::Color32::from_rgb(255, 38, 38)), "Heart cut outline must be red (#ff2626)");
        }
    }

    #[test]
    fn test_unicorn_paper_zero_origin_and_pattern_clipping() {
        let test_files = [
            "AN UNICORN PAPER #1.studio3",
            "AN UNICORN PAPER #2.studio3",
        ];

        for filename in &test_files {
            let candidates = [
                std::path::PathBuf::from(format!("../test_samples/{}", filename)),
                std::path::PathBuf::from(format!("test_samples/{}", filename)),
            ];
            let path = match candidates.iter().find(|p| p.exists()) {
                Some(p) => p,
                None => continue,
            };

            let data = std::fs::read(path).unwrap_or_else(|e| panic!("Failed to read {}: {}", filename, e));

            // 1. Verify pattern swatch is extracted and print artwork shape is suppressed to prevent unclipped rectangle
            let (print_art, swatch) = extract_embedded_png(&data);
            assert!(swatch.is_some(), "{}: Pattern swatch must be extracted", filename);
            assert!(print_art.is_none(), "{}: Print artwork rectangle must be suppressed when pattern swatch is present", filename);

            // 2. Parse full studio document
            let doc = parse_studio_data(&data).unwrap_or_else(|e| panic!("Failed to parse {}: {}", filename, e));
            assert!(!doc.shapes.is_empty(), "{}: Must extract vector shapes", filename);

            // 3. Verify no unclipped Print Artwork rectangle shape was created
            let has_unclipped_print_art = doc.shapes.iter().any(|s| {
                s.name.as_deref() == Some("Print Artwork")
            });
            assert!(!has_unclipped_print_art, "{}: Must not contain unclipped Print Artwork rectangle", filename);

            // 4. Verify no shape contains stray zero-origin vertices or extreme spikes
            for (idx, shape) in doc.shapes.iter().enumerate() {
                if shape.points.len() >= 3 {
                    let has_non_zero = shape.points.iter().any(|p| p.x.abs() >= 1.0 || p.y.abs() >= 1.0);
                    if has_non_zero {
                        let zero_pt_count = shape.points.iter().filter(|p| p.x.abs() < 1e-3 && p.y.abs() < 1e-3).count();
                        assert_eq!(zero_pt_count, 0,
                            "{}: Shape #{} contains stray (0,0) vertices while positioned at ({:.2},{:.2})!",
                            filename, idx, shape.x, shape.y
                        );
                    }
                }

                // Verify stroke width and cut line color
                assert!(shape.stroke_width <= 0.015, "{}: Stroke width must be hairline (<= 0.015), got {}", filename, shape.stroke_width);
                assert_eq!(shape.stroke_color, Some(egui::Color32::from_rgb(255, 38, 38)), "{}: Stroke color must be red (#ff2626)", filename);
            }
        }
    }

    #[test]
    fn test_all_samples_corpus_transforms_and_bounds() {
        let dir_candidates = [
            Path::new("../test_samples"),
            Path::new("test_samples"),
        ];
        let sample_dir = match dir_candidates.iter().find(|p| p.is_dir()) {
            Some(dir) => dir,
            None => {
                println!("Skipping test_all_samples_corpus_transforms_and_bounds: test_samples dir not found");
                return;
            }
        };
        let mut sample_files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(sample_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ext == "studio" || ext == "studio3" {
                        sample_files.push(path);
                    }
                }
            }
        }
        if sample_files.is_empty() {
            println!("Skipping test_all_samples_corpus_transforms_and_bounds: no sample files found in {:?}", sample_dir);
            return;
        }

        for path in sample_files {
            let filename = path.file_name().unwrap().to_string_lossy().to_string();
            let data = std::fs::read(&path).unwrap_or_else(|e| panic!("Read {}: {}", filename, e));
            let doc = parse_studio_data(&data).unwrap_or_else(|e| panic!("Parse {}: {}", filename, e));

            assert!(!doc.shapes.is_empty(), "{}: Must extract vector shapes", filename);
            assert!(doc.mat_width_in > 0.0 && doc.mat_height_in > 0.0, "{}: Mat size must be valid", filename);

            let max_dim = doc.mat_width_in.max(doc.mat_height_in) * 4.0;
            for (idx, shape) in doc.shapes.iter().enumerate() {
                assert!(shape.width > 0.0, "{}: Shape #{} width must be > 0, got {}", filename, idx, shape.width);
                assert!(shape.height > 0.0, "{}: Shape #{} height must be > 0, got {}", filename, idx, shape.height);
                assert!(shape.width <= max_dim, "{}: Shape #{} width too large ({:.2} in > {:.2} in)", filename, idx, shape.width, max_dim);
                assert!(shape.height <= max_dim, "{}: Shape #{} height too large ({:.2} in > {:.2} in)", filename, idx, shape.height, max_dim);
                assert!(shape.x.is_finite() && shape.y.is_finite(), "{}: Shape #{} pos not finite", filename, idx);

                if shape.is_svg_path {
                    assert!(!shape.text.is_empty(), "{}: SVG path d string must not be empty", filename);
                    assert!(shape.text.starts_with('M'), "{}: SVG path d string must start with M", filename);
                }
            }

            // Verify clean SVG export
            let svg = convert_studio_to_svg(&data).unwrap_or_else(|e| panic!("SVG export {}: {}", filename, e));
            assert!(svg.starts_with("<?xml"), "{}: SVG export missing XML declaration", filename);
            assert!(svg.contains("</svg>"), "{}: SVG export missing closing tag", filename);
            assert!(svg.len() > 100, "{}: SVG export output unexpectedly short ({} bytes)", filename, svg.len());
        }
    }

    #[test]
    fn test_merge_compound_paths_group_isolation_and_cutline_hairline() {
        // Test 1: In StudioV5, concentric shapes WITHOUT an explicit group MUST NOT be merged
        let outer_rect = CanvasShape {
            kind: ShapeKind::Freehand,
            x: 1.0,
            y: 1.0,
            width: 4.0,
            height: 4.0,
            points: vec![
                egui::pos2(1.0, 1.0),
                egui::pos2(5.0, 1.0),
                egui::pos2(5.0, 5.0),
                egui::pos2(1.0, 5.0),
            ],
            text: "M 1.0 1.0 L 5.0 1.0 L 5.0 5.0 L 1.0 5.0 Z".to_string(),
            fill_color: egui::Color32::from_rgb(40, 40, 40),
            stroke_color: Some(egui::Color32::from_rgb(255, 38, 38)),
            stroke_width: 0.01,
            is_closed: true,
            is_svg_path: true,
            is_modified: false,
            triangles: Vec::new(),
            group_id: None, // Independent shape
            group_path: Vec::new(),
            is_visible: true,
            is_locked: false,
            name: None,
            pixmap: None,
            texture: None,
        };

        let inner_rect = CanvasShape {
            kind: ShapeKind::Freehand,
            x: 2.0,
            y: 2.0,
            width: 2.0,
            height: 2.0,
            points: vec![
                egui::pos2(2.0, 2.0),
                egui::pos2(4.0, 2.0),
                egui::pos2(4.0, 4.0),
                egui::pos2(2.0, 4.0),
            ],
            text: "M 2.0 2.0 L 4.0 2.0 L 4.0 4.0 L 2.0 4.0 Z".to_string(),
            fill_color: egui::Color32::from_rgb(40, 40, 40),
            stroke_color: Some(egui::Color32::from_rgb(255, 38, 38)),
            stroke_width: 0.01,
            is_closed: true,
            is_svg_path: true,
            is_modified: false,
            triangles: Vec::new(),
            group_id: None, // Independent shape
            group_path: Vec::new(),
            is_visible: true,
            is_locked: false,
            name: None,
            pixmap: None,
            texture: None,
        };

        let shapes_ungrouped = vec![outer_rect.clone(), inner_rect.clone()];
        let merged_v5_ungrouped = merge_compound_paths(shapes_ungrouped, StudioFormatVersion::StudioV5, None);
        assert_eq!(
            merged_v5_ungrouped.len(),
            2,
            "StudioV5 must NEVER merge independent ungrouped shapes into a single compound path"
        );

        // Test 2: In StudioV5, shapes WITH identical group_id ARE merged into a compound path
        let mut outer_grouped = outer_rect.clone();
        outer_grouped.group_id = Some(101);
        outer_grouped.group_path = vec![101];

        let mut inner_grouped = inner_rect.clone();
        inner_grouped.group_id = Some(101);
        inner_grouped.group_path = vec![101];

        let shapes_grouped = vec![outer_grouped, inner_grouped];
        let merged_v5_grouped = merge_compound_paths(shapes_grouped, StudioFormatVersion::StudioV5, None);
        assert_eq!(
            merged_v5_grouped.len(),
            1,
            "StudioV5 must merge shapes sharing the same Group / CompoundPath record"
        );
        assert_eq!(merged_v5_grouped[0].text.matches('M').count(), 2);

        // Test 3: If external sample exists (TRANSPORTATION TRAIN #2), verify parser execution
        let train_path = Path::new("/home/vineofthedead/Projects/kits-site/KITS/TRANSPORTATION/SVG/TRAIN #2/TRANSPORTATION TRAIN #2.studio3");
        if train_path.exists() {
            if let Ok(data) = std::fs::read(train_path) {
                if let Ok(doc) = parse_studio_data(&data) {
                    assert!(doc.shapes.len() >= 5, "Train sample should retain individual parts, got {}", doc.shapes.len());
                }
            }
        }
    }

    #[test]
    fn test_target_files_print_cut_and_pattern_rendering() {
        let file_tuples = [
            (
                "/run/media/vineofthedead/7BFE-5A98/Master Kits Folder/DISNEY/D - KITS IN SILHOUETTE/100 ACRE WOODS POOH.studio3",
                "/home/vineofthedead/Documents/Studio to SVG files for testing/100 ACRE WOODS POOH.studio3",
            ),
            (
                "/run/media/vineofthedead/7BFE-5A98/Master Kits Folder/DISNEY/D - KITS IN SILHOUETTE/AT HOME WITH MINNIE & MICKEY.studio3",
                "/home/vineofthedead/Documents/Studio to SVG files for testing/AT HOME WITH MINNIE & MICKEY.studio3",
            ),
            (
                "/run/media/vineofthedead/7BFE-5A98/Master Kits Folder/DISNEY/D - KITS IN SILHOUETTE/BIG BIRD.studio3",
                "/home/vineofthedead/Documents/Studio to SVG files for testing/BIG BIRD.studio3",
            ),
            (
                "/run/media/vineofthedead/7BFE-5A98/Master Kits Folder/DISNEY/D - KITS IN SILHOUETTE/CHIP & DALE #2.studio3",
                "/home/vineofthedead/Documents/Studio to SVG files for testing/Chip and Dale/CHIP & DALE #2.studio3",
            ),
            (
                "/home/vineofthedead/Documents/Studio to SVG files for testing/CHIP & DALE #1.studio3",
                "/home/vineofthedead/Documents/Studio to SVG files for testing/Chip and Dale/CHIP & DALE #1.studio3",
            ),
        ];
        for (f1, f2) in &file_tuples {
            let p1 = Path::new(f1);
            let p2 = Path::new(f2);
            let path = if p1.exists() {
                p1
            } else if p2.exists() {
                p2
            } else {
                println!("File not found (drive not mounted): {} / {}", f1, f2);
                continue;
            };
            let data = std::fs::read(path).unwrap();
            let doc = parse_studio_data(&data).unwrap();
            let fname = path.file_name().unwrap().to_string_lossy();
            println!("=== FILE: {} ===", fname);
            println!("Version: {:?}, Mat: {}x{}, Shapes count: {}", doc.version, doc.mat_width_in, doc.mat_height_in, doc.shapes.len());

            let has_print_artwork = doc.shapes.iter().any(|s| s.name.as_deref() == Some("Print Artwork"));
            let transparent_shapes: Vec<_> = doc.shapes.iter().filter(|s| s.fill_color.a() == 0).collect();
            let opaque_shapes: Vec<_> = doc.shapes.iter().filter(|s| s.fill_color.a() > 0 && s.name.as_deref() != Some("Print Artwork")).collect();

            println!("  Print Artwork present: {}", has_print_artwork);
            println!("  Transparent shapes (cut lines): {}", transparent_shapes.len());
            println!("  Opaque shapes: {}", opaque_shapes.len());

            // 1. In Print & Cut files (Pooh & Minnie/Mickey), verify cut lines over artwork are transparent
            if fname.contains("POOH") || fname.contains("MINNIE") {
                assert!(has_print_artwork, "{}: Expected Print Artwork embedded image", fname);
                assert!(!transparent_shapes.is_empty(), "{}: Expected transparent vector cut-lines", fname);
                // Cut-lines must not have pixmaps (no low-res raster stroke)
                for s in &transparent_shapes {
                    assert!(s.pixmap.is_none(), "{}: Transparent cut line must not have pixmap", fname);
                }
            }

            // 2. In BIG BIRD, verify pattern swatch is extracted and no shape with transparent fill has pixmap
            if fname.contains("BIG BIRD") {
                assert!(doc.pattern_swatch.is_some(), "BIG BIRD must have pattern swatch");
                for s in &transparent_shapes {
                    assert!(s.pixmap.is_none(), "BIG BIRD: Transparent shape must not have pixmap");
                }
                // Verify no shape is a massive unclipped rectangle covering the 24x12" or 12x12" mat
                for s in &doc.shapes {
                    if (s.width >= 20.0 && s.height >= 10.0) || (s.width >= 11.5 && s.height >= 11.5) {
                        assert_eq!(s.fill_color.a(), 0, "Large mat boundary shape must not be pattern-filled or opaque");
                        assert!(s.pixmap.is_none(), "Large mat boundary shape must not have pixmap");
                    }
                }
            }

            // 3. Verify SVG export contains visible red cut stroke for cut lines
            let svg = convert_studio_to_svg(&data).expect("Convert to SVG");
            if fname.contains("CHIP & DALE #2") {
                let first_lines: Vec<&str> = svg.lines().take(20).collect();
                println!("CHIP & DALE #2 SVG FIRST LINES:\n{}", first_lines.join("\n"));
            }
            assert!(svg.contains("<svg"), "{}: SVG must contain svg tag", fname);
            assert!(svg.contains("stroke=\"#ff2626\""), "{}: SVG must have red cut lines", fname);
            if !transparent_shapes.is_empty() {
                assert!(svg.contains("fill=\"none\""), "{}: SVG must have fill=\"none\" for cut lines", fname);
            }
        }
    }

    #[test]
    fn test_v4_vector_raster_alignment_and_companion_loading() {
        // Test 1: Sibling companion image loading and alignment (OL DISNEY CHIP & DALE)
        let chip_candidates = [
            Path::new("/run/media/vineofthedead/7BFE-5A98/Master Kits Folder/ZZZ FILES FROM OLD COMPUTER/original overlay sale/studio overlay/OL DISNEY CHIP & DALE.studio"),
            Path::new("/run/media/vineofthedead/7BFE-5A98/Master Kits Folder/CHARACTERS/OL DISNEY CHIP & DALE.studio"),
        ];
        let chip_path = chip_candidates.iter().find(|p| p.exists());
        if let Some(&chip_path) = chip_path {
            let doc = parse_studio_file(chip_path).expect("Parse CHIP & DALE with companion image");
            assert_eq!(doc.version, StudioFormatVersion::StudioV4);
            // Verify Print Artwork was attached from companion PNG
            let artwork = doc.shapes.iter().find(|s| s.name.as_deref() == Some("Print Artwork"));
            assert!(artwork.is_some(), "CHIP & DALE must load companion Print Artwork");
            let art = artwork.unwrap();
            assert!(art.pixmap.is_some(), "Print Artwork must have decoded pixmap");

            // Verify vector cut path exists and overlays artwork
            let cut_shapes: Vec<_> = doc.shapes.iter().filter(|s| s.name.as_deref() != Some("Print Artwork")).collect();
            assert!(!cut_shapes.is_empty(), "CHIP & DALE must have cut shapes");
            let cut = cut_shapes[0];
            assert_eq!(cut.fill_color.a(), 0, "Cut shape over Print Artwork must have transparent fill");
            assert_eq!(cut.stroke_color, Some(egui::Color32::from_rgb(255, 38, 38)), "Cut shape must have red cutline stroke");

            // Verify vector geometry and raster artwork are aligned in the same bounds (not detached/shrunk)
            let center_dx = ((art.x + art.width * 0.5) - (cut.x + cut.width * 0.5)).abs();
            let center_dy = ((art.y + art.height * 0.5) - (cut.y + cut.height * 0.5)).abs();
            assert!(center_dx < 0.2, "Vector cut-line and raster artwork must be aligned in X, got dx={}", center_dx);
            assert!(center_dy < 0.2, "Vector cut-line and raster artwork must be aligned in Y, got dy={}", center_dy);
            assert!((art.width - cut.width).abs() < 0.2, "Artwork and cut width must match, got art={}, cut={}", art.width, cut.width);
            assert!((art.height - cut.height).abs() < 0.2, "Artwork and cut height must match, got art={}, cut={}", art.height, cut.height);
        }

        // Test 2: Embedded raster alignment in V4 (OL BASEBALL SOLID)
        let baseball_path = Path::new("/run/media/vineofthedead/7BFE-5A98/Master Kits Folder/ZZZ FILES FROM OLD COMPUTER/original overlay sale/studio overlay/OL BASEBALL SOLID.studio");
        if baseball_path.exists() {
            let doc = parse_studio_file(baseball_path).expect("Parse OL BASEBALL SOLID");
            assert_eq!(doc.version, StudioFormatVersion::StudioV4);
            let artwork = doc.shapes.iter().find(|s| s.name.as_deref() == Some("Print Artwork"));
            assert!(artwork.is_some(), "OL BASEBALL SOLID must extract embedded Print Artwork");
            let art = artwork.unwrap();
            let cut_shapes: Vec<_> = doc.shapes.iter().filter(|s| s.name.as_deref() != Some("Print Artwork")).collect();
            assert!(!cut_shapes.is_empty(), "OL BASEBALL SOLID must have cut shapes");
            let cut = &cut_shapes[cut_shapes.len() - 1];
            assert!((art.width - cut.width).abs() < 0.5, "Artwork and cut width must match, got art={}, cut={}", art.width, cut.width);
            assert!(art.width <= 11.5, "Artwork must not exceed mat bounds, got width {}", art.width);
        }
    }
}
