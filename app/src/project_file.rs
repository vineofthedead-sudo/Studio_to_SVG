//! `.craft` project file format (plain JSON, version 2).
//!
//! Implements serialisation/deserialisation for the Craft Anything papercraft
//! design studio, plus a dependency-free base64 codec used to embed raster
//! pixmaps inside the JSON document.

use std::collections::BTreeMap;
use std::path::Path;

// ── default helpers ────────────────────────────────────────────────────────────

fn default_true() -> bool {
    true
}

fn default_reg_inset() -> f32 {
    0.25
}

// ── CraftShape ─────────────────────────────────────────────────────────────────

/// A single design element stored in a `.craft` file.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CraftShape {
    /// Shape kind: `"Rectangle"`, `"Ellipse"`, `"Line"`, `"Freehand"`, `"Text"`.
    pub kind: String,

    /// Bounding-box origin (mat inches).
    pub x: f32,
    pub y: f32,

    /// Bounding-box dimensions (mat inches).
    pub width: f32,
    pub height: f32,

    /// Control / stroke points for `Freehand` / `Line` shapes (mat inches).
    pub points: Vec<[f32; 2]>,

    /// Text content for `Text` shapes.
    pub text: String,

    /// Fill colour as `[R, G, B, A]`.
    pub fill_color: [u8; 4],

    /// Stroke colour as `[R, G, B, A]`, or `None` for no stroke.
    #[serde(default)]
    pub stroke_color: Option<[u8; 4]>,

    /// Stroke width in mat inches.
    #[serde(default)]
    pub stroke_width: f32,

    /// Whether the freehand path is closed.
    #[serde(default)]
    pub is_closed: bool,

    /// Whether `points` encodes an SVG path rather than raw polyline vertices.
    #[serde(default)]
    pub is_svg_path: bool,

    /// ID of the group this shape belongs to, if any.
    #[serde(default)]
    pub group_id: Option<u64>,

    /// Ancestry chain of group IDs (innermost last) for nested groups.
    #[serde(default)]
    pub group_path: Vec<u64>,

    /// Visibility toggle.
    #[serde(default = "default_true")]
    pub is_visible: bool,

    /// Whether the shape is locked against editing.
    #[serde(default)]
    pub is_locked: bool,

    /// Optional custom name/label for the shape (e.g. "Yellow Shoes", "Base Silhouette").
    #[serde(default)]
    pub name: Option<String>,

    /// Base64-encoded PNG bytes for raster pixmap shapes, or `None`.
    #[serde(default)]
    pub pixmap_b64: Option<String>,

    /// Width of the embedded pixmap in pixels.
    #[serde(default)]
    pub pixmap_width: u32,

    /// Height of the embedded pixmap in pixels.
    #[serde(default)]
    pub pixmap_height: u32,
}

// ── CraftProject ───────────────────────────────────────────────────────────────

/// Top-level document stored in a `.craft` file.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CraftProject {
    /// File-format version — currently `2`.
    pub version: u32,

    /// Application identifier — `"Craft Anything"`.
    pub app_name: String,

    /// Design-mat width in inches.
    pub mat_width_in: f32,

    /// Design-mat height in inches.
    pub mat_height_in: f32,

    /// All shapes in Z-order (bottom → top).
    pub shapes: Vec<CraftShape>,

    /// Human-readable labels keyed by group ID.
    pub group_names: BTreeMap<u64, String>,

    /// Next available group ID (monotonically increasing).
    pub next_group_id: u64,

    /// Base64-encoded PNG for the paper-background image, or `None`.
    #[serde(default)]
    pub paper_image_b64: Option<String>,

    /// Whether registration marks should be rendered / cut.
    #[serde(default)]
    pub show_reg_marks: bool,

    /// Inset from the mat edge for registration marks (inches).
    #[serde(default = "default_reg_inset")]
    pub reg_mark_inset_in: f32,

    /// Document / tab title.
    #[serde(default)]
    pub title: Option<String>,

    /// Original source file path (e.g. imported .studio or .svg file).
    #[serde(default)]
    pub source_path: Option<String>,
}

// ── Base64 codec (no external crates) ─────────────────────────────────────────

const B64_CHARS: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode `data` to a standard base64 string (padded with `=`).
pub fn base64_encode_bytes(data: &[u8]) -> String {
    if data.is_empty() {
        return String::new();
    }

    let full_chunks = data.len() / 3;
    let remainder = data.len() % 3;
    let out_len = (full_chunks + if remainder > 0 { 1 } else { 0 }) * 4;
    let mut out = Vec::with_capacity(out_len);

    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };

        let combined = (b0 << 16) | (b1 << 8) | b2;

        out.push(B64_CHARS[((combined >> 18) & 0x3F) as usize]);
        out.push(B64_CHARS[((combined >> 12) & 0x3F) as usize]);

        if chunk.len() > 1 {
            out.push(B64_CHARS[((combined >> 6) & 0x3F) as usize]);
        } else {
            out.push(b'=');
        }

        if chunk.len() > 2 {
            out.push(B64_CHARS[(combined & 0x3F) as usize]);
        } else {
            out.push(b'=');
        }
    }

    // SAFETY: we only pushed valid ASCII bytes from B64_CHARS and '='.
    unsafe { String::from_utf8_unchecked(out) }
}

/// Build a decode lookup table: index = ASCII byte value, value = 6-bit decoded
/// value or 0xFF for whitespace (skip) or 0xFE for '=' (padding) or 0xFD for
/// invalid.
fn build_decode_table() -> [u8; 256] {
    let mut table = [0xFD_u8; 256];
    for (i, &c) in B64_CHARS.iter().enumerate() {
        table[c as usize] = i as u8;
    }
    table[b'=' as usize] = 0xFE;
    // whitespace characters
    for &ws in b" \t\r\n" {
        table[ws as usize] = 0xFF;
    }
    table
}

/// Decode a standard base64 string to bytes.
///
/// # Errors
/// Returns `Err(String)` when the input contains characters that are not part
/// of the base64 alphabet (excluding whitespace, which is silently skipped).
pub fn base64_decode_bytes(s: &str) -> Result<Vec<u8>, String> {
    if s.is_empty() {
        return Ok(Vec::new());
    }

    let table = build_decode_table();

    // Collect valid (non-whitespace) characters
    let mut chars: Vec<u8> = Vec::with_capacity(s.len());
    for byte in s.bytes() {
        let v = table[byte as usize];
        match v {
            0xFF => {} // whitespace — skip
            0xFD => {
                return Err(format!(
                    "Invalid base64 character: {:?}",
                    byte as char
                ));
            }
            _ => chars.push(v), // 0–63 or 0xFE (padding)
        }
    }

    // Strip trailing padding indicators
    let pad_count = chars.iter().rev().take_while(|&&b| b == 0xFE).count();
    let data_chars = &chars[..chars.len() - pad_count];

    if data_chars.len() % 4 != 0 && (data_chars.len() + pad_count) % 4 != 0 {
        // Allow groups of 4 (possibly with trailing pad)
    }

    let total_len = chars.len();
    if total_len % 4 != 0 {
        return Err(format!(
            "Invalid base64 length: {} (must be multiple of 4 after stripping whitespace)",
            total_len
        ));
    }

    let mut out = Vec::with_capacity((total_len / 4) * 3);

    for group in chars.chunks(4) {
        let c0 = group[0];
        let c1 = group[1];
        let c2 = group[2];
        let c3 = group[3];

        // c0 and c1 must always be real data
        if c0 == 0xFE || c1 == 0xFE {
            return Err("Unexpected padding character in base64 input".to_string());
        }

        let combined = ((c0 as u32) << 18)
            | ((c1 as u32) << 12)
            | (if c2 == 0xFE { 0 } else { (c2 as u32) << 6 })
            | (if c3 == 0xFE { 0 } else { c3 as u32 });

        out.push(((combined >> 16) & 0xFF) as u8);

        if c2 != 0xFE {
            out.push(((combined >> 8) & 0xFF) as u8);
        }
        if c3 != 0xFE {
            out.push((combined & 0xFF) as u8);
        }
    }

    Ok(out)
}

// ── File I/O ───────────────────────────────────────────────────────────────────

/// Serialise `project` to a pretty-printed JSON `.craft` file at `path`.
///
/// # Errors
/// Returns an error if the file cannot be created or if serialisation fails.
pub fn save_craft_project(
    path: &Path,
    project: &CraftProject,
) -> Result<(), Box<dyn std::error::Error>> {
    let json = serde_json::to_string_pretty(project)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Deserialise a `.craft` file from `path` into a [`CraftProject`].
///
/// # Errors
/// Returns an error if the file cannot be read or if the JSON is malformed.
pub fn load_craft_project(path: &Path) -> Result<CraftProject, Box<dyn std::error::Error>> {
    let json = std::fs::read_to_string(path)?;
    let project: CraftProject = serde_json::from_str(&json)?;
    Ok(project)
}

// ── Unit tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // 1. Round-trip encode→decode for 32 known bytes.
    #[test]
    fn test_base64_roundtrip() {
        let input: Vec<u8> = (0u8..32).collect();
        let encoded = base64_encode_bytes(&input);
        let decoded = base64_decode_bytes(&encoded).expect("decode failed");
        assert_eq!(decoded, input, "round-trip mismatch");
    }

    // 2. Empty slice encodes to "", empty string decodes to empty vec.
    #[test]
    fn test_base64_empty() {
        assert_eq!(base64_encode_bytes(&[]), "");
        assert_eq!(
            base64_decode_bytes("").expect("decode empty failed"),
            Vec::<u8>::new()
        );
    }

    // 3. Full CraftProject round-trip through a temp file.
    #[test]
    fn test_craft_project_roundtrip() {
        let shape_a = CraftShape {
            kind: "Rectangle".to_string(),
            x: 1.0,
            y: 2.0,
            width: 3.0,
            height: 4.0,
            points: vec![[0.0, 0.0], [1.0, 1.0]],
            text: String::new(),
            fill_color: [255, 0, 128, 255],
            stroke_color: Some([0, 0, 0, 255]),
            stroke_width: 0.01,
            is_closed: true,
            is_svg_path: false,
            group_id: Some(1),
            group_path: vec![1],
            is_visible: true,
            is_locked: false,
            name: Some("Base Silhouette".to_string()),
            pixmap_b64: None,
            pixmap_width: 0,
            pixmap_height: 0,
        };

        let shape_b = CraftShape {
            kind: "Text".to_string(),
            x: 0.5,
            y: 0.5,
            width: 2.0,
            height: 0.5,
            points: vec![],
            text: "Hello".to_string(),
            fill_color: [0, 0, 0, 255],
            stroke_color: None,
            stroke_width: 0.0,
            is_closed: false,
            is_svg_path: false,
            group_id: None,
            group_path: vec![],
            is_visible: true,
            is_locked: false,
            name: None,
            pixmap_b64: Some("dGVzdA==".to_string()),
            pixmap_width: 10,
            pixmap_height: 20,
        };

        let mut group_names = BTreeMap::new();
        group_names.insert(1u64, "Layer 1".to_string());

        let project = CraftProject {
            version: 2,
            app_name: "Craft Anything".to_string(),
            mat_width_in: 8.5,
            mat_height_in: 11.0,
            shapes: vec![shape_a, shape_b],
            group_names,
            next_group_id: 5,
            paper_image_b64: None,
            show_reg_marks: false,
            reg_mark_inset_in: 0.25,
            title: Some("Roundtrip Design".to_string()),
            source_path: Some("/tmp/sample.studio".to_string()),
        };

        // Write to a temp file.
        let tmp_path = std::env::temp_dir().join("craft_roundtrip_test.craft");
        save_craft_project(&tmp_path, &project).expect("save failed");

        // Read back and compare.
        let loaded = load_craft_project(&tmp_path).expect("load failed");
        assert_eq!(loaded, project, "loaded project does not match saved");

        // Clean up.
        let _ = std::fs::remove_file(&tmp_path);
    }

    // 4. Missing optional fields in JSON should use serde defaults.
    #[test]
    fn test_craft_project_load_missing_optionals() {
        // Minimal valid JSON — no paper_image_b64, no show_reg_marks,
        // no reg_mark_inset_in.
        let json = r#"{
            "version": 2,
            "app_name": "Craft Anything",
            "mat_width_in": 12.0,
            "mat_height_in": 12.0,
            "shapes": [],
            "group_names": {},
            "next_group_id": 1
        }"#;

        let project: CraftProject =
            serde_json::from_str(json).expect("parse failed");

        assert_eq!(project.show_reg_marks, false, "show_reg_marks should default to false");
        assert_eq!(project.paper_image_b64, None, "paper_image_b64 should default to None");
        assert!(
            (project.reg_mark_inset_in - 0.25).abs() < f32::EPSILON,
            "reg_mark_inset_in should default to 0.25, got {}",
            project.reg_mark_inset_in
        );
    }
}
