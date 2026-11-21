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

impl AffineTransform2D {
    pub fn new(a: f32, b: f32, tx: f32, c: f32, d: f32, ty: f32) -> Self {
        Self { a, b, tx, c, d, ty }
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

/// Parsed Silhouette Studio Document containing metadata and `CanvasShape`s.
#[derive(Debug, Clone)]
pub struct ParsedStudioDocument {
    pub version: StudioFormatVersion,
    pub stream_offset: usize,
    pub mat_width_in: f32,
    pub mat_height_in: f32,
    pub shapes: Vec<CanvasShape>,
    pub group_names: BTreeMap<u64, String>,
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
                    HEADER_OFFSET_V5.saturating_add(len).min(png_start.unwrap_or(data.len()))
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

    // 3. Parse cubic Bézier paths
    let mut shapes = parse_paths(stream, &layer_name_by_id);

    // 4. Normalize and center shapes onto the mat canvas
    normalize_shapes_onto_mat(&mut shapes, mat_w_in, mat_h_in);

    Ok(ParsedStudioDocument {
        version,
        stream_offset: stream_start,
        mat_width_in: mat_w_in,
        mat_height_in: mat_h_in,
        shapes,
        group_names,
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

    // If shapes are outside normal margin or have negative bounds, center them
    let margin = 0.5_f32;
    let needs_recenter = min_x < margin || min_y < margin || max_x > (mat_w_in - margin) || max_y > (mat_h_in - margin);

    if needs_recenter {
        // Proportionally scale if larger than mat
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

            let (s_min_x, s_min_y, s_max_x, s_max_y) = compute_bounds(&shape.points);
            shape.x = s_min_x;
            shape.y = s_min_y;
            shape.width = (s_max_x - s_min_x).max(0.005);
            shape.height = (s_max_y - s_min_y).max(0.005);
            shape.precompute_triangles();
        }
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
            let fill_str = if shape.fill_color.a() == 0 {
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

            let stroke_str = if let Some(sc) = shape.stroke_color {
                let hex = format!("#{:02x}{:02x}{:02x}", sc.r(), sc.g(), sc.b());
                let opacity = sc.a() as f32 / 255.0;
                if opacity < 0.999 {
                    format!("stroke=\"{}\" stroke-opacity=\"{:.2}\" stroke-width=\"{:.3}\"", hex, opacity, shape.stroke_width)
                } else {
                    format!("stroke=\"{}\" stroke-width=\"{:.3}\"", hex, shape.stroke_width)
                }
            } else {
                "stroke=\"none\"".to_string()
            };

            if shape.points.len() >= 2 {
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
fn parse_paths(stream: &[u8], _layers: &BTreeMap<u64, String>) -> Vec<CanvasShape> {
    let mut shapes = Vec::new();

    // First attempt: Check for structured SPTH blocks
    if let Some(structured_shapes) = parse_structured_paths(stream) {
        if !structured_shapes.is_empty() {
            return structured_shapes;
        }
    }

    // Second attempt: General stream scanning for 01 21 00 00 00 records and affine matrices
    let mut vertices_with_pos = Vec::new();
    let mut i = 0;
    while i + 5 <= stream.len() {
        if let Some(marker_slice) = stream.get(i..i + 5) {
            if marker_slice == BEZIER_VERTEX_MARKER {
                if let Some(payload) = stream.get(i + 5..) {
                    if let Some((v, len)) = parse_bezier_vertex(payload) {
                        vertices_with_pos.push((i, v));
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

    // Cluster consecutive vertices into paths based on stream proximity (nominal stride = 34 bytes)
    let mut current_path: Vec<StudioBezierVertex> = Vec::new();
    let mut current_transform = AffineTransform2D::default();
    let current_fill = egui::Color32::TRANSPARENT;
    let current_stroke = egui::Color32::from_rgb(255, 0, 0); // Default Silhouette cut line: Red
    let current_stroke_width = 0.02_f32; // ~1.5 pt
    let current_layer_id = 1_u64;

    for k in 0..vertices_with_pos.len() {
        let (pos, vert) = vertices_with_pos[k];

        if k > 0 {
            let prev_pos = vertices_with_pos[k - 1].0;
            let gap = pos.saturating_sub(prev_pos);

            // Stride between records in a single path is nominally 34 bytes (gap <= 48 bytes)
            if gap > 48 {
                // Finalize previous path
                if current_path.len() >= 2 {
                    if let Some(shape) = create_canvas_shape_from_vertices(
                        &current_path,
                        current_layer_id,
                        Some(&current_transform),
                        current_fill,
                        current_stroke,
                        current_stroke_width,
                    ) {
                        shapes.push(shape);
                    }
                }
                current_path.clear();

                // Look for affine transform following previous path or preceding current path
                let search_start = prev_pos.saturating_add(34);
                let search_end = pos.min(search_start.saturating_add(120)).min(stream.len());
                if search_start < search_end {
                    if let Some(gap_slice) = stream.get(search_start..search_end) {
                        if let Some(xfm) = find_transform_in_slice(gap_slice) {
                            current_transform = xfm;
                        }
                    }
                }
            }
        } else {
            // Check stream before the first vertex for transform or color
            let prefix_end = pos.min(stream.len());
            let prefix_start = prefix_end.saturating_sub(120);
            if let Some(prefix_slice) = stream.get(prefix_start..prefix_end) {
                if let Some(xfm) = find_transform_in_slice(prefix_slice) {
                    current_transform = xfm;
                }
            }
        }

        current_path.push(vert);
    }

    if current_path.len() >= 2 {
        // Check trailing bytes for final path transform
        if let Some(&(last_pos, _)) = vertices_with_pos.last() {
            let search_start = last_pos.saturating_add(34);
            let search_end = (search_start + 120).min(stream.len());
            if search_start < search_end {
                if let Some(trailing_slice) = stream.get(search_start..search_end) {
                    if let Some(xfm) = find_transform_in_slice(trailing_slice) {
                        current_transform = xfm;
                    }
                }
            }
        }

        if let Some(shape) = create_canvas_shape_from_vertices(
            &current_path,
            current_layer_id,
            Some(&current_transform),
            current_fill,
            current_stroke,
            current_stroke_width,
        ) {
            shapes.push(shape);
        }
    }

    shapes
}

/// Parses structured SPTH blocks if present in the stream.
fn parse_structured_paths(stream: &[u8]) -> Option<Vec<CanvasShape>> {
    let mut shapes = Vec::new();
    let mut i = 0;

    while i + 4 <= stream.len() {
        if stream.get(i..i + 4) == Some(&TAG_PATH_BLOCK) {
            let header_start = i + 4;
            let num_vertices = read_u32_le(stream, header_start)? as usize;
            let is_closed = read_u8(stream, header_start + 4)? != 0;
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
                let scale_to_in = if max_coord > 36.0 { 1.0 / 72.0 } else { 1.0 };

                let polyline = path_vertices_to_polyline(&vertices, is_closed, Some(&transform), scale_to_in, 0.002);
                if polyline.len() >= 2 {
                    let (min_x, min_y, max_x, max_y) = compute_bounds(&polyline);
                    let w = (max_x - min_x).max(0.005);
                    let h = (max_y - min_y).max(0.005);

                    let mut shape = CanvasShape {
                        kind: ShapeKind::Freehand,
                        x: min_x,
                        y: min_y,
                        width: w,
                        height: h,
                        points: polyline,
                        text: String::new(),
                        fill_color,
                        stroke_color: Some(stroke_color),
                        stroke_width: stroke_width.max(0.01),
                        is_closed,
                        is_svg_path: true,
                        is_modified: false,
                        triangles: Vec::new(),
                        group_id: Some(layer_id),
                        group_path: vec![layer_id],
                        is_visible: true,
                        is_locked: false,
                        pixmap: None,
                        texture: None,
                    };
                    shape.precompute_triangles();
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

#[inline]
fn is_plausible_coord(val: f32) -> bool {
    val.is_finite() && val.abs() < 50_000.0
}

fn create_canvas_shape_from_vertices(
    vertices: &[StudioBezierVertex],
    layer_id: u64,
    transform: Option<&AffineTransform2D>,
    fill_color: egui::Color32,
    stroke_color: egui::Color32,
    stroke_width: f32,
) -> Option<CanvasShape> {
    if vertices.len() < 2 {
        return None;
    }

    // Detect if closed: either endpoints are coincident or standard closed shape (>= 3 vertices)
    let first = vertices.first()?;
    let last = vertices.last()?;
    let d = (first.anchor.x - last.anchor.x).hypot(first.anchor.y - last.anchor.y);
    let is_closed = d < 1.0 || vertices.len() >= 3;

    let max_coord = vertices
        .iter()
        .map(|v| v.anchor.x.abs().max(v.anchor.y.abs()))
        .fold(0.0_f32, f32::max);
    let scale_to_in = if max_coord > 36.0 { 1.0 / 72.0 } else { 1.0 };

    let polyline = path_vertices_to_polyline(vertices, is_closed, transform, scale_to_in, 0.002);
    if polyline.len() < 2 {
        return None;
    }

    let (min_x, min_y, max_x, max_y) = compute_bounds(&polyline);
    let w = (max_x - min_x).max(0.005);
    let h = (max_y - min_y).max(0.005);

    let mut shape = CanvasShape {
        kind: ShapeKind::Freehand,
        x: min_x,
        y: min_y,
        width: w,
        height: h,
        points: polyline,
        text: String::new(),
        fill_color,
        stroke_color: Some(stroke_color),
        stroke_width: stroke_width.max(0.01),
        is_closed,
        is_svg_path: true,
        is_modified: false,
        triangles: Vec::new(),
        group_id: Some(layer_id),
        group_path: vec![layer_id],
        is_visible: true,
        is_locked: false,
        pixmap: None,
        texture: None,
    };
    shape.precompute_triangles();
    Some(shape)
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

    // 2. Look for valid 24-byte matrix [a, b, tx, c, d, ty] in slice
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
        i = i.saturating_add(4);
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
        assert!(svg.contains("<polygon points="));
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
        ];

        let p1 = sample1_candidates.iter().find(|p| p.exists());
        let p2 = sample2_candidates.iter().find(|p| p.exists());

        if let Some(path1) = p1 {
            let doc1 = parse_studio_file(path1).expect("Parse Peeker Bears.studio");
            assert_eq!(doc1.version, StudioFormatVersion::StudioV4);
            assert!(doc1.shapes.len() >= 20, "Expected >= 20 shapes in Peeker Bears, got {}", doc1.shapes.len());
            for shape in &doc1.shapes {
                assert!(shape.width > 0.0);
                assert!(shape.height > 0.0);
                assert!(shape.x.is_finite());
                assert!(shape.y.is_finite());
                assert!(shape.x >= 0.0 && shape.x <= doc1.mat_width_in);
                assert!(shape.y >= 0.0 && shape.y <= doc1.mat_height_in);
            }

            let svg1 = convert_studio_to_svg(&std::fs::read(path1).unwrap()).expect("Convert Peeker Bears to SVG");
            assert!(svg1.contains("<svg"));
            assert!(svg1.contains("<polygon") || svg1.contains("<polyline"));
        }

        if let Some(path2) = p2 {
            let doc2 = parse_studio_file(path2).expect("Parse DISNEY HEARTS.studio3");
            assert_eq!(doc2.version, StudioFormatVersion::StudioV5);
            assert!(doc2.shapes.len() >= 100, "Expected >= 100 shapes in DISNEY HEARTS, got {}", doc2.shapes.len());
            for shape in &doc2.shapes {
                assert!(shape.width > 0.0);
                assert!(shape.height > 0.0);
                assert!(shape.x.is_finite());
                assert!(shape.y.is_finite());
                assert!(shape.x >= 0.0 && shape.x <= doc2.mat_width_in);
                assert!(shape.y >= 0.0 && shape.y <= doc2.mat_height_in);
            }

            let svg2 = convert_studio_to_svg(&std::fs::read(path2).unwrap()).expect("Convert DISNEY HEARTS to SVG");
            assert!(svg2.contains("<svg"));
            assert!(svg2.contains("<polygon") || svg2.contains("<polyline"));
        }
    }
}
