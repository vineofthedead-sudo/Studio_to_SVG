use eframe::egui;
use std::path::Path;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let screenshot_path = args.windows(2).find(|w| w[0] == "--screenshot").map(|w| w[1].clone());
    let mut viewport = egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]).with_title("Silhouette Studio");
    if screenshot_path.is_some() { viewport = viewport.with_active(false); }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("Silhouette Studio", options, Box::new(move |_cc| Box::new(StudioApp::new(screenshot_path))))
}

#[derive(PartialEq, Eq, Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub enum Tool {
    Select,
    Line,
    Rectangle,
    Ellipse,
    Text,
    Freehand,
}

#[derive(PartialEq, Eq, Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
    Line,
    Freehand,
    Text,
}

#[derive(Clone, Debug)]
pub struct CanvasShape {
    pub kind: ShapeKind,
    pub x: f32,                  // mat-inch X (left)
    pub y: f32,                  // mat-inch Y (top)
    pub width: f32,              // mat-inch width
    pub height: f32,             // mat-inch height
    pub points: Vec<egui::Pos2>, // mat-inch coordinates for Line or Freehand
    pub text: String,
    pub fill_color: egui::Color32,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SerializableShape {
    pub kind: ShapeKind,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub points: Vec<[f32; 2]>,
    pub text: String,
    pub fill_color: [u8; 4], // [r, g, b, a]
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ProjectData {
    pub mat_width_in: f32,
    pub mat_height_in: f32,
    pub shapes: Vec<SerializableShape>,
}

impl CanvasShape {
    pub fn to_serializable(&self) -> SerializableShape {
        SerializableShape {
            kind: self.kind,
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            points: self.points.iter().map(|p| [p.x, p.y]).collect(),
            text: self.text.clone(),
            fill_color: [
                self.fill_color.r(),
                self.fill_color.g(),
                self.fill_color.b(),
                self.fill_color.a(),
            ],
        }
    }

    pub fn from_serializable(s: SerializableShape) -> Self {
        CanvasShape {
            kind: s.kind,
            x: s.x,
            y: s.y,
            width: s.width,
            height: s.height,
            points: s.points.into_iter().map(|[x, y]| egui::pos2(x, y)).collect(),
            text: s.text,
            fill_color: egui::Color32::from_rgba_unmultiplied(
                s.fill_color[0],
                s.fill_color[1],
                s.fill_color[2],
                s.fill_color[3],
            ),
        }
    }

    pub fn bounding_rect(&self) -> egui::Rect {
        egui::Rect::from_min_size(
            egui::pos2(self.x, self.y),
            egui::vec2(self.width.max(0.01), self.height.max(0.01)),
        )
    }

    pub fn set_x(&mut self, new_x: f32) {
        let dx = new_x - self.x;
        self.translate(egui::vec2(dx, 0.0));
    }

    pub fn set_y(&mut self, new_y: f32) {
        let dy = new_y - self.y;
        self.translate(egui::vec2(0.0, dy));
    }

    pub fn set_width(&mut self, new_w: f32) {
        let new_w = new_w.max(0.05);
        let old_w = self.width.max(0.01);
        if old_w < 0.01 {
            if self.points.len() >= 2 {
                self.points[1].x = self.points[0].x + new_w;
            }
        } else {
            let scale_x = new_w / old_w;
            for p in &mut self.points {
                p.x = self.x + (p.x - self.x) * scale_x;
            }
        }
        self.width = new_w;
    }

    pub fn set_height(&mut self, new_h: f32) {
        let new_h = new_h.max(0.05);
        let old_h = self.height.max(0.01);
        if old_h < 0.01 {
            if self.points.len() >= 2 {
                self.points[1].y = self.points[0].y + new_h;
            }
        } else {
            let scale_y = new_h / old_h;
            for p in &mut self.points {
                p.y = self.y + (p.y - self.y) * scale_y;
            }
        }
        self.height = new_h;
    }

    pub fn translate(&mut self, delta: egui::Vec2) {
        self.x += delta.x;
        self.y += delta.y;
        for p in &mut self.points {
            *p += delta;
        }
    }

    pub fn hit_test(&self, p: egui::Pos2, tolerance: f32) -> bool {
        match self.kind {
            ShapeKind::Rectangle => {
                let r = egui::Rect::from_min_size(
                    egui::pos2(self.x, self.y),
                    egui::vec2(self.width, self.height),
                );
                r.expand(tolerance).contains(p)
            }
            ShapeKind::Ellipse => {
                let cx = self.x + self.width * 0.5;
                let cy = self.y + self.height * 0.5;
                let rx = (self.width * 0.5).max(0.01) + tolerance;
                let ry = (self.height * 0.5).max(0.01) + tolerance;
                let dx = (p.x - cx) / rx;
                let dy = (p.y - cy) / ry;
                dx * dx + dy * dy <= 1.0
            }
            ShapeKind::Line => {
                if self.points.len() >= 2 {
                    dist_to_segment(p, self.points[0], self.points[1]) <= tolerance
                } else {
                    let r = egui::Rect::from_min_size(
                        egui::pos2(self.x, self.y),
                        egui::vec2(self.width, self.height),
                    );
                    r.expand(tolerance).contains(p)
                }
            }
            ShapeKind::Freehand => {
                if self.points.len() >= 2 {
                    self.points.windows(2).any(|w| dist_to_segment(p, w[0], w[1]) <= tolerance)
                } else if let Some(&first) = self.points.first() {
                    (p - first).length() <= tolerance
                } else {
                    false
                }
            }
            ShapeKind::Text => {
                let r = egui::Rect::from_min_size(
                    egui::pos2(self.x, self.y),
                    egui::vec2(self.width, self.height),
                );
                r.expand(tolerance).contains(p)
            }
        }
    }
}

fn dist_to_segment(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_sq();
    if len_sq < 0.0001 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    let proj = a + ab * t;
    (p - proj).length()
}

fn draw_dashed_line(painter: &egui::Painter, from: egui::Pos2, to: egui::Pos2, stroke: egui::Stroke, dash: f32, gap: f32) {
    let dir = to - from;
    let dist = dir.length();
    if dist < 0.001 {
        return;
    }
    let unit = dir / dist;
    let mut curr = 0.0;
    while curr < dist {
        let next = (curr + dash).min(dist);
        painter.line_segment([from + unit * curr, from + unit * next], stroke);
        curr += dash + gap;
    }
}

fn draw_dashed_rect(painter: &egui::Painter, rect: egui::Rect, stroke: egui::Stroke, dash: f32, gap: f32) {
    let tl = rect.left_top();
    let tr = rect.right_top();
    let br = rect.right_bottom();
    let bl = rect.left_bottom();
    draw_dashed_line(painter, tl, tr, stroke, dash, gap);
    draw_dashed_line(painter, tr, br, stroke, dash, gap);
    draw_dashed_line(painter, br, bl, stroke, dash, gap);
    draw_dashed_line(painter, bl, tl, stroke, dash, gap);
}

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

pub struct StudioApp {
    pub active_tool: Tool,
    pub right_panel: &'static str,
    pub zoom: f32,
    pub pan: egui::Vec2,
    pub mat_width_in: f32,
    pub mat_height_in: f32,
    pub fill_color: [f32; 3],
    pub text_sample: String,
    pub shapes: Vec<CanvasShape>,
    pub selected_index: Option<usize>,
    pub drag_start: Option<egui::Pos2>,       // in mat inches
    pub freehand_points: Vec<egui::Pos2>,     // in mat inches
    pub is_dragging_shape: bool,
    pub status_message: Option<(String, std::time::Instant)>,
    pub screenshot_path: Option<String>,
    pub frames_rendered: u32,
}

impl StudioApp {
    pub fn new(screenshot_path: Option<String>) -> Self {
        Self {
            active_tool: Tool::Select,
            right_panel: "Page Setup",
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            mat_width_in: 12.0,
            mat_height_in: 12.0,
            fill_color: [0.85, 0.2, 0.35],
            text_sample: "Sample Design".to_string(),
            shapes: Vec::new(),
            selected_index: None,
            drag_start: None,
            freehand_points: Vec::new(),
            is_dragging_shape: false,
            status_message: None,
            screenshot_path,
            frames_rendered: 0,
        }
    }

    pub fn current_color32(&self) -> egui::Color32 {
        egui::Color32::from_rgb(
            (self.fill_color[0] * 255.0).clamp(0.0, 255.0) as u8,
            (self.fill_color[1] * 255.0).clamp(0.0, 255.0) as u8,
            (self.fill_color[2] * 255.0).clamp(0.0, 255.0) as u8,
        )
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some((msg.into(), std::time::Instant::now()));
    }

    // 1. Save Project to .json
    pub fn save_project_file(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let data = ProjectData {
            mat_width_in: self.mat_width_in,
            mat_height_in: self.mat_height_in,
            shapes: self.shapes.iter().map(|s| s.to_serializable()).collect(),
        };
        let json_str = serde_json::to_string_pretty(&data)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, json_str)?;
        Ok(())
    }

    // 1. Open Project from .json
    pub fn open_project_file(&mut self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let data: ProjectData = serde_json::from_str(&content)?;
        self.mat_width_in = data.mat_width_in;
        self.mat_height_in = data.mat_height_in;
        self.shapes = data.shapes.into_iter().map(CanvasShape::from_serializable).collect();
        self.selected_index = None;
        Ok(())
    }

    // 2. Export SVG string in mat-inch coordinates
    pub fn export_svg_string(&self) -> String {
        let mut out = String::new();
        out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        out.push_str(&format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{:.2}in\" height=\"{:.2}in\" viewBox=\"0 0 {:.2} {:.2}\" version=\"1.1\">\n",
            self.mat_width_in, self.mat_height_in, self.mat_width_in, self.mat_height_in
        ));

        for shape in &self.shapes {
            let hex = format!("#{:02x}{:02x}{:02x}", shape.fill_color.r(), shape.fill_color.g(), shape.fill_color.b());
            let opacity = shape.fill_color.a() as f32 / 255.0;
            let opacity_str = if opacity < 0.999 {
                format!(" fill-opacity=\"{:.2}\"", opacity)
            } else {
                String::new()
            };

            match shape.kind {
                ShapeKind::Rectangle => {
                    out.push_str(&format!(
                        "  <rect x=\"{:.3}\" y=\"{:.3}\" width=\"{:.3}\" height=\"{:.3}\" fill=\"{}\"{} stroke=\"#000000\" stroke-width=\"0.05\" />\n",
                        shape.x, shape.y, shape.width, shape.height, hex, opacity_str
                    ));
                }
                ShapeKind::Ellipse => {
                    let cx = shape.x + shape.width * 0.5;
                    let cy = shape.y + shape.height * 0.5;
                    let rx = shape.width * 0.5;
                    let ry = shape.height * 0.5;
                    out.push_str(&format!(
                        "  <ellipse cx=\"{:.3}\" cy=\"{:.3}\" rx=\"{:.3}\" ry=\"{:.3}\" fill=\"{}\"{} stroke=\"#000000\" stroke-width=\"0.05\" />\n",
                        cx, cy, rx, ry, hex, opacity_str
                    ));
                }
                ShapeKind::Line => {
                    if shape.points.len() >= 2 {
                        out.push_str(&format!(
                            "  <line x1=\"{:.3}\" y1=\"{:.3}\" x2=\"{:.3}\" y2=\"{:.3}\" stroke=\"#000000\" stroke-width=\"0.05\" stroke-linecap=\"round\" />\n",
                            shape.points[0].x, shape.points[0].y, shape.points[1].x, shape.points[1].y
                        ));
                    }
                }
                ShapeKind::Freehand => {
                    if shape.points.len() >= 2 {
                        let pts_str = shape.points.iter()
                            .map(|p| format!("{:.3},{:.3}", p.x, p.y))
                            .collect::<Vec<_>>()
                            .join(" ");
                        out.push_str(&format!(
                            "  <polyline points=\"{}\" fill=\"none\" stroke=\"#000000\" stroke-width=\"0.05\" stroke-linecap=\"round\" stroke-linejoin=\"round\" />\n",
                            pts_str
                        ));
                    }
                }
                ShapeKind::Text => {
                    let escaped = escape_xml(&shape.text);
                    let y_base = shape.y + shape.height * 0.85;
                    out.push_str(&format!(
                        "  <text x=\"{:.3}\" y=\"{:.3}\" font-family=\"sans-serif\" font-size=\"{:.3}\" fill=\"{}\"{}>{}</text>\n",
                        shape.x, y_base, shape.height, hex, opacity_str, escaped
                    ));
                }
            }
        }

        out.push_str("</svg>\n");
        out
    }

    pub fn export_svg_file(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let svg = self.export_svg_string();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, svg)?;
        Ok(())
    }

    // 3. Export PNG (High-DPI mat design without selection dashed box)
    pub fn export_png_file(&self, path: &Path, dpi: f32) -> Result<(), Box<dyn std::error::Error>> {
        let w = ((self.mat_width_in * dpi).round() as u32).max(10);
        let h = ((self.mat_height_in * dpi).round() as u32).max(10);
        let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or("Failed to allocate Pixmap")?;
        pixmap.fill(tiny_skia::Color::WHITE);

        // Mat border & grid
        let mut grid_paint = tiny_skia::Paint::default();
        grid_paint.set_color_rgba8(230, 230, 230, 255);
        let grid_stroke = tiny_skia::Stroke { width: 1.0, ..Default::default() };

        for c in 1..(self.mat_width_in.ceil() as i32) {
            let x = (c as f32) * dpi;
            let mut pb = tiny_skia::PathBuilder::new();
            pb.move_to(x, 0.0);
            pb.line_to(x, h as f32);
            if let Some(path) = pb.finish() {
                pixmap.stroke_path(&path, &grid_paint, &grid_stroke, tiny_skia::Transform::identity(), None);
            }
        }
        for r in 1..(self.mat_height_in.ceil() as i32) {
            let y = (r as f32) * dpi;
            let mut pb = tiny_skia::PathBuilder::new();
            pb.move_to(0.0, y);
            pb.line_to(w as f32, y);
            if let Some(path) = pb.finish() {
                pixmap.stroke_path(&path, &grid_paint, &grid_stroke, tiny_skia::Transform::identity(), None);
            }
        }

        // Mat border (cyan)
        let mut border_paint = tiny_skia::Paint::default();
        border_paint.set_color_rgba8(0, 150, 214, 255);
        let border_stroke = tiny_skia::Stroke { width: 3.0, ..Default::default() };
        if let Some(rect) = tiny_skia::Rect::from_xywh(1.5, 1.5, (w as f32 - 3.0).max(1.0), (h as f32 - 3.0).max(1.0)) {
            let path = tiny_skia::PathBuilder::from_rect(rect);
            pixmap.stroke_path(&path, &border_paint, &border_stroke, tiny_skia::Transform::identity(), None);
        }

        // Font loader for text
        let font_data = std::fs::read("/usr/share/fonts/TTF/DejaVuSans.ttf")
            .or_else(|_| std::fs::read("/usr/share/fonts/liberation/LiberationSans-Regular.ttf"))
            .or_else(|_| std::fs::read("/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf"))
            .ok();

        // Render shapes (WITHOUT selection box)
        let stroke_w = (2.0 * (dpi / 40.0)).max(2.0);
        let black_stroke = tiny_skia::Stroke {
            width: stroke_w,
            line_cap: tiny_skia::LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Default::default()
        };
        let mut black_paint = tiny_skia::Paint::default();
        black_paint.set_color_rgba8(0, 0, 0, 255);
        black_paint.anti_alias = true;

        for shape in &self.shapes {
            let mut fill_paint = tiny_skia::Paint::default();
            fill_paint.set_color_rgba8(
                shape.fill_color.r(),
                shape.fill_color.g(),
                shape.fill_color.b(),
                shape.fill_color.a(),
            );
            fill_paint.anti_alias = true;

            match shape.kind {
                ShapeKind::Rectangle => {
                    if let Some(rect) = tiny_skia::Rect::from_xywh(
                        shape.x * dpi,
                        shape.y * dpi,
                        shape.width * dpi,
                        shape.height * dpi,
                    ) {
                        pixmap.fill_rect(rect, &fill_paint, tiny_skia::Transform::identity(), None);
                        let path = tiny_skia::PathBuilder::from_rect(rect);
                        pixmap.stroke_path(&path, &black_paint, &black_stroke, tiny_skia::Transform::identity(), None);
                    }
                }
                ShapeKind::Ellipse => {
                    if let Some(rect) = tiny_skia::Rect::from_xywh(
                        shape.x * dpi,
                        shape.y * dpi,
                        shape.width * dpi,
                        shape.height * dpi,
                    ) {
                        if let Some(path) = tiny_skia::PathBuilder::from_oval(rect) {
                            pixmap.fill_path(&path, &fill_paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
                            pixmap.stroke_path(&path, &black_paint, &black_stroke, tiny_skia::Transform::identity(), None);
                        }
                    }
                }
                ShapeKind::Line => {
                    if shape.points.len() >= 2 {
                        let mut pb = tiny_skia::PathBuilder::new();
                        pb.move_to(shape.points[0].x * dpi, shape.points[0].y * dpi);
                        pb.line_to(shape.points[1].x * dpi, shape.points[1].y * dpi);
                        if let Some(path) = pb.finish() {
                            pixmap.stroke_path(&path, &black_paint, &black_stroke, tiny_skia::Transform::identity(), None);
                        }
                    }
                }
                ShapeKind::Freehand => {
                    if shape.points.len() >= 2 {
                        let mut pb = tiny_skia::PathBuilder::new();
                        pb.move_to(shape.points[0].x * dpi, shape.points[0].y * dpi);
                        for p in &shape.points[1..] {
                            pb.line_to(p.x * dpi, p.y * dpi);
                        }
                        if let Some(path) = pb.finish() {
                            pixmap.stroke_path(&path, &black_paint, &black_stroke, tiny_skia::Transform::identity(), None);
                        }
                    }
                }
                ShapeKind::Text => {
                    if let Some(ref data) = font_data {
                        if let Ok(font) = ab_glyph::FontRef::try_from_slice(data) {
                            use ab_glyph::{Font, ScaleFont};
                            let font_size = shape.height * dpi;
                            let scaled = font.as_scaled(font_size);
                            let mut cx = shape.x * dpi;
                            let cy = shape.y * dpi + scaled.ascent();
                            for ch in shape.text.chars() {
                                let gid = scaled.glyph_id(ch);
                                let glyph = gid.with_scale_and_position(font_size, ab_glyph::point(cx, cy));
                                cx += scaled.h_advance(gid);
                                if let Some(outlined) = font.outline_glyph(glyph) {
                                    let bounds = outlined.px_bounds();
                                    outlined.draw(|gx, gy, c| {
                                        let px = bounds.min.x as i32 + gx as i32;
                                        let py = bounds.min.y as i32 + gy as i32;
                                        if px >= 0 && px < w as i32 && py >= 0 && py < h as i32 {
                                            let alpha = (c * (shape.fill_color.a() as f32 / 255.0) * 255.0) as u8;
                                            if alpha > 0 {
                                                if let Some(dot) = tiny_skia::Rect::from_xywh(px as f32, py as f32, 1.0, 1.0) {
                                                    let mut dot_paint = tiny_skia::Paint::default();
                                                    dot_paint.set_color_rgba8(shape.fill_color.r(), shape.fill_color.g(), shape.fill_color.b(), alpha);
                                                    pixmap.fill_rect(dot, &dot_paint, tiny_skia::Transform::identity(), None);
                                                }
                                            }
                                        }
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        pixmap.save_png(path)?;
        Ok(())
    }

    // 4. Import SVG into CanvasShapes
    pub fn import_svg_file(&mut self, path: &Path) -> Result<usize, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let imported = import_svg_str(&content, self.current_color32());
        let count = imported.len();
        if count > 0 {
            self.shapes.extend(imported);
            self.selected_index = Some(self.shapes.len() - 1);
        }
        Ok(count)
    }
}

// SVG Import parser
pub fn import_svg_str(xml: &str, default_color: egui::Color32) -> Vec<CanvasShape> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut shapes = Vec::new();
    let mut scale_to_in = 1.0_f32;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let tag_name = e.name();
                let tag = tag_name.as_ref();

                if tag == b"svg" {
                    let mut vb_w: Option<f32> = None;
                    let mut vb_h: Option<f32> = None;
                    let mut w_in: Option<f32> = None;

                    for a in e.attributes().flatten() {
                        match a.key.as_ref() {
                            b"viewBox" | b"viewbox" => {
                                if let Ok(val) = std::str::from_utf8(a.value.as_ref()) {
                                    let nums: Vec<f32> = val
                                        .split(|c: char| c.is_whitespace() || c == ',')
                                        .filter_map(|s| s.parse::<f32>().ok())
                                        .collect();
                                    if nums.len() >= 4 {
                                        vb_w = Some(nums[2]);
                                        vb_h = Some(nums[3]);
                                    }
                                }
                            }
                            b"width" => {
                                if let Ok(val) = std::str::from_utf8(a.value.as_ref()) {
                                    if val.ends_with("in") {
                                        w_in = val[..val.len() - 2].trim().parse::<f32>().ok();
                                    }
                                }
                            }
                            _ => {}
                        }
                    }

                    if let (Some(w), Some(vbw)) = (w_in, vb_w) {
                        if vbw > 0.0 {
                            scale_to_in = w / vbw;
                        }
                    } else if let Some(vbw) = vb_w {
                        if vbw <= 36.0 && vb_h.map_or(true, |h| h <= 36.0) {
                            scale_to_in = 1.0;
                        } else {
                            scale_to_in = 1.0 / 96.0;
                        }
                    }
                } else if tag == b"rect" {
                    let mut x = 0.0;
                    let mut y = 0.0;
                    let mut width = 0.0;
                    let mut height = 0.0;
                    let mut fill = default_color;

                    for a in e.attributes().flatten() {
                        let k = a.key.as_ref();
                        if let Ok(val) = std::str::from_utf8(a.value.as_ref()) {
                            match k {
                                b"x" => x = parse_svg_length(val, scale_to_in),
                                b"y" => y = parse_svg_length(val, scale_to_in),
                                b"width" => width = parse_svg_length(val, scale_to_in),
                                b"height" => height = parse_svg_length(val, scale_to_in),
                                b"fill" => fill = parse_svg_color(val, default_color),
                                _ => {}
                            }
                        }
                    }
                    if width > 0.01 && height > 0.01 {
                        shapes.push(CanvasShape {
                            kind: ShapeKind::Rectangle,
                            x,
                            y,
                            width,
                            height,
                            points: vec![],
                            text: String::new(),
                            fill_color: fill,
                        });
                    }
                } else if tag == b"circle" {
                    let mut cx = 0.0;
                    let mut cy = 0.0;
                    let mut r = 0.0;
                    let mut fill = default_color;

                    for a in e.attributes().flatten() {
                        let k = a.key.as_ref();
                        if let Ok(val) = std::str::from_utf8(a.value.as_ref()) {
                            match k {
                                b"cx" => cx = parse_svg_length(val, scale_to_in),
                                b"cy" => cy = parse_svg_length(val, scale_to_in),
                                b"r" => r = parse_svg_length(val, scale_to_in),
                                b"fill" => fill = parse_svg_color(val, default_color),
                                _ => {}
                            }
                        }
                    }
                    if r > 0.005 {
                        shapes.push(CanvasShape {
                            kind: ShapeKind::Ellipse,
                            x: cx - r,
                            y: cy - r,
                            width: 2.0 * r,
                            height: 2.0 * r,
                            points: vec![],
                            text: String::new(),
                            fill_color: fill,
                        });
                    }
                } else if tag == b"ellipse" {
                    let mut cx = 0.0;
                    let mut cy = 0.0;
                    let mut rx = 0.0;
                    let mut ry = 0.0;
                    let mut fill = default_color;

                    for a in e.attributes().flatten() {
                        let k = a.key.as_ref();
                        if let Ok(val) = std::str::from_utf8(a.value.as_ref()) {
                            match k {
                                b"cx" => cx = parse_svg_length(val, scale_to_in),
                                b"cy" => cy = parse_svg_length(val, scale_to_in),
                                b"rx" => rx = parse_svg_length(val, scale_to_in),
                                b"ry" => ry = parse_svg_length(val, scale_to_in),
                                b"fill" => fill = parse_svg_color(val, default_color),
                                _ => {}
                            }
                        }
                    }
                    if rx > 0.005 && ry > 0.005 {
                        shapes.push(CanvasShape {
                            kind: ShapeKind::Ellipse,
                            x: cx - rx,
                            y: cy - ry,
                            width: 2.0 * rx,
                            height: 2.0 * ry,
                            points: vec![],
                            text: String::new(),
                            fill_color: fill,
                        });
                    }
                } else if tag == b"line" {
                    let mut x1 = 0.0;
                    let mut y1 = 0.0;
                    let mut x2 = 0.0;
                    let mut y2 = 0.0;
                    let mut stroke = egui::Color32::BLACK;

                    for a in e.attributes().flatten() {
                        let k = a.key.as_ref();
                        if let Ok(val) = std::str::from_utf8(a.value.as_ref()) {
                            match k {
                                b"x1" => x1 = parse_svg_length(val, scale_to_in),
                                b"y1" => y1 = parse_svg_length(val, scale_to_in),
                                b"x2" => x2 = parse_svg_length(val, scale_to_in),
                                b"y2" => y2 = parse_svg_length(val, scale_to_in),
                                b"stroke" => stroke = parse_svg_color(val, egui::Color32::BLACK),
                                _ => {}
                            }
                        }
                    }
                    shapes.push(CanvasShape {
                        kind: ShapeKind::Line,
                        x: x1.min(x2),
                        y: y1.min(y2),
                        width: (x1 - x2).abs().max(0.01),
                        height: (y1 - y2).abs().max(0.01),
                        points: vec![egui::pos2(x1, y1), egui::pos2(x2, y2)],
                        text: String::new(),
                        fill_color: stroke,
                    });
                } else if tag == b"polyline" || tag == b"polygon" {
                    let is_polygon = tag == b"polygon";
                    let mut pts_str = String::new();
                    let mut fill = default_color;

                    for a in e.attributes().flatten() {
                        let k = a.key.as_ref();
                        if let Ok(val) = std::str::from_utf8(a.value.as_ref()) {
                            match k {
                                b"points" => pts_str = val.to_string(),
                                b"fill" => fill = parse_svg_color(val, default_color),
                                _ => {}
                            }
                        }
                    }

                    let mut points = parse_points_list(&pts_str, scale_to_in);
                    if is_polygon && points.len() >= 2 {
                        if (points[0] - *points.last().unwrap()).length() > 0.001 {
                            points.push(points[0]);
                        }
                    }
                    if points.len() >= 2 {
                        let mut min_x = points[0].x;
                        let mut min_y = points[0].y;
                        let mut max_x = points[0].x;
                        let mut max_y = points[0].y;
                        for p in &points {
                            min_x = min_x.min(p.x);
                            min_y = min_y.min(p.y);
                            max_x = max_x.max(p.x);
                            max_y = max_y.max(p.y);
                        }
                        shapes.push(CanvasShape {
                            kind: ShapeKind::Freehand,
                            x: min_x,
                            y: min_y,
                            width: (max_x - min_x).max(0.05),
                            height: (max_y - min_y).max(0.05),
                            points,
                            text: String::new(),
                            fill_color: fill,
                        });
                    }
                } else if tag == b"path" {
                    let mut d_str = String::new();
                    let mut fill = default_color;

                    for a in e.attributes().flatten() {
                        let k = a.key.as_ref();
                        if let Ok(val) = std::str::from_utf8(a.value.as_ref()) {
                            match k {
                                b"d" => d_str = val.to_string(),
                                b"fill" => fill = parse_svg_color(val, default_color),
                                _ => {}
                            }
                        }
                    }

                    let subpaths = parse_svg_path_d(&d_str, scale_to_in);
                    for sub in subpaths {
                        if sub.len() >= 2 {
                            let mut min_x = sub[0].x;
                            let mut min_y = sub[0].y;
                            let mut max_x = sub[0].x;
                            let mut max_y = sub[0].y;
                            for p in &sub {
                                min_x = min_x.min(p.x);
                                min_y = min_y.min(p.y);
                                max_x = max_x.max(p.x);
                                max_y = max_y.max(p.y);
                            }
                            shapes.push(CanvasShape {
                                kind: ShapeKind::Freehand,
                                x: min_x,
                                y: min_y,
                                width: (max_x - min_x).max(0.05),
                                height: (max_y - min_y).max(0.05),
                                points: sub,
                                text: String::new(),
                                fill_color: fill,
                            });
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    shapes
}

fn parse_svg_length(s: &str, scale: f32) -> f32 {
    let s = s.trim();
    if s.ends_with("in") {
        s[..s.len() - 2].trim().parse::<f32>().unwrap_or(0.0)
    } else if s.ends_with("cm") {
        s[..s.len() - 2].trim().parse::<f32>().unwrap_or(0.0) / 2.54
    } else if s.ends_with("mm") {
        s[..s.len() - 2].trim().parse::<f32>().unwrap_or(0.0) / 25.4
    } else if s.ends_with("pt") {
        s[..s.len() - 2].trim().parse::<f32>().unwrap_or(0.0) / 72.0
    } else if s.ends_with("px") {
        s[..s.len() - 2].trim().parse::<f32>().unwrap_or(0.0) * scale
    } else {
        s.parse::<f32>().unwrap_or(0.0) * scale
    }
}

fn parse_svg_color(s: &str, default: egui::Color32) -> egui::Color32 {
    let s = s.trim();
    if s == "none" || s == "transparent" {
        return egui::Color32::TRANSPARENT;
    }
    if s.starts_with('#') {
        let hex = &s[1..];
        if hex.len() == 3 {
            let r = u8::from_str_radix(&hex[0..1], 16).unwrap_or(0) * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).unwrap_or(0) * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).unwrap_or(0) * 17;
            return egui::Color32::from_rgb(r, g, b);
        } else if hex.len() >= 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
            let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
            return egui::Color32::from_rgb(r, g, b);
        }
    } else if s.starts_with("rgb(") && s.ends_with(')') {
        let inner = &s[4..s.len() - 1];
        let parts: Vec<u8> = inner
            .split(',')
            .filter_map(|p| p.trim().parse::<u8>().ok())
            .collect();
        if parts.len() >= 3 {
            return egui::Color32::from_rgb(parts[0], parts[1], parts[2]);
        }
    }
    match s.to_lowercase().as_str() {
        "black" => egui::Color32::BLACK,
        "white" => egui::Color32::WHITE,
        "red" => egui::Color32::from_rgb(255, 0, 0),
        "green" => egui::Color32::from_rgb(0, 180, 0),
        "blue" => egui::Color32::from_rgb(0, 100, 255),
        "yellow" => egui::Color32::from_rgb(255, 230, 0),
        "cyan" => egui::Color32::from_rgb(0, 220, 220),
        "magenta" => egui::Color32::from_rgb(230, 0, 230),
        "gray" | "grey" => egui::Color32::from_gray(128),
        "orange" => egui::Color32::from_rgb(255, 140, 0),
        "purple" => egui::Color32::from_rgb(128, 0, 128),
        _ => default,
    }
}

fn parse_points_list(s: &str, scale: f32) -> Vec<egui::Pos2> {
    let mut numbers = Vec::new();
    let mut current = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E' {
            current.push(c);
        } else if !current.is_empty() {
            if let Ok(num) = current.parse::<f32>() {
                numbers.push(num);
            }
            current.clear();
        }
    }
    if !current.is_empty() {
        if let Ok(num) = current.parse::<f32>() {
            numbers.push(num);
        }
    }

    let mut points = Vec::new();
    for chunk in numbers.chunks_exact(2) {
        points.push(egui::pos2(chunk[0] * scale, chunk[1] * scale));
    }
    points
}

enum PathToken {
    Cmd(char),
    Num(f32),
}

fn tokenize_path_d(d: &str) -> Vec<PathToken> {
    let mut tokens = Vec::new();
    let mut chars = d.chars().peekable();

    while let Some(&c) = chars.peek() {
        if c.is_whitespace() || c == ',' {
            chars.next();
            continue;
        }
        if c.is_ascii_alphabetic() {
            chars.next();
            tokens.push(PathToken::Cmd(c));
            continue;
        }
        if c.is_ascii_digit() || c == '-' || c == '+' || c == '.' {
            let mut num_str = String::new();
            if let Some(&sign) = chars.peek() {
                if sign == '-' || sign == '+' {
                    num_str.push(chars.next().unwrap());
                }
            }
            let mut has_dot = false;
            while let Some(&nc) = chars.peek() {
                if nc.is_ascii_digit() {
                    num_str.push(chars.next().unwrap());
                } else if nc == '.' && !has_dot {
                    has_dot = true;
                    num_str.push(chars.next().unwrap());
                } else if (nc == 'e' || nc == 'E') && !num_str.ends_with('e') && !num_str.ends_with('E') {
                    num_str.push(chars.next().unwrap());
                    if let Some(&sc) = chars.peek() {
                        if sc == '+' || sc == '-' {
                            num_str.push(chars.next().unwrap());
                        }
                    }
                } else {
                    break;
                }
            }
            if let Ok(val) = num_str.parse::<f32>() {
                tokens.push(PathToken::Num(val));
            }
        } else {
            chars.next();
        }
    }
    tokens
}

fn parse_svg_path_d(d: &str, scale: f32) -> Vec<Vec<egui::Pos2>> {
    let tokens = tokenize_path_d(d);
    let mut subpaths: Vec<Vec<egui::Pos2>> = Vec::new();
    let mut current_subpath: Vec<egui::Pos2> = Vec::new();

    let mut cur_x = 0.0_f32;
    let mut cur_y = 0.0_f32;
    let mut start_x = 0.0_f32;
    let mut start_y = 0.0_f32;
    let mut last_cmd = ' ';

    let mut i = 0;
    while i < tokens.len() {
        let cmd = match tokens[i] {
            PathToken::Cmd(c) => {
                i += 1;
                last_cmd = c;
                c
            }
            PathToken::Num(_) => {
                if last_cmd == 'M' {
                    'L'
                } else if last_cmd == 'm' {
                    'l'
                } else {
                    last_cmd
                }
            }
        };

        match cmd {
            'M' | 'm' => {
                let is_rel = cmd == 'm';
                if i + 1 < tokens.len() {
                    if let (PathToken::Num(x), PathToken::Num(y)) = (&tokens[i], &tokens[i + 1]) {
                        if !current_subpath.is_empty() {
                            subpaths.push(std::mem::take(&mut current_subpath));
                        }
                        if is_rel {
                            cur_x += x * scale;
                            cur_y += y * scale;
                        } else {
                            cur_x = x * scale;
                            cur_y = y * scale;
                        }
                        start_x = cur_x;
                        start_y = cur_y;
                        current_subpath.push(egui::pos2(cur_x, cur_y));
                        i += 2;
                    } else { i += 1; }
                } else { i += 1; }
            }
            'L' | 'l' => {
                let is_rel = cmd == 'l';
                if i + 1 < tokens.len() {
                    if let (PathToken::Num(x), PathToken::Num(y)) = (&tokens[i], &tokens[i + 1]) {
                        if is_rel {
                            cur_x += x * scale;
                            cur_y += y * scale;
                        } else {
                            cur_x = x * scale;
                            cur_y = y * scale;
                        }
                        current_subpath.push(egui::pos2(cur_x, cur_y));
                        i += 2;
                    } else { i += 1; }
                } else { i += 1; }
            }
            'H' | 'h' => {
                let is_rel = cmd == 'h';
                if i < tokens.len() {
                    if let PathToken::Num(x) = &tokens[i] {
                        if is_rel { cur_x += x * scale; } else { cur_x = x * scale; }
                        current_subpath.push(egui::pos2(cur_x, cur_y));
                        i += 1;
                    } else { i += 1; }
                } else { i += 1; }
            }
            'V' | 'v' => {
                let is_rel = cmd == 'v';
                if i < tokens.len() {
                    if let PathToken::Num(y) = &tokens[i] {
                        if is_rel { cur_y += y * scale; } else { cur_y = y * scale; }
                        current_subpath.push(egui::pos2(cur_x, cur_y));
                        i += 1;
                    } else { i += 1; }
                } else { i += 1; }
            }
            'C' | 'c' => {
                let is_rel = cmd == 'c';
                if i + 5 < tokens.len() {
                    if let (
                        PathToken::Num(x1), PathToken::Num(y1),
                        PathToken::Num(x2), PathToken::Num(y2),
                        PathToken::Num(x), PathToken::Num(y)
                    ) = (&tokens[i], &tokens[i+1], &tokens[i+2], &tokens[i+3], &tokens[i+4], &tokens[i+5]) {
                        let (p1x, p1y, p2x, p2y, target_x, target_y) = if is_rel {
                            (cur_x + x1*scale, cur_y + y1*scale, cur_x + x2*scale, cur_y + y2*scale, cur_x + x*scale, cur_y + y*scale)
                        } else {
                            (x1*scale, y1*scale, x2*scale, y2*scale, x*scale, y*scale)
                        };
                        let p0 = egui::pos2(cur_x, cur_y);
                        let p1 = egui::pos2(p1x, p1y);
                        let p2 = egui::pos2(p2x, p2y);
                        let p3 = egui::pos2(target_x, target_y);
                        for step in 1..=4 {
                            let t = step as f32 / 4.0;
                            let it = 1.0 - t;
                            let pt = p0 * (it * it * it) + p1.to_vec2() * (3.0 * it * it * t) + p2.to_vec2() * (3.0 * it * t * t) + p3.to_vec2() * (t * t * t);
                            current_subpath.push(pt);
                        }
                        cur_x = target_x;
                        cur_y = target_y;
                        i += 6;
                    } else { i += 1; }
                } else { i += 1; }
            }
            'S' | 's' | 'Q' | 'q' => {
                let is_rel = cmd == 's' || cmd == 'q';
                if i + 3 < tokens.len() {
                    if let (
                        PathToken::Num(x1), PathToken::Num(y1),
                        PathToken::Num(x), PathToken::Num(y)
                    ) = (&tokens[i], &tokens[i+1], &tokens[i+2], &tokens[i+3]) {
                        let (p1x, p1y, target_x, target_y) = if is_rel {
                            (cur_x + x1*scale, cur_y + y1*scale, cur_x + x*scale, cur_y + y*scale)
                        } else {
                            (x1*scale, y1*scale, x*scale, y*scale)
                        };
                        let p0 = egui::pos2(cur_x, cur_y);
                        let p1 = egui::pos2(p1x, p1y);
                        let p2 = egui::pos2(target_x, target_y);
                        for step in 1..=3 {
                            let t = step as f32 / 3.0;
                            let it = 1.0 - t;
                            let pt = p0 * (it * it) + p1.to_vec2() * (2.0 * it * t) + p2.to_vec2() * (t * t);
                            current_subpath.push(pt);
                        }
                        cur_x = target_x;
                        cur_y = target_y;
                        i += 4;
                    } else { i += 1; }
                } else { i += 1; }
            }
            'A' | 'a' => {
                let is_rel = cmd == 'a';
                if i + 6 < tokens.len() {
                    if let (PathToken::Num(_), PathToken::Num(_), PathToken::Num(_), PathToken::Num(_), PathToken::Num(_), PathToken::Num(x), PathToken::Num(y)) =
                        (&tokens[i], &tokens[i+1], &tokens[i+2], &tokens[i+3], &tokens[i+4], &tokens[i+5], &tokens[i+6]) {
                        if is_rel {
                            cur_x += x * scale;
                            cur_y += y * scale;
                        } else {
                            cur_x = x * scale;
                            cur_y = y * scale;
                        }
                        current_subpath.push(egui::pos2(cur_x, cur_y));
                        i += 7;
                    } else { i += 1; }
                } else { i += 1; }
            }
            'Z' | 'z' => {
                if (cur_x - start_x).abs() > 0.001 || (cur_y - start_y).abs() > 0.001 {
                    current_subpath.push(egui::pos2(start_x, start_y));
                }
                cur_x = start_x;
                cur_y = start_y;
            }
            _ => {
                i += 1;
            }
        }
    }

    if !current_subpath.is_empty() {
        subpaths.push(current_subpath);
    }

    subpaths
}

impl eframe::App for StudioApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.screenshot_path.is_some() {
            ctx.request_repaint();
            self.frames_rendered += 1;
            if self.frames_rendered == 60 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            }
            if self.frames_rendered > 120 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        ctx.input(|i| {
            for event in &i.events {
                if let egui::Event::Screenshot { image, .. } = event {
                    if let Some(path) = &self.screenshot_path {
                        let _ = save_png(path, image);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
        });

        // Keyboard shortcuts
        if !ctx.wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
                if let Some(idx) = self.selected_index {
                    if idx < self.shapes.len() {
                        self.shapes.remove(idx);
                        self.selected_index = None;
                    }
                }
            }
        }

        // Top menu
        egui::TopBottomPanel::top("top_menu").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New Design").clicked() {
                        self.shapes.clear();
                        self.selected_index = None;
                        self.pan = egui::Vec2::ZERO;
                        self.zoom = 1.0;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Open Project (.json)...").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Silhouette Project (*.json)", &["json"])
                            .pick_file()
                        {
                            match self.open_project_file(&path) {
                                Ok(()) => self.set_status("Project loaded successfully"),
                                Err(e) => self.set_status(format!("Error loading project: {}", e)),
                            }
                        }
                        ui.close_menu();
                    }
                    if ui.button("Save Project (.json)...").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Silhouette Project (*.json)", &["json"])
                            .set_file_name("project.json")
                            .save_file()
                        {
                            match self.save_project_file(&path) {
                                Ok(()) => self.set_status("Project saved successfully"),
                                Err(e) => self.set_status(format!("Error saving project: {}", e)),
                            }
                        }
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Import SVG...").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Scalable Vector Graphics (*.svg)", &["svg"])
                            .pick_file()
                        {
                            match self.import_svg_file(&path) {
                                Ok(n) => self.set_status(format!("Imported {} shapes from SVG", n)),
                                Err(e) => self.set_status(format!("Error importing SVG: {}", e)),
                            }
                        }
                        ui.close_menu();
                    }
                    if ui.button("Export SVG...").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Scalable Vector Graphics (*.svg)", &["svg"])
                            .set_file_name("design.svg")
                            .save_file()
                        {
                            match self.export_svg_file(&path) {
                                Ok(()) => self.set_status("SVG exported successfully"),
                                Err(e) => self.set_status(format!("Error exporting SVG: {}", e)),
                            }
                        }
                        ui.close_menu();
                    }
                    if ui.button("Export PNG (High-DPI)...").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("PNG Image (*.png)", &["png"])
                            .set_file_name("mat_design.png")
                            .save_file()
                        {
                            match self.export_png_file(&path, 150.0) {
                                Ok(()) => self.set_status("High-DPI PNG exported successfully"),
                                Err(e) => self.set_status(format!("Error exporting PNG: {}", e)),
                            }
                        }
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Edit", |ui| {
                    if ui.button("Reset View").clicked() {
                        self.zoom = 1.0;
                        self.pan = egui::Vec2::ZERO;
                        ui.close_menu();
                    }
                    let has_sel = self.selected_index.is_some();
                    if ui.add_enabled(has_sel, egui::Button::new("Delete Selected")).clicked() {
                        if let Some(idx) = self.selected_index {
                            if idx < self.shapes.len() {
                                self.shapes.remove(idx);
                                self.selected_index = None;
                            }
                        }
                        ui.close_menu();
                    }
                    if ui.button("Clear All Shapes").clicked() {
                        self.shapes.clear();
                        self.selected_index = None;
                        ui.close_menu();
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui.button("Zoom In").clicked() {
                        self.zoom = (self.zoom * 1.2).min(5.0);
                        ui.close_menu();
                    }
                    if ui.button("Zoom Out").clicked() {
                        self.zoom = (self.zoom / 1.2).max(0.25);
                        ui.close_menu();
                    }
                });
                ui.separator();
                ui.label(format!("Zoom: {:.0}%", self.zoom * 100.0));
                ui.separator();
                ui.label(format!("Shapes: {}", self.shapes.len()));

                if let Some((msg, created)) = &self.status_message {
                    if created.elapsed() < std::time::Duration::from_secs(5) {
                        ui.separator();
                        ui.label(egui::RichText::new(msg).color(egui::Color32::from_rgb(0, 180, 240)));
                    }
                }
            });
        });

        // Left tool panel
        egui::SidePanel::left("left_tools").default_width(110.0).show(ctx, |ui| {
            ui.heading("Tools");
            ui.separator();
            if ui.selectable_value(&mut self.active_tool, Tool::Select, "Select").clicked() {
                self.drag_start = None;
                self.freehand_points.clear();
            }
            if ui.selectable_value(&mut self.active_tool, Tool::Line, "Line").clicked() {
                self.drag_start = None;
                self.freehand_points.clear();
            }
            if ui.selectable_value(&mut self.active_tool, Tool::Rectangle, "Rectangle").clicked() {
                self.drag_start = None;
                self.freehand_points.clear();
            }
            if ui.selectable_value(&mut self.active_tool, Tool::Ellipse, "Ellipse").clicked() {
                self.drag_start = None;
                self.freehand_points.clear();
            }
            if ui.selectable_value(&mut self.active_tool, Tool::Text, "Text").clicked() {
                self.drag_start = None;
                self.freehand_points.clear();
            }
            if ui.selectable_value(&mut self.active_tool, Tool::Freehand, "Freehand").clicked() {
                self.drag_start = None;
                self.freehand_points.clear();
            }
        });

        // Right inspector panel
        egui::SidePanel::right("right_panels").default_width(240.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for tab in ["Page Setup", "Fill", "Text", "Transform"] {
                    if ui.selectable_label(self.right_panel == tab, tab).clicked() {
                        self.right_panel = tab;
                    }
                }
            });
            ui.separator();
            match self.right_panel {
                "Page Setup" => {
                    ui.heading("Page Setup");
                    ui.add(egui::Slider::new(&mut self.mat_width_in, 4.0..=24.0).text("Width (in)"));
                    ui.add(egui::Slider::new(&mut self.mat_height_in, 4.0..=24.0).text("Height (in)"));
                }
                "Fill" => {
                    ui.heading("Color Fill");
                    if ui.color_edit_button_rgb(&mut self.fill_color).changed() {
                        let new_color = self.current_color32();
                        if let Some(idx) = self.selected_index {
                            if let Some(shape) = self.shapes.get_mut(idx) {
                                shape.fill_color = new_color;
                            }
                        }
                    }
                }
                "Text" => {
                    ui.heading("Text Style");
                    if ui.text_edit_singleline(&mut self.text_sample).changed() {
                        let text_val = self.text_sample.clone();
                        if let Some(idx) = self.selected_index {
                            if let Some(shape) = self.shapes.get_mut(idx) {
                                if shape.kind == ShapeKind::Text {
                                    shape.text = text_val;
                                }
                            }
                        }
                    }
                    ui.add_space(8.0);
                    ui.label("Click on the mat with the Text tool to place.");
                }
                _ => {
                    ui.heading("Transform");
                    if let Some(idx) = self.selected_index {
                        if idx < self.shapes.len() {
                            let mut delete_clicked = false;
                            {
                                let shape = &mut self.shapes[idx];
                                let kind_str = match shape.kind {
                                    ShapeKind::Rectangle => "Rectangle",
                                    ShapeKind::Ellipse => "Ellipse",
                                    ShapeKind::Line => "Line",
                                    ShapeKind::Freehand => "Freehand",
                                    ShapeKind::Text => "Text",
                                };
                                ui.label(format!("Selected: {}", kind_str));
                                ui.separator();

                                let mut x = shape.x;
                                let mut y = shape.y;
                                let mut w = shape.width;
                                let mut h = shape.height;

                                ui.horizontal(|ui| {
                                    ui.label("X:     ");
                                    if ui.add(egui::DragValue::new(&mut x).speed(0.05).suffix(" in")).changed() {
                                        shape.set_x(x);
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Y:     ");
                                    if ui.add(egui::DragValue::new(&mut y).speed(0.05).suffix(" in")).changed() {
                                        shape.set_y(y);
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Width: ");
                                    if ui.add(egui::DragValue::new(&mut w).speed(0.05).clamp_range(0.05..=100.0).suffix(" in")).changed() {
                                        shape.set_width(w);
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Height:");
                                    if ui.add(egui::DragValue::new(&mut h).speed(0.05).clamp_range(0.05..=100.0).suffix(" in")).changed() {
                                        shape.set_height(h);
                                    }
                                });

                                ui.add_space(10.0);
                                if ui.button("🗑 Delete Shape").clicked() {
                                    delete_clicked = true;
                                }
                            }
                            if delete_clicked {
                                self.shapes.remove(idx);
                                self.selected_index = None;
                            }
                        } else {
                            self.selected_index = None;
                            ui.label("Select an object on the mat to transform.");
                        }
                    } else {
                        ui.label("Select an object on the mat to transform.");
                    }
                }
            }
        });

        // Cutting Mat canvas
        egui::CentralPanel::default().show(ctx, |ui| {
            let (response, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
            let rect = response.rect;

            // 1. Panning with middle-mouse or right-mouse drag
            if response.dragged_by(egui::PointerButton::Middle) || response.dragged_by(egui::PointerButton::Secondary) {
                self.pan += response.drag_delta();
            }

            // Zooming with scroll wheel
            if response.hovered() {
                let scroll = ctx.input(|i| i.raw_scroll_delta.y);
                if scroll != 0.0 {
                    self.zoom = (self.zoom * (1.0 + scroll * 0.001)).clamp(0.25, 5.0);
                }
            }

            // Set cursor icon based on active tool
            if response.hovered() {
                match self.active_tool {
                    Tool::Select => {
                        if self.is_dragging_shape {
                            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                        } else {
                            ctx.set_cursor_icon(egui::CursorIcon::Default);
                        }
                    }
                    Tool::Rectangle | Tool::Ellipse | Tool::Line | Tool::Freehand => {
                        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                    }
                    Tool::Text => {
                        ctx.set_cursor_icon(egui::CursorIcon::Text);
                    }
                }
            }

            // Background canvas fill
            painter.rect_filled(rect, 0.0, egui::Color32::from_gray(42));

            let px_per_in = 40.0 * self.zoom;
            let mat_size = egui::vec2(self.mat_width_in * px_per_in, self.mat_height_in * px_per_in);
            let mat_min = rect.center() - mat_size * 0.5 + self.pan;
            let mat_rect = egui::Rect::from_min_size(mat_min, mat_size);

            let screen_to_mat = |pos: egui::Pos2| -> egui::Pos2 {
                egui::pos2(
                    (pos.x - mat_rect.min.x) / px_per_in,
                    (pos.y - mat_rect.min.y) / px_per_in,
                )
            };

            let mat_to_screen = |pos: egui::Pos2| -> egui::Pos2 {
                egui::pos2(
                    mat_rect.min.x + pos.x * px_per_in,
                    mat_rect.min.y + pos.y * px_per_in,
                )
            };

            // Mat background and grid lines
            painter.rect_filled(mat_rect, 2.0, egui::Color32::WHITE);
            painter.rect_stroke(mat_rect, 2.0_f32, egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(0, 150, 214)));
            let grid_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_gray(225));
            for c in 1..(self.mat_width_in.ceil() as i32) {
                let x = mat_rect.min.x + (c as f32) * px_per_in;
                painter.line_segment([egui::pos2(x, mat_rect.min.y), egui::pos2(x, mat_rect.max.y)], grid_stroke);
            }
            for r in 1..(self.mat_height_in.ceil() as i32) {
                let y = mat_rect.min.y + (r as f32) * px_per_in;
                painter.line_segment([egui::pos2(mat_rect.min.x, y), egui::pos2(mat_rect.max.x, y)], grid_stroke);
            }

            // Left-mouse tool interactions
            let hit_tol = (10.0 / px_per_in).max(0.2);
            let current_fill = self.current_color32();

            // Drag started
            if response.drag_started_by(egui::PointerButton::Primary) {
                let start_pos = response.interact_pointer_pos().or_else(|| ctx.input(|i| i.pointer.press_origin()));
                if let Some(pos) = start_pos {
                    let mat_pos = screen_to_mat(pos);
                    match self.active_tool {
                        Tool::Select => {
                            let hit = self.shapes.iter().enumerate().rev().find(|(_, s)| {
                                s.hit_test(mat_pos, hit_tol)
                            }).map(|(i, _)| i);
                            self.selected_index = hit;
                            self.is_dragging_shape = hit.is_some();
                            if let Some(idx) = hit {
                                let s = &self.shapes[idx];
                                self.fill_color = [
                                    s.fill_color.r() as f32 / 255.0,
                                    s.fill_color.g() as f32 / 255.0,
                                    s.fill_color.b() as f32 / 255.0,
                                ];
                                if s.kind == ShapeKind::Text && !s.text.is_empty() {
                                    self.text_sample = s.text.clone();
                                }
                            }
                        }
                        Tool::Rectangle | Tool::Ellipse | Tool::Line => {
                            self.drag_start = Some(mat_pos);
                        }
                        Tool::Freehand => {
                            self.freehand_points = vec![mat_pos];
                        }
                        Tool::Text => {}
                    }
                }
            }

            // Dragging
            if response.dragged_by(egui::PointerButton::Primary) {
                match self.active_tool {
                    Tool::Select => {
                        if self.is_dragging_shape {
                            if let Some(idx) = self.selected_index {
                                if let Some(shape) = self.shapes.get_mut(idx) {
                                    let delta_in = response.drag_delta() / px_per_in;
                                    shape.translate(delta_in);
                                }
                            }
                        }
                    }
                    Tool::Rectangle | Tool::Ellipse | Tool::Line => {
                        if self.drag_start.is_none() {
                            let start_pos = ctx.input(|i| i.pointer.press_origin().or_else(|| i.pointer.latest_pos()));
                            if let Some(pos) = start_pos {
                                self.drag_start = Some(screen_to_mat(pos));
                            }
                        }
                    }
                    Tool::Freehand => {
                        let curr_pos = response.interact_pointer_pos().or_else(|| ctx.input(|i| i.pointer.latest_pos()));
                        if let Some(pos) = curr_pos {
                            let mat_pos = screen_to_mat(pos);
                            if self.freehand_points.is_empty() {
                                self.freehand_points.push(mat_pos);
                            } else if let Some(&last) = self.freehand_points.last() {
                                if (mat_pos - last).length() > (2.0 / px_per_in).max(0.03) {
                                    self.freehand_points.push(mat_pos);
                                }
                            }
                        }
                    }
                    Tool::Text => {}
                }
            }

            // Click handling
            if response.clicked() {
                let click_pos = response.interact_pointer_pos().or_else(|| ctx.input(|i| i.pointer.latest_pos()));
                if let Some(pos) = click_pos {
                    let mat_pos = screen_to_mat(pos);
                    match self.active_tool {
                        Tool::Select => {
                            let hit = self.shapes.iter().enumerate().rev().find(|(_, s)| {
                                s.hit_test(mat_pos, hit_tol)
                            }).map(|(i, _)| i);
                            self.selected_index = hit;
                            if let Some(idx) = hit {
                                let s = &self.shapes[idx];
                                self.fill_color = [
                                    s.fill_color.r() as f32 / 255.0,
                                    s.fill_color.g() as f32 / 255.0,
                                    s.fill_color.b() as f32 / 255.0,
                                ];
                                if s.kind == ShapeKind::Text && !s.text.is_empty() {
                                    self.text_sample = s.text.clone();
                                }
                            }
                        }
                        Tool::Text => {
                            let text = if self.text_sample.is_empty() { "Sample Design".to_string() } else { self.text_sample.clone() };
                            let initial_w = (text.len() as f32 * 0.35).max(0.8);
                            let initial_h = 0.6;
                            self.shapes.push(CanvasShape {
                                kind: ShapeKind::Text,
                                x: mat_pos.x,
                                y: mat_pos.y,
                                width: initial_w,
                                height: initial_h,
                                points: vec![],
                                text,
                                fill_color: current_fill,
                            });
                            self.selected_index = Some(self.shapes.len() - 1);
                        }
                        _ => {}
                    }
                }
            }

            // Mouse release / drag stopped
            let primary_released = response.drag_stopped_by(egui::PointerButton::Primary)
                || ctx.input(|i| i.pointer.primary_released());

            if primary_released {
                self.is_dragging_shape = false;
                let curr_pos = response.interact_pointer_pos().or_else(|| ctx.input(|i| i.pointer.latest_pos()));
                let curr_mat = curr_pos.map(screen_to_mat);

                if let Some(start) = self.drag_start.take() {
                    let end = curr_mat.unwrap_or(start);
                    match self.active_tool {
                        Tool::Rectangle => {
                            let min_x = start.x.min(end.x);
                            let min_y = start.y.min(end.y);
                            let w = (start.x - end.x).abs();
                            let h = (start.y - end.y).abs();
                            if w > 0.05 || h > 0.05 {
                                self.shapes.push(CanvasShape {
                                    kind: ShapeKind::Rectangle,
                                    x: min_x,
                                    y: min_y,
                                    width: w.max(0.05),
                                    height: h.max(0.05),
                                    points: vec![],
                                    text: String::new(),
                                    fill_color: current_fill,
                                });
                                self.selected_index = Some(self.shapes.len() - 1);
                            }
                        }
                        Tool::Ellipse => {
                            let min_x = start.x.min(end.x);
                            let min_y = start.y.min(end.y);
                            let w = (start.x - end.x).abs();
                            let h = (start.y - end.y).abs();
                            if w > 0.05 || h > 0.05 {
                                self.shapes.push(CanvasShape {
                                    kind: ShapeKind::Ellipse,
                                    x: min_x,
                                    y: min_y,
                                    width: w.max(0.05),
                                    height: h.max(0.05),
                                    points: vec![],
                                    text: String::new(),
                                    fill_color: current_fill,
                                });
                                self.selected_index = Some(self.shapes.len() - 1);
                            }
                        }
                        Tool::Line => {
                            let dist = (start - end).length();
                            if dist > 0.05 {
                                let min_x = start.x.min(end.x);
                                let min_y = start.y.min(end.y);
                                let w = (start.x - end.x).abs().max(0.01);
                                let h = (start.y - end.y).abs().max(0.01);
                                self.shapes.push(CanvasShape {
                                    kind: ShapeKind::Line,
                                    x: min_x,
                                    y: min_y,
                                    width: w,
                                    height: h,
                                    points: vec![start, end],
                                    text: String::new(),
                                    fill_color: current_fill,
                                });
                                self.selected_index = Some(self.shapes.len() - 1);
                            }
                        }
                        _ => {}
                    }
                }

                if !self.freehand_points.is_empty() {
                    if self.freehand_points.len() >= 2 {
                        let mut min_x = self.freehand_points[0].x;
                        let mut min_y = self.freehand_points[0].y;
                        let mut max_x = self.freehand_points[0].x;
                        let mut max_y = self.freehand_points[0].y;
                        for p in &self.freehand_points {
                            min_x = min_x.min(p.x);
                            min_y = min_y.min(p.y);
                            max_x = max_x.max(p.x);
                            max_y = max_y.max(p.y);
                        }
                        self.shapes.push(CanvasShape {
                            kind: ShapeKind::Freehand,
                            x: min_x,
                            y: min_y,
                            width: (max_x - min_x).max(0.05),
                            height: (max_y - min_y).max(0.05),
                            points: std::mem::take(&mut self.freehand_points),
                            text: String::new(),
                            fill_color: current_fill,
                        });
                        self.selected_index = Some(self.shapes.len() - 1);
                    } else {
                        self.freehand_points.clear();
                    }
                }
            }

            // Render shapes over the grid using fill_color and 2px black stroke
            let black_stroke = egui::Stroke::new(2.0_f32, egui::Color32::BLACK);
            for shape in &mut self.shapes {
                match shape.kind {
                    ShapeKind::Rectangle => {
                        let screen_r = egui::Rect::from_min_size(
                            mat_to_screen(egui::pos2(shape.x, shape.y)),
                            egui::vec2(shape.width * px_per_in, shape.height * px_per_in),
                        );
                        painter.rect_filled(screen_r, 0.0, shape.fill_color);
                        painter.rect_stroke(screen_r, 0.0, black_stroke);
                    }
                    ShapeKind::Ellipse => {
                        let screen_min = mat_to_screen(egui::pos2(shape.x, shape.y));
                        let screen_size = egui::vec2(shape.width * px_per_in, shape.height * px_per_in);
                        let center = screen_min + screen_size * 0.5;
                        let rx = (screen_size.x * 0.5).abs();
                        let ry = (screen_size.y * 0.5).abs();
                        if rx > 0.5 && ry > 0.5 {
                            let n = 48;
                            let pts: Vec<egui::Pos2> = (0..n).map(|i| {
                                let angle = (i as f32) / (n as f32) * std::f32::consts::TAU;
                                egui::pos2(center.x + rx * angle.cos(), center.y + ry * angle.sin())
                            }).collect();
                            painter.add(egui::Shape::convex_polygon(
                                pts,
                                shape.fill_color,
                                black_stroke,
                            ));
                        }
                    }
                    ShapeKind::Line => {
                        if shape.points.len() >= 2 {
                            let p1 = mat_to_screen(shape.points[0]);
                            let p2 = mat_to_screen(shape.points[1]);
                            painter.line_segment([p1, p2], black_stroke);
                        }
                    }
                    ShapeKind::Freehand => {
                        if shape.points.len() >= 2 {
                            let screen_pts: Vec<egui::Pos2> = shape.points.iter().map(|&p| mat_to_screen(p)).collect();
                            painter.add(egui::Shape::line(
                                screen_pts,
                                black_stroke,
                            ));
                        } else if let Some(&p) = shape.points.first() {
                            let sp = mat_to_screen(p);
                            painter.circle_filled(sp, 2.0_f32, egui::Color32::BLACK);
                        }
                    }
                    ShapeKind::Text => {
                        let screen_pos = mat_to_screen(egui::pos2(shape.x, shape.y));
                        let font_size = (shape.height * px_per_in).max(12.0);
                        let text_rect = painter.text(
                            screen_pos,
                            egui::Align2::LEFT_TOP,
                            &shape.text,
                            egui::FontId::proportional(font_size),
                            shape.fill_color,
                        );
                        shape.width = (text_rect.width() / px_per_in).max(0.2);
                    }
                }
            }

            // Live preview while drawing
            if let Some(start) = self.drag_start {
                let curr_pos = response.interact_pointer_pos().or_else(|| ctx.input(|i| i.pointer.latest_pos()));
                if let Some(pos) = curr_pos {
                    let curr = screen_to_mat(pos);
                    match self.active_tool {
                        Tool::Rectangle => {
                            let p1 = egui::pos2(start.x.min(curr.x), start.y.min(curr.y));
                            let p2 = egui::pos2(start.x.max(curr.x), start.y.max(curr.y));
                            let s_r = egui::Rect::from_two_pos(mat_to_screen(p1), mat_to_screen(p2));
                            painter.rect_filled(s_r, 0.0, current_fill);
                            painter.rect_stroke(s_r, 0.0, black_stroke);
                        }
                        Tool::Ellipse => {
                            let p1 = egui::pos2(start.x.min(curr.x), start.y.min(curr.y));
                            let p2 = egui::pos2(start.x.max(curr.x), start.y.max(curr.y));
                            let s_min = mat_to_screen(p1);
                            let s_max = mat_to_screen(p2);
                            let center = egui::pos2((s_min.x + s_max.x) * 0.5, (s_min.y + s_max.y) * 0.5);
                            let rx = ((s_max.x - s_min.x) * 0.5).abs();
                            let ry = ((s_max.y - s_min.y) * 0.5).abs();
                            if rx > 1.0 && ry > 1.0 {
                                let n = 48;
                                let pts: Vec<egui::Pos2> = (0..n).map(|i| {
                                    let angle = (i as f32) / (n as f32) * std::f32::consts::TAU;
                                    egui::pos2(center.x + rx * angle.cos(), center.y + ry * angle.sin())
                                }).collect();
                                painter.add(egui::Shape::convex_polygon(
                                    pts,
                                    current_fill,
                                    black_stroke,
                                ));
                            }
                        }
                        Tool::Line => {
                            let s1 = mat_to_screen(start);
                            let s2 = mat_to_screen(curr);
                            painter.line_segment([s1, s2], black_stroke);
                        }
                        _ => {}
                    }
                }
            }

            if !self.freehand_points.is_empty() {
                let screen_pts: Vec<egui::Pos2> = self.freehand_points.iter().map(|&p| mat_to_screen(p)).collect();
                if screen_pts.len() >= 2 {
                    painter.add(egui::Shape::line(
                        screen_pts,
                        black_stroke,
                    ));
                }
            }

            // Dashed bounding box for selected shape
            if let Some(idx) = self.selected_index {
                if let Some(shape) = self.shapes.get(idx) {
                    let screen_min = mat_to_screen(egui::pos2(shape.x, shape.y));
                    let screen_size = egui::vec2(shape.width * px_per_in, shape.height * px_per_in);
                    let bounding_rect = egui::Rect::from_min_size(screen_min, screen_size).expand(3.0);
                    let stroke = egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(0, 120, 215));
                    draw_dashed_rect(&painter, bounding_rect, stroke, 6.0, 4.0);

                    let handle_size = 6.0;
                    for pt in [
                        bounding_rect.left_top(),
                        bounding_rect.right_top(),
                        bounding_rect.left_bottom(),
                        bounding_rect.right_bottom(),
                    ] {
                        let hr = egui::Rect::from_center_size(pt, egui::vec2(handle_size, handle_size));
                        painter.rect_filled(hr, 0.0, egui::Color32::WHITE);
                        painter.rect_stroke(hr, 0.0, egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(0, 120, 215)));
                    }
                }
            }
        });
    }
}

fn save_png(path: &str, image: &egui::ColorImage) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(path)?;
    let w = std::io::BufWriter::new(file);
    let mut encoder = png::Encoder::new(w, image.width() as u32, image.height() as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    let raw: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
    writer.write_image_data(&raw)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_open_project() {
        let mut app = StudioApp::new(None);
        app.mat_width_in = 15.0;
        app.mat_height_in = 15.0;
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Rectangle,
            x: 1.0,
            y: 2.0,
            width: 3.0,
            height: 4.0,
            points: vec![],
            text: String::new(),
            fill_color: egui::Color32::RED,
        });
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Line,
            x: 0.0,
            y: 0.0,
            width: 5.0,
            height: 5.0,
            points: vec![egui::pos2(0.0, 0.0), egui::pos2(5.0, 5.0)],
            text: String::new(),
            fill_color: egui::Color32::BLACK,
        });

        let tmp_path = std::env::temp_dir().join("test_silhouette_project.json");
        app.save_project_file(&tmp_path).expect("Failed to save project");

        let mut app2 = StudioApp::new(None);
        app2.open_project_file(&tmp_path).expect("Failed to open project");
        let _ = std::fs::remove_file(&tmp_path);

        assert_eq!(app2.mat_width_in, 15.0);
        assert_eq!(app2.mat_height_in, 15.0);
        assert_eq!(app2.shapes.len(), 2);
        assert_eq!(app2.shapes[0].kind, ShapeKind::Rectangle);
        assert_eq!(app2.shapes[0].width, 3.0);
        assert_eq!(app2.shapes[1].points.len(), 2);
    }

    #[test]
    fn test_export_svg() {
        let mut app = StudioApp::new(None);
        app.mat_width_in = 12.0;
        app.mat_height_in = 12.0;
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Rectangle,
            x: 1.0,
            y: 1.0,
            width: 2.0,
            height: 2.0,
            points: vec![],
            text: String::new(),
            fill_color: egui::Color32::RED,
        });
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Ellipse,
            x: 4.0,
            y: 4.0,
            width: 2.0,
            height: 3.0,
            points: vec![],
            text: String::new(),
            fill_color: egui::Color32::BLUE,
        });
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Line,
            x: 1.0,
            y: 5.0,
            width: 3.0,
            height: 0.0,
            points: vec![egui::pos2(1.0, 5.0), egui::pos2(4.0, 5.0)],
            text: String::new(),
            fill_color: egui::Color32::BLACK,
        });
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Freehand,
            x: 2.0,
            y: 6.0,
            width: 2.0,
            height: 2.0,
            points: vec![egui::pos2(2.0, 6.0), egui::pos2(3.0, 7.0), egui::pos2(4.0, 6.0)],
            text: String::new(),
            fill_color: egui::Color32::BLACK,
        });
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Text,
            x: 1.0,
            y: 8.0,
            width: 4.0,
            height: 0.8,
            points: vec![],
            text: "Hello Silhouette".to_string(),
            fill_color: egui::Color32::BLACK,
        });

        let svg = app.export_svg_string();
        assert!(svg.contains("width=\"12.00in\""));
        assert!(svg.contains("height=\"12.00in\""));
        assert!(svg.contains("viewBox=\"0 0 12.00 12.00\""));
        assert!(svg.contains("<rect "));
        assert!(svg.contains("<ellipse "));
        assert!(svg.contains("<line "));
        assert!(svg.contains("<polyline "));
        assert!(svg.contains("<text "));
        assert!(svg.contains("Hello Silhouette"));
    }

    #[test]
    fn test_import_svg() {
        let svg_data = r##"<svg xmlns="http://www.w3.org/2000/svg" width="12in" height="12in" viewBox="0 0 12 12">
            <rect x="1" y="2" width="3" height="4" fill="#ff0000" />
            <circle cx="5" cy="5" r="2" fill="#00ff00" />
            <ellipse cx="8" cy="8" rx="2" ry="1" fill="#0000ff" />
            <line x1="1" y1="1" x2="4" y2="4" stroke="black" />
            <polyline points="2,2 3,3 4,2" fill="none" />
            <polygon points="5,5 6,6 5,7" fill="yellow" />
            <path d="M 1 1 L 2 3 L 4 1 Z" fill="cyan" />
        </svg>"##;

        let shapes = import_svg_str(svg_data, egui::Color32::GRAY);
        assert_eq!(shapes.len(), 7);
        assert_eq!(shapes[0].kind, ShapeKind::Rectangle);
        assert_eq!(shapes[1].kind, ShapeKind::Ellipse); // circle -> Ellipse
        assert_eq!(shapes[2].kind, ShapeKind::Ellipse);
        assert_eq!(shapes[3].kind, ShapeKind::Line);
        assert_eq!(shapes[4].kind, ShapeKind::Freehand); // polyline
        assert_eq!(shapes[5].kind, ShapeKind::Freehand); // polygon
        assert_eq!(shapes[6].kind, ShapeKind::Freehand); // path
    }

    #[test]
    fn test_export_png() {
        let mut app = StudioApp::new(None);
        app.mat_width_in = 6.0;
        app.mat_height_in = 6.0;
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Rectangle,
            x: 1.0,
            y: 1.0,
            width: 2.0,
            height: 2.0,
            points: vec![],
            text: String::new(),
            fill_color: egui::Color32::from_rgb(200, 50, 50),
        });

        let tmp_png = std::env::temp_dir().join("test_export_mat.png");
        app.export_png_file(&tmp_png, 50.0).expect("Failed to export PNG");
        assert!(tmp_png.exists());
        let meta = std::fs::metadata(&tmp_png).unwrap();
        assert!(meta.len() > 100);
        let _ = std::fs::remove_file(&tmp_png);
    }

    #[test]
    fn test_svg_roundtrip() {
        let mut app = StudioApp::new(None);
        app.mat_width_in = 12.0;
        app.mat_height_in = 12.0;
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Rectangle,
            x: 2.0,
            y: 3.0,
            width: 4.0,
            height: 5.0,
            points: vec![],
            text: String::new(),
            fill_color: egui::Color32::from_rgb(255, 0, 0),
        });
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Ellipse,
            x: 6.0,
            y: 7.0,
            width: 3.0,
            height: 2.0,
            points: vec![],
            text: String::new(),
            fill_color: egui::Color32::from_rgb(0, 255, 0),
        });
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Line,
            x: 1.0,
            y: 1.0,
            width: 2.0,
            height: 2.0,
            points: vec![egui::pos2(1.0, 1.0), egui::pos2(3.0, 3.0)],
            text: String::new(),
            fill_color: egui::Color32::BLACK,
        });
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Freehand,
            x: 0.0,
            y: 0.0,
            width: 2.0,
            height: 2.0,
            points: vec![egui::pos2(0.0, 0.0), egui::pos2(1.0, 2.0), egui::pos2(2.0, 1.0)],
            text: String::new(),
            fill_color: egui::Color32::BLACK,
        });

        let svg_str = app.export_svg_string();
        let imported = import_svg_str(&svg_str, egui::Color32::GRAY);
        assert_eq!(imported.len(), 4);
        assert_eq!(imported[0].kind, ShapeKind::Rectangle);
        assert!((imported[0].x - 2.0).abs() < 0.01);
        assert!((imported[0].y - 3.0).abs() < 0.01);
        assert!((imported[0].width - 4.0).abs() < 0.01);
        assert!((imported[0].height - 5.0).abs() < 0.01);

        assert_eq!(imported[1].kind, ShapeKind::Ellipse);
        assert!((imported[1].x - 6.0).abs() < 0.01);
        assert!((imported[1].y - 7.0).abs() < 0.01);
        assert!((imported[1].width - 3.0).abs() < 0.01);
        assert!((imported[1].height - 2.0).abs() < 0.01);

        assert_eq!(imported[2].kind, ShapeKind::Line);
        assert_eq!(imported[3].kind, ShapeKind::Freehand);
    }
}
