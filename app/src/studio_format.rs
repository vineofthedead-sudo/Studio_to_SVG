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
    pub pattern_swatch: Option<tiny_skia::Pixmap>,
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
    parse_studio_data(&data)
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
    let (png_shape_opt, pattern_swatch) = extract_embedded_png(data, mat_w_in, mat_h_in);

    // 4. Parse cubic Bézier paths with pattern_swatch reference
    let mut shapes = parse_paths(stream, &layer_name_by_id, pattern_swatch.as_ref());

    // 5. Normalize and center shapes onto the mat canvas
    normalize_shapes_onto_mat(&mut shapes, mat_w_in, mat_h_in);

    // 6. Detect and merge compound paths (e.g. concentric wheels with inner cutouts)
    let mut shapes = merge_compound_paths(shapes, pattern_swatch.as_ref());

    // 7. Add positioned Print & Cut artwork (if any, placed under vector shapes)
    if let Some(png_shape) = png_shape_opt {
        shapes.insert(0, png_shape);
    }

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

            let fill_str = if doc.pattern_swatch.is_some() && shape.fill_color.a() > 0 {
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

            let is_white_or_light = (shape.fill_color.r() > 240 && shape.fill_color.g() > 240 && shape.fill_color.b() > 240) || shape.fill_color == egui::Color32::WHITE;
            let stroke_str = if let Some(sc) = shape.stroke_color {
                if sc.a() > 0 {
                    let hex = format!("#{:02x}{:02x}{:02x}", sc.r(), sc.g(), sc.b());
                    let opacity = sc.a() as f32 / 255.0;
                    if opacity < 0.999 {
                        format!("stroke=\"{}\" stroke-opacity=\"{:.2}\" stroke-width=\"{:.3}\"", hex, opacity, shape.stroke_width.max(0.02))
                    } else {
                        format!("stroke=\"{}\" stroke-width=\"{:.3}\"", hex, shape.stroke_width.max(0.02))
                    }
                } else if doc.pattern_swatch.is_some() || is_white_or_light {
                    format!("stroke=\"#ff2626\" stroke-width=\"{:.3}\"", shape.stroke_width.max(0.02))
                } else {
                    "stroke=\"none\"".to_string()
                }
            } else if doc.pattern_swatch.is_some() || is_white_or_light {
                format!("stroke=\"#ff2626\" stroke-width=\"{:.3}\"", shape.stroke_width.max(0.02))
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
    if lower.contains("a4") {
        return (8.27, 11.69);
    }
    if lower.contains("12x12") || lower.contains("12 x 12") || lower.contains("cameo") {
        return (12.0, 12.0);
    }

    // Default: standard 12"x12" Cameo mat (864 x 864 points)
    (12.0, 12.0)
}

fn preset_to_dims(preset: &str) -> Option<(f32, f32)> {
    let lower = preset.to_lowercase();
    if lower.contains("12x24") {
        Some((12.0, 24.0))
    } else if lower.contains("letter") || lower.contains("8.5x11") {
        Some((8.5, 11.0))
    } else if lower.contains("portrait") || lower.contains("8x12") {
        Some((8.0, 12.0))
    } else if lower.contains("a4") {
        Some((8.27, 11.69))
    } else if lower.contains("12x12") || lower.contains("cameo") {
        Some((12.0, 12.0))
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
fn parse_paths(stream: &[u8], _layers: &BTreeMap<u64, String>, pattern_swatch: Option<&tiny_skia::Pixmap>) -> Vec<CanvasShape> {
    let mut shapes = Vec::new();

    // First attempt: Check for structured SPTH blocks
    if let Some(structured_shapes) = parse_structured_paths(stream, pattern_swatch) {
        if !structured_shapes.is_empty() {
            return structured_shapes;
        }
    }

    // Second attempt: General stream scanning for 01 21 (2 handles), 01 19 (1 handle), and 01 11 (0 handles / corner) records
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

    if vertices_with_pos.is_empty() {
        return shapes;
    }

    let mut subpaths: Vec<StudioSubpath> = Vec::new();
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

    if subpaths.is_empty() {
        return shapes;
    }

    // Pre-scan all style color records in the stream (01 [len] 00 01 [r, g, b, a])
    #[derive(Debug, Clone, Copy)]
    struct StyleRecord {
        pos: usize,
        color: egui::Color32,
    }

    let mut style_records: Vec<StyleRecord> = Vec::new();
    let mut s_idx = 0;
    while s_idx + 23 <= stream.len() {
        if stream[s_idx] == 0x01 && stream[s_idx + 5] == 0x00 && stream[s_idx + 6] == 0x01 {
            if let Some(len) = read_u32_le(stream, s_idx + 1) {
                if (16..=2000).contains(&len) {
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
                                color: egui::Color32::from_rgba_unmultiplied(
                                    (r * 255.0).round() as u8,
                                    (g * 255.0).round() as u8,
                                    (b * 255.0).round() as u8,
                                    255,
                                ),
                            });
                            s_idx = s_idx.saturating_add(5 + (len as usize).min(200));
                            continue;
                        }
                    }
                }
            }
        }
        s_idx = s_idx.saturating_add(1);
    }

    // Group consecutive subpaths into shapes.
    // Consecutive subpaths belonging to the same compound path are separated by 10-byte contour headers (gap <= 48)
    // and share the same style record. Different shapes are separated by gap > 48 or by style records.
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
            let has_style_boundary = style_records.iter().any(|sr| sr.pos > prev.start_pos && sr.pos <= sub.start_pos);
            if gap > 48 || has_style_boundary {
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
        cur_cluster_subs.push(sub);
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

    // Pre-scan explicit Group records (primarily Studio V5 / silhouette05; files)
    #[derive(Debug, Clone, Copy)]
    struct StudioGroupRecord {
        pos: usize,
        transform: AffineTransform2D,
    }

    let mut group_records: Vec<StudioGroupRecord> = Vec::new();
    let group_tag_prefixed = b"\x05\x00\x00\x00Group";
    let mut g_idx = 0;
    while g_idx + 5 <= stream.len() {
        let is_prefixed = g_idx + 9 <= stream.len() && &stream[g_idx..g_idx + 9] == group_tag_prefixed;
        let is_plain = &stream[g_idx..g_idx + 5] == b"Group";
        if is_prefixed || is_plain {
            let tag_offset = if is_prefixed { g_idx + 4 } else { g_idx };
            // Scan backwards up to 200 bytes for the 3x3 matrix preceding this group closure
            let bwd_start = tag_offset.saturating_sub(200);
            if let Some(bwd_slice) = stream.get(bwd_start..tag_offset) {
                if let Some(xfm) = find_transform_in_slice(bwd_slice) {
                    group_records.push(StudioGroupRecord {
                        pos: tag_offset,
                        transform: xfm,
                    });
                }
            }
            g_idx = tag_offset + 5;
            continue;
        }
        g_idx += 1;
    }

    // Macro/layer groups: Group records that immediately follow another Group record (within 350 bytes)
    let mut macro_groups: Vec<StudioGroupRecord> = Vec::new();
    for window in group_records.windows(2) {
        if window[1].pos.saturating_sub(window[0].pos) < 350 {
            macro_groups.push(window[1]);
        }
    }

    // Determine matrix and fill for each cluster
    let mut cluster_data: Vec<(AffineTransform2D, egui::Color32)> = Vec::new();
    let mut last_valid_xfm = AffineTransform2D::default();
    let mut last_valid_fill = egui::Color32::TRANSPARENT;

    for idx in 0..shape_clusters.len() {
        let cluster = &shape_clusters[idx];
        let next_start = shape_clusters.get(idx + 1).map(|p| p.start_pos).unwrap_or(stream.len());

        let cur_style = style_records.iter().rev().find(|sr| sr.pos <= cluster.start_pos);
        let next_style_pos = style_records
            .iter()
            .find(|sr| sr.pos > cluster.start_pos)
            .map(|sr| sr.pos)
            .unwrap_or(stream.len());

        let group_start = cur_style.map(|s| s.pos).unwrap_or(0);
        let group_end = next_style_pos.min(stream.len());

        // 1. Matrix resolution:
        // A. Check gap immediately following this cluster
        let gap_start = cluster.end_pos;
        let gap_end = next_start.min(gap_start.saturating_add(150)).min(stream.len());
        let mut local_xfm = None;
        if gap_start < gap_end {
            if let Some(gap_slice) = stream.get(gap_start..gap_end) {
                local_xfm = find_transform_in_slice(gap_slice);
            }
        }

        // B. Search for group-level transform:
        let final_xfm = if !group_records.is_empty() {
            // Studio V5 explicit hierarchical Group records:
            let imm_group = group_records.iter().find(|g| g.pos >= cluster.end_pos).map(|g| g.transform);
            let layer_group = macro_groups.iter().find(|g| g.pos >= cluster.end_pos).map(|g| g.transform);

            let mut combined_group = AffineTransform2D::default();
            if let Some(imm) = imm_group {
                combined_group = imm;
            }
            if let Some(layer) = layer_group {
                if Some(layer) != imm_group {
                    combined_group = layer.combine(&combined_group);
                }
            }

            match local_xfm {
                Some(local) => combined_group.combine(&local),
                None => combined_group,
            }
        } else {
            // Studio V4: Group interval search
            let mut group_xfm = None;
            if gap_start < group_end {
                if let Some(fwd_slice) = stream.get(gap_start..group_end) {
                    group_xfm = find_transform_in_slice(fwd_slice);
                }
            }
            if group_xfm.is_none() && group_start < cluster.start_pos {
                if let Some(bwd_slice) = stream.get(group_start..cluster.start_pos) {
                    group_xfm = find_transform_in_slice(bwd_slice);
                }
            }

            match (group_xfm, local_xfm) {
                (Some(parent), Some(child)) => {
                    if parent != child {
                        parent.combine(&child)
                    } else {
                        parent
                    }
                }
                (Some(parent), None) => parent,
                (None, Some(child)) => child,
                (None, None) => last_valid_xfm,
            }
        };
        last_valid_xfm = final_xfm;

        // 2. Fill color resolution:
        let final_fill = match cur_style {
            Some(s) => s.color,
            None => {
                let prefix_start = cluster.start_pos.saturating_sub(600);
                if let Some(prefix_slice) = stream.get(prefix_start..cluster.start_pos) {
                    find_fill_color_in_slice(prefix_slice).unwrap_or(last_valid_fill)
                } else {
                    last_valid_fill
                }
            }
        };
        last_valid_fill = final_fill;

        cluster_data.push((final_xfm, final_fill));
    }

    // Document-wide unit check: check maximum transformed coordinate
    let mut doc_max_coord: f32 = 0.0;
    for (cluster, (xfm, _)) in shape_clusters.iter().zip(cluster_data.iter()) {
        for sub in &cluster.subpaths {
            for v in &sub.vertices {
                let p_tx = xfm.apply(v.anchor);
                doc_max_coord = doc_max_coord.max(p_tx.x.abs().max(p_tx.y.abs()));
            }
        }
    }

    let scale_to_in = if doc_max_coord > 24.0 { 1.0 / 25.4 } else { 1.0 };

    let current_stroke = egui::Color32::from_rgb(255, 38, 38); // Silhouette cut line: Red (#ff2626)
    let current_stroke_width = 0.02_f32; // ~1.5 pt
    let current_layer_id = 1_u64;

    for (cluster, (xfm, fill)) in shape_clusters.into_iter().zip(cluster_data.into_iter()) {
        if let Some(shape) = create_canvas_shape_from_subpaths(
            &cluster.subpaths,
            current_layer_id,
            Some(&xfm),
            fill,
            current_stroke,
            current_stroke_width,
            scale_to_in,
            pattern_swatch,
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
                let scale_to_in = if max_coord > 24.0 { 1.0 / 25.4 } else { 1.0 };

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
    val.is_finite() && val.abs() < 50_000.0
}

fn create_canvas_shape_from_subpaths(
    subpaths: &[StudioSubpath],
    layer_id: u64,
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
        if sub.vertices.len() < 2 {
            continue;
        }
        let first = sub.vertices.first()?;
        let last = sub.vertices.last()?;
        let d = (first.anchor.x - last.anchor.x).hypot(first.anchor.y - last.anchor.y);
        let is_closed = d < 1.0 || sub.vertices.len() >= 3;

        let polyline = path_vertices_to_polyline(&sub.vertices, is_closed, transform, scale_to_in, 0.002);
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

    // Pre-render a tiny_skia::Pixmap with FillRule::EvenOdd so multi-subpaths and holes render cleanly
    let dpi = 150.0_f32;
    let pix_w = ((w * dpi).ceil() as u32).max(16);
    let pix_h = ((h * dpi).ceil() as u32).max(16);

    let is_white_or_light = (fill_color.r() > 240 && fill_color.g() > 240 && fill_color.b() > 240) || fill_color == egui::Color32::WHITE;
    let final_stroke_c = if stroke_color.a() > 0 {
        stroke_color
    } else if pattern_swatch.is_some() || is_white_or_light {
        egui::Color32::from_rgb(255, 38, 38)
    } else {
        egui::Color32::TRANSPARENT
    };

    let mut pixmap = None;
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
                if fill_color.a() > 0 {
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
                }
            } else if fill_color.a() > 0 {
                let mut fill_paint = tiny_skia::Paint::default();
                fill_paint.set_color_rgba8(fill_color.r(), fill_color.g(), fill_color.b(), fill_color.a());
                fill_paint.anti_alias = true;
                pm.fill_path(&path, &fill_paint, tiny_skia::FillRule::EvenOdd, tiny_skia::Transform::identity(), None);
            }

            if final_stroke_c.a() > 0 {
                let mut stroke_paint = tiny_skia::Paint::default();
                stroke_paint.set_color_rgba8(final_stroke_c.r(), final_stroke_c.g(), final_stroke_c.b(), final_stroke_c.a());
                stroke_paint.anti_alias = true;
                let st = tiny_skia::Stroke {
                    width: (stroke_width * sx).max(1.5),
                    line_cap: tiny_skia::LineCap::Round,
                    line_join: tiny_skia::LineJoin::Round,
                    ..Default::default()
                };
                pm.stroke_path(&path, &stroke_paint, &st, tiny_skia::Transform::identity(), None);
            }
            pixmap = Some(pm);
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
        stroke_width: stroke_width.max(0.02),
        is_closed: true,
        is_svg_path: true,
        is_modified: false,
        triangles: Vec::new(),
        group_id: Some(layer_id),
        group_path: vec![layer_id],
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
        layer_id,
        transform,
        fill_color,
        stroke_color,
        stroke_width,
        scale_to_in,
        pattern_swatch,
    )
}

fn find_transform_in_slice(slice: &[u8]) -> Option<AffineTransform2D> {
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

    // 2. Look for valid 36-byte 3x3 matrix [a, b, tx, c, d, ty, 0, 0, 1] in slice
    let mut i = 0;
    while i + 36 <= slice.len() {
        if let (Some(a), Some(b), Some(tx), Some(c), Some(d), Some(ty), Some(m20), Some(m21), Some(m22)) = (
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
            if m20.abs() < 1e-4 && m21.abs() < 1e-4 && (m22 - 1.0).abs() < 1e-4 {
                let det = a * d - b * c;
                if (0.001..500.0).contains(&det.abs()) && tx.abs() < 20_000.0 && ty.abs() < 20_000.0 {
                    return Some(AffineTransform2D::new(a, b, tx, c, d, ty));
                }
            }
        }
        i = i.saturating_add(1);
    }

    // 3. Look for valid 24-byte matrix [a, b, tx, c, d, ty] in slice
    let mut i = 0;
    while i + 24 <= slice.len() {
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
        i = i.saturating_add(1);
    }

    None
}

fn find_fill_color_in_slice(slice: &[u8]) -> Option<egui::Color32> {
    if slice.len() < 18 {
        return None;
    }

    // 1. Scan backwards from end of slice for official style record:
    // 01 [len: 4 bytes] 00 01 [r: f32] [g: f32] [b: f32] [a: f32]
    // where 0.0 <= r,g,b <= 1.0 and (a - 1.0).abs() < 1e-3
    for i in (0..slice.len().saturating_sub(22)).rev() {
        if slice[i] == 0x01 && slice[i + 5] == 0x00 && slice[i + 6] == 0x01 {
            if let Some(len) = read_u32_le(slice, i + 1) {
                if (16..=2000).contains(&len) {
                    if let (Some(r), Some(g), Some(b), Some(a)) = (
                        read_f32_le(slice, i + 7),
                        read_f32_le(slice, i + 11),
                        read_f32_le(slice, i + 15),
                        read_f32_le(slice, i + 19),
                    ) {
                        if (0.0..=1.0).contains(&r)
                            && (0.0..=1.0).contains(&g)
                            && (0.0..=1.0).contains(&b)
                            && (a - 1.0).abs() < 1e-3
                        {
                            return Some(egui::Color32::from_rgba_unmultiplied(
                                (r * 255.0).round() as u8,
                                (g * 255.0).round() as u8,
                                (b * 255.0).round() as u8,
                                255,
                            ));
                        }
                    }
                }
            }
        }
    }

    // 2. Fallback: scan backwards for 00 01 [r, g, b, a] directly
    for i in (0..slice.len().saturating_sub(17)).rev() {
        if slice[i] == 0x00 && slice[i + 1] == 0x01 {
            if let (Some(r), Some(g), Some(b), Some(a)) = (
                read_f32_le(slice, i + 2),
                read_f32_le(slice, i + 6),
                read_f32_le(slice, i + 10),
                read_f32_le(slice, i + 14),
            ) {
                if (0.0..=1.0).contains(&r)
                    && (0.0..=1.0).contains(&g)
                    && (0.0..=1.0).contains(&b)
                    && (a - 1.0).abs() < 1e-3
                {
                    return Some(egui::Color32::from_rgba_unmultiplied(
                        (r * 255.0).round() as u8,
                        (g * 255.0).round() as u8,
                        (b * 255.0).round() as u8,
                        255,
                    ));
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
fn extract_embedded_png(data: &[u8], mat_w_in: f32, mat_h_in: f32) -> (Option<CanvasShape>, Option<tiny_skia::Pixmap>) {
    let mut pattern_swatch = None;
    let mut print_artwork_shape = None;

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
                                if pattern_swatch.is_none() || w >= pattern_swatch.as_ref().map_or(0, |s: &tiny_skia::Pixmap| s.width()) {
                                    pattern_swatch = Some(pm);
                                }
                            } else if w >= 400 && h >= 400 && !is_square && print_artwork_shape.is_none() {
                                // Positioned Print & Cut raster artwork
                                let img_aspect = w as f32 / h.max(1) as f32;
                                let mat_aspect = mat_w_in / mat_h_in.max(0.1);
                                let (shape_w, shape_h) = if img_aspect > mat_aspect {
                                    (mat_w_in, mat_w_in / img_aspect)
                                } else {
                                    (mat_h_in * img_aspect, mat_h_in)
                                };
                                let shape_x = ((mat_w_in - shape_w) * 0.5).max(0.0);
                                let shape_y = ((mat_h_in - shape_h) * 0.5).max(0.0);

                                let shape = CanvasShape {
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
                                print_artwork_shape = Some(shape);
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

    (print_artwork_shape, pattern_swatch)
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
fn merge_compound_paths(shapes: Vec<CanvasShape>, pattern_swatch: Option<&tiny_skia::Pixmap>) -> Vec<CanvasShape> {
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
                    hole_indices.push(j);
                }
            }
        }

        if !hole_indices.is_empty() {
            consumed[i] = true;
            for &j in &hole_indices {
                consumed[j] = true;
            }
            let outer = s1;

            // Build compound SVG path d string: M {outer} Z M {inner1} Z M {inner2} Z ...
            let outer_str = outer.points.iter().map(|p| format!("{:.3},{:.3}", p.x, p.y)).collect::<Vec<_>>().join(" L ");
            let mut compound_d = format!("M {} Z", outer_str);
            for &j in &hole_indices {
                let inner = &shapes[j];
                let inner_str = inner.points.iter().map(|p| format!("{:.3},{:.3}", p.x, p.y)).collect::<Vec<_>>().join(" L ");
                compound_d.push_str(&format!(" M {} Z", inner_str));
            }

            // Pre-render a tiny_skia::Pixmap with FillRule::EvenOdd so the holes punch through
            let dpi = 150.0_f32;
            let pix_w = ((outer.width * dpi).ceil() as u32).max(16);
            let pix_h = ((outer.height * dpi).ceil() as u32).max(16);

            let mut pixmap = None;
            if let Some(mut pm) = tiny_skia::Pixmap::new(pix_w, pix_h) {
                let pad = 2.0_f32;
                let sx = (pix_w as f32 - pad * 2.0) / outer.width.max(0.001);
                let sy = (pix_h as f32 - pad * 2.0) / outer.height.max(0.001);

                let mut pb = tiny_skia::PathBuilder::new();
                if let Some(p0) = outer.points.first() {
                    pb.move_to((p0.x - outer.x) * sx + pad, (p0.y - outer.y) * sy + pad);
                    for p in outer.points.iter().skip(1) {
                        pb.line_to((p.x - outer.x) * sx + pad, (p.y - outer.y) * sy + pad);
                    }
                    pb.close();
                }
                for &j in &hole_indices {
                    let inner = &shapes[j];
                    if let Some(p0) = inner.points.first() {
                        pb.move_to((p0.x - outer.x) * sx + pad, (p0.y - outer.y) * sy + pad);
                        for p in inner.points.iter().skip(1) {
                            pb.line_to((p.x - outer.x) * sx + pad, (p.y - outer.y) * sy + pad);
                        }
                        pb.close();
                    }
                }

                if let Some(path) = pb.finish() {
                    if let Some(swatch) = pattern_swatch {
                        if outer.fill_color.a() > 0 {
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
                        }
                    } else if outer.fill_color.a() > 0 {
                        let mut fill_paint = tiny_skia::Paint::default();
                        fill_paint.set_color_rgba8(outer.fill_color.r(), outer.fill_color.g(), outer.fill_color.b(), outer.fill_color.a());
                        fill_paint.anti_alias = true;
                        pm.fill_path(&path, &fill_paint, tiny_skia::FillRule::EvenOdd, tiny_skia::Transform::identity(), None);
                    }

                    let is_white_or_light = (outer.fill_color.r() > 240 && outer.fill_color.g() > 240 && outer.fill_color.b() > 240) || outer.fill_color == egui::Color32::WHITE;
                    let stroke_c = if let Some(sc) = outer.stroke_color {
                        if sc.a() > 0 { sc } else if pattern_swatch.is_some() || is_white_or_light { egui::Color32::from_rgb(255, 38, 38) } else { egui::Color32::TRANSPARENT }
                    } else if pattern_swatch.is_some() || is_white_or_light {
                        egui::Color32::from_rgb(255, 38, 38)
                    } else {
                        egui::Color32::TRANSPARENT
                    };

                    if stroke_c.a() > 0 {
                        let mut stroke_paint = tiny_skia::Paint::default();
                        stroke_paint.set_color_rgba8(stroke_c.r(), stroke_c.g(), stroke_c.b(), stroke_c.a());
                        stroke_paint.anti_alias = true;
                        let st = tiny_skia::Stroke {
                            width: (outer.stroke_width * sx).max(1.5),
                            line_cap: tiny_skia::LineCap::Round,
                            line_join: tiny_skia::LineJoin::Round,
                            ..Default::default()
                        };
                        pm.stroke_path(&path, &stroke_paint, &st, tiny_skia::Transform::identity(), None);
                    }
                    pixmap = Some(pm);
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
            Path::new("../test_samples/DISNEY HEARTS.studio3"),
            Path::new("test_samples/DISNEY HEARTS.studio3"),
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
            assert!(design_w > 6.0 && design_w <= 8.5, "Unexpected design width: {:.2} in", design_w);
            assert!(design_h > 5.0 && design_h <= 11.0, "Unexpected design height: {:.2} in", design_h);

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

                    // Verify tiny_skia pixmap is pre-rendered with drawn pixels (clipped pattern + stroke)
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

            // 4. Verify SVG export contains <pattern id="swatch_pattern"> and visible red cut strokes
            let svg = convert_studio_to_svg(&data).expect("Convert .studio3 to SVG");
            assert!(svg.contains("<svg"));
            assert!(svg.contains("<pattern id=\"swatch_pattern\""));
            assert!(svg.contains("stroke=\"#ff2626\""));
            assert!(svg.contains("fill=\"url(#swatch_pattern)\""));
        }
    }
}
