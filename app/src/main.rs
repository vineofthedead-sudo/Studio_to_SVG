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
    pub stroke_color: Option<egui::Color32>,
    pub stroke_width: f32,       // mat-inch stroke width
    pub is_closed: bool,
    pub is_svg_path: bool,
    pub is_modified: bool,
    pub triangles: Vec<[usize; 3]>,
    pub group_id: Option<u64>,
    pub is_visible: bool,
    pub is_locked: bool,
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
    #[serde(default)]
    pub stroke_color: Option<[u8; 4]>,
    #[serde(default)]
    pub stroke_width: Option<f32>,
    #[serde(default)]
    pub is_closed: bool,
    #[serde(default)]
    pub is_svg_path: bool,
    #[serde(default)]
    pub is_modified: bool,
    #[serde(default)]
    pub group_id: Option<u64>,
    #[serde(default = "default_true")]
    pub is_visible: bool,
    #[serde(default)]
    pub is_locked: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ProjectData {
    pub mat_width_in: f32,
    pub mat_height_in: f32,
    pub shapes: Vec<SerializableShape>,
}

impl CanvasShape {
    pub fn precompute_triangles(&mut self) {
        if self.kind == ShapeKind::Freehand && self.is_closed && self.points.len() >= 3 && self.fill_color.a() > 0 {
            self.triangles = triangulate_polygon(&self.points);
        } else {
            self.triangles.clear();
        }
    }

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
            stroke_color: self.stroke_color.map(|c| [c.r(), c.g(), c.b(), c.a()]),
            stroke_width: Some(self.stroke_width),
            is_closed: self.is_closed,
            is_svg_path: self.is_svg_path,
            is_modified: self.is_modified,
            group_id: self.group_id,
            is_visible: self.is_visible,
            is_locked: self.is_locked,
        }
    }

    pub fn from_serializable(s: SerializableShape) -> Self {
        let mut shape = CanvasShape {
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
            stroke_color: s.stroke_color.map(|c| egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]))
                .or_else(|| match s.kind {
                    ShapeKind::Text => None,
                    _ => Some(egui::Color32::BLACK),
                }),
            stroke_width: s.stroke_width.unwrap_or(0.05),
            is_closed: s.is_closed,
            is_svg_path: s.is_svg_path,
            is_modified: s.is_modified,
            triangles: Vec::new(),
            group_id: s.group_id,
            is_visible: s.is_visible,
            is_locked: s.is_locked,
        };
        shape.precompute_triangles();
        shape
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
        self.is_modified = true;
        self.precompute_triangles();
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
        self.is_modified = true;
        self.precompute_triangles();
    }

    pub fn translate(&mut self, delta: egui::Vec2) {
        self.x += delta.x;
        self.y += delta.y;
        for p in &mut self.points {
            *p += delta;
        }
        self.is_modified = true;
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
                let r = egui::Rect::from_min_size(
                    egui::pos2(self.x, self.y),
                    egui::vec2(self.width, self.height),
                );
                if !r.expand(tolerance).contains(p) {
                    return false;
                }
                if self.is_closed && self.fill_color.a() > 0 && point_in_poly(p, &self.points) {
                    return true;
                }
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

pub struct CachedSvg {
    pub group_id: u64,
    pub init_min_x: f32,
    pub init_min_y: f32,
    pub init_width: f32,
    pub init_height: f32,
    pub orig_svg_width: f32,
    pub orig_svg_height: f32,
    pub orig_shape_count: usize,
    pub pixmap: tiny_skia::Pixmap,
    pub texture: Option<egui::TextureHandle>,
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
    pub cached_svgs: Vec<CachedSvg>,
    pub selected_index: Option<usize>,
    pub selected_indices: std::collections::BTreeSet<usize>,
    pub expanded_groups: std::collections::BTreeSet<u64>,
    pub next_group_id: u64,
    pub drag_start: Option<egui::Pos2>,       // in mat inches
    pub marquee_start: Option<egui::Pos2>,    // in mat inches
    pub freehand_points: Vec<egui::Pos2>,     // in mat inches
    pub is_dragging_shape: bool,
    pub paper_texture: Option<egui::TextureHandle>,
    pub paper_image: Option<egui::ColorImage>,
    pub paper_bytes: Option<Vec<u8>>,
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
            cached_svgs: Vec::new(),
            selected_index: None,
            selected_indices: std::collections::BTreeSet::new(),
            expanded_groups: std::collections::BTreeSet::new(),
            next_group_id: 0,
            drag_start: None,
            marquee_start: None,
            freehand_points: Vec::new(),
            is_dragging_shape: false,
            paper_texture: None,
            paper_image: None,
            paper_bytes: None,
            status_message: None,
            screenshot_path,
            frames_rendered: 0,
        }
    }

    pub fn is_selected(&self, idx: usize) -> bool {
        self.selected_indices.contains(&idx) || self.selected_index == Some(idx)
    }

    pub fn all_selected_indices(&self) -> Vec<usize> {
        let mut set = self.selected_indices.clone();
        if let Some(i) = self.selected_index {
            set.insert(i);
        }
        set.into_iter().filter(|&i| i < self.shapes.len()).collect()
    }

    pub fn select_shape(&mut self, idx: usize, multi: bool) {
        if !multi {
            self.selected_indices.clear();
        }
        if let Some(gid) = self.shapes.get(idx).and_then(|s| s.group_id) {
            for (i, s) in self.shapes.iter().enumerate() {
                if s.group_id == Some(gid) {
                    self.selected_indices.insert(i);
                }
            }
        } else {
            self.selected_indices.insert(idx);
        }
        self.selected_index = Some(idx);
        self.sync_active_properties();
    }

    pub fn toggle_select_shape(&mut self, idx: usize) {
        let gid = self.shapes.get(idx).and_then(|s| s.group_id);
        let was_selected = self.is_selected(idx);
        if let Some(gid) = gid {
            for (i, s) in self.shapes.iter().enumerate() {
                if s.group_id == Some(gid) {
                    if was_selected {
                        self.selected_indices.remove(&i);
                    } else {
                        self.selected_indices.insert(i);
                    }
                }
            }
        } else {
            if was_selected {
                self.selected_indices.remove(&idx);
            } else {
                self.selected_indices.insert(idx);
            }
        }
        self.selected_index = self.selected_indices.iter().copied().next_back();
        self.sync_active_properties();
    }

    pub fn clear_selection(&mut self) {
        self.selected_indices.clear();
        self.selected_index = None;
    }

    pub fn sync_active_properties(&mut self) {
        if let Some(idx) = self.selected_index {
            if let Some(s) = self.shapes.get(idx) {
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
    }

    pub fn next_group_id(&mut self) -> u64 {
        self.next_group_id += 1;
        self.next_group_id
    }

    pub fn group_selected(&mut self) {
        let sel = self.all_selected_indices();
        if sel.len() >= 2 {
            let gid = self.next_group_id();
            for &idx in &sel {
                if let Some(s) = self.shapes.get_mut(idx) {
                    s.group_id = Some(gid);
                }
            }
            self.set_status(format!("Grouped {} shapes", sel.len()));
        } else if sel.len() == 1 {
            self.set_status("Select at least 2 shapes to group");
        }
    }

    pub fn ungroup_selected(&mut self) {
        let sel = self.all_selected_indices();
        let mut count = 0;
        for &idx in &sel {
            if let Some(s) = self.shapes.get_mut(idx) {
                if s.group_id.is_some() {
                    s.group_id = None;
                    s.is_modified = true;
                    count += 1;
                }
            }
        }
        if count > 0 {
            self.set_status(format!("Ungrouped {} shapes", count));
        } else {
            self.set_status("No grouped shapes selected");
        }
    }

    pub fn get_group_bounds(&self, gid: u64) -> Option<(f32, f32, f32, f32, usize, bool)> {
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        let mut count = 0;
        let mut any_visible = false;
        for s in &self.shapes {
            if s.group_id == Some(gid) {
                count += 1;
                if s.is_visible {
                    any_visible = true;
                    min_x = min_x.min(s.x);
                    min_y = min_y.min(s.y);
                    max_x = max_x.max(s.x + s.width);
                    max_y = max_y.max(s.y + s.height);
                }
            }
        }
        if count > 0 {
            Some((min_x, min_y, max_x, max_y, count, any_visible))
        } else {
            None
        }
    }

    pub fn group_by_color(&mut self) {
        let targets: Vec<usize> = if self.all_selected_indices().is_empty() {
            (0..self.shapes.len()).collect()
        } else {
            self.all_selected_indices()
        };
        if targets.is_empty() {
            self.set_status("No shapes to group");
            return;
        }

        let mut color_groups: std::collections::BTreeMap<([u8; 4], Option<[u8; 4]>), Vec<usize>> = std::collections::BTreeMap::new();
        for &idx in &targets {
            let shape = &self.shapes[idx];
            let fill_arr = [shape.fill_color.r(), shape.fill_color.g(), shape.fill_color.b(), shape.fill_color.a()];
            let stroke_arr = shape.stroke_color.map(|c| [c.r(), c.g(), c.b(), c.a()]);
            color_groups.entry((fill_arr, stroke_arr)).or_default().push(idx);
        }

        let mut num_groups = 0;
        for (_key, indices) in color_groups {
            let gid = self.next_group_id();
            for idx in indices {
                self.shapes[idx].group_id = Some(gid);
                self.shapes[idx].is_modified = true;
            }
            num_groups += 1;
        }
        self.set_status(format!("Grouped {} shapes into {} color layers", targets.len(), num_groups));
    }

    pub fn bring_to_front(&mut self) {
        let sel = self.all_selected_indices();
        if sel.is_empty() || sel.len() == self.shapes.len() {
            return;
        }
        let mut selected_shapes = Vec::new();
        let mut unselected_shapes = Vec::new();
        for (i, shape) in self.shapes.drain(..).enumerate() {
            if sel.contains(&i) {
                selected_shapes.push(shape);
            } else {
                unselected_shapes.push(shape);
            }
        }
        let unsel_count = unselected_shapes.len();
        self.shapes = unselected_shapes;
        self.shapes.extend(selected_shapes);
        self.selected_indices = (unsel_count..self.shapes.len()).collect();
        self.selected_index = self.shapes.len().checked_sub(1);
        self.set_status("Brought to front");
    }

    pub fn send_to_back(&mut self) {
        let sel = self.all_selected_indices();
        if sel.is_empty() || sel.len() == self.shapes.len() {
            return;
        }
        let mut selected_shapes = Vec::new();
        let mut unselected_shapes = Vec::new();
        for (i, shape) in self.shapes.drain(..).enumerate() {
            if sel.contains(&i) {
                selected_shapes.push(shape);
            } else {
                unselected_shapes.push(shape);
            }
        }
        let sel_count = selected_shapes.len();
        selected_shapes.extend(unselected_shapes);
        self.shapes = selected_shapes;
        self.selected_indices = (0..sel_count).collect();
        self.selected_index = Some(0);
        self.set_status("Sent to back");
    }

    pub fn bring_forward(&mut self) {
        let sel = self.all_selected_indices();
        if sel.is_empty() {
            return;
        }
        let mut new_sel = std::collections::BTreeSet::new();
        for &idx in sel.iter().rev() {
            if idx + 1 < self.shapes.len() && !sel.contains(&(idx + 1)) {
                self.shapes.swap(idx, idx + 1);
                new_sel.insert(idx + 1);
            } else {
                new_sel.insert(idx);
            }
        }
        self.selected_indices = new_sel;
        self.selected_index = self.selected_indices.iter().copied().next_back();
        self.set_status("Brought forward");
    }

    pub fn send_backward(&mut self) {
        let sel = self.all_selected_indices();
        if sel.is_empty() {
            return;
        }
        let mut new_sel = std::collections::BTreeSet::new();
        for &idx in &sel {
            if idx > 0 && !sel.contains(&(idx - 1)) {
                self.shapes.swap(idx, idx - 1);
                new_sel.insert(idx - 1);
            } else {
                new_sel.insert(idx);
            }
        }
        self.selected_indices = new_sel;
        self.selected_index = self.selected_indices.iter().copied().next();
        self.set_status("Sent backward");
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
        self.cached_svgs.clear();
        self.clear_selection();
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
            let fill_str = if shape.fill_color.a() == 0 {
                " fill=\"none\"".to_string()
            } else if opacity < 0.999 {
                format!(" fill=\"{}\" fill-opacity=\"{:.2}\"", hex, opacity)
            } else {
                format!(" fill=\"{}\"", hex)
            };

            let stroke_str = if let Some(sc) = shape.stroke_color {
                if sc.a() > 0 {
                    let sc_hex = format!("#{:02x}{:02x}{:02x}", sc.r(), sc.g(), sc.b());
                    let sw = if shape.stroke_width > 0.0 { shape.stroke_width } else { 0.05 };
                    if sc.a() < 255 {
                        format!(" stroke=\"{}\" stroke-width=\"{:.3}\" stroke-opacity=\"{:.2}\"", sc_hex, sw, sc.a() as f32 / 255.0)
                    } else {
                        format!(" stroke=\"{}\" stroke-width=\"{:.3}\"", sc_hex, sw)
                    }
                } else {
                    " stroke=\"none\"".to_string()
                }
            } else {
                " stroke=\"none\"".to_string()
            };

            match shape.kind {
                ShapeKind::Rectangle => {
                    out.push_str(&format!(
                        "  <rect x=\"{:.3}\" y=\"{:.3}\" width=\"{:.3}\" height=\"{:.3}\"{}{} />\n",
                        shape.x, shape.y, shape.width, shape.height, fill_str, stroke_str
                    ));
                }
                ShapeKind::Ellipse => {
                    let cx = shape.x + shape.width * 0.5;
                    let cy = shape.y + shape.height * 0.5;
                    let rx = shape.width * 0.5;
                    let ry = shape.height * 0.5;
                    out.push_str(&format!(
                        "  <ellipse cx=\"{:.3}\" cy=\"{:.3}\" rx=\"{:.3}\" ry=\"{:.3}\"{}{} />\n",
                        cx, cy, rx, ry, fill_str, stroke_str
                    ));
                }
                ShapeKind::Line => {
                    if shape.points.len() >= 2 {
                        out.push_str(&format!(
                            "  <line x1=\"{:.3}\" y1=\"{:.3}\" x2=\"{:.3}\" y2=\"{:.3}\"{} stroke-linecap=\"round\" />\n",
                            shape.points[0].x, shape.points[0].y, shape.points[1].x, shape.points[1].y, stroke_str
                        ));
                    }
                }
                ShapeKind::Freehand => {
                    if shape.points.len() >= 2 {
                        let pts_str = shape.points.iter()
                            .map(|p| format!("{:.3},{:.3}", p.x, p.y))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let tag = if shape.is_closed { "polygon" } else { "polyline" };
                        out.push_str(&format!(
                            "  <{} points=\"{}\"{}{} stroke-linecap=\"round\" stroke-linejoin=\"round\" />\n",
                            tag, pts_str, fill_str, stroke_str
                        ));
                    }
                }
                ShapeKind::Text => {
                    let escaped = escape_xml(&shape.text);
                    let y_base = shape.y + shape.height * 0.85;
                    out.push_str(&format!(
                        "  <text x=\"{:.3}\" y=\"{:.3}\" font-family=\"sans-serif\" font-size=\"{:.3}\"{}>{}</text>\n",
                        shape.x, y_base, shape.height, fill_str, escaped
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

        // Optional paper background image
        if let Some(ref bytes) = self.paper_bytes {
            if let Ok(dyn_img) = image::load_from_memory(bytes) {
                let rgba = dyn_img.to_rgba8();
                let (pw, ph) = rgba.dimensions();
                if let Some(paper_pixmap) = tiny_skia::Pixmap::from_vec(rgba.into_raw(), tiny_skia::IntSize::from_wh(pw, ph).unwrap()) {
                    let sx = w as f32 / pw as f32;
                    let sy = h as f32 / ph as f32;
                    let ts = tiny_skia::Transform::from_scale(sx, sy);
                    pixmap.draw_pixmap(0, 0, paper_pixmap.as_ref(), &tiny_skia::PixmapPaint::default(), ts, None);
                }
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

        // Render cached SVGs (crisp vector rendering with EvenOdd cutouts and gradients bound to live group transforms)
        let mut active_cached_groups = std::collections::BTreeSet::new();
        for cached in &self.cached_svgs {
            if let Some((min_x, min_y, max_x, max_y, count, any_visible)) = self.get_group_bounds(cached.group_id) {
                if count == cached.orig_shape_count && any_visible {
                    active_cached_groups.insert(cached.group_id);
                    let live_w = (max_x - min_x).max(0.001);
                    let live_h = (max_y - min_y).max(0.001);
                    let scale_x = live_w / cached.init_width.max(0.001);
                    let scale_y = live_h / cached.init_height.max(0.001);
                    let live_svg_x = min_x - (cached.init_min_x * scale_x);
                    let live_svg_y = min_y - (cached.init_min_y * scale_y);
                    let live_svg_w = cached.orig_svg_width * scale_x;
                    let live_svg_h = cached.orig_svg_height * scale_y;

                    let sx = (live_svg_w * dpi) / cached.pixmap.width() as f32;
                    let sy = (live_svg_h * dpi) / cached.pixmap.height() as f32;
                    let ts = tiny_skia::Transform::from_scale(sx, sy).post_translate(live_svg_x * dpi, live_svg_y * dpi);
                    pixmap.draw_pixmap(0, 0, cached.pixmap.as_ref(), &tiny_skia::PixmapPaint::default(), ts, None);
                }
            }
        }

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
            if !shape.is_visible {
                continue;
            }
            if let Some(gid) = shape.group_id {
                if active_cached_groups.contains(&gid) {
                    continue;
                }
            }
            let mut fill_paint = tiny_skia::Paint::default();
            fill_paint.set_color_rgba8(
                shape.fill_color.r(),
                shape.fill_color.g(),
                shape.fill_color.b(),
                shape.fill_color.a(),
            );
            fill_paint.anti_alias = true;

            let stroke_paint_and_width = shape.stroke_color.and_then(|sc| {
                if sc.a() > 0 {
                    let mut sp = tiny_skia::Paint::default();
                    sp.set_color_rgba8(sc.r(), sc.g(), sc.b(), sc.a());
                    sp.anti_alias = true;
                    let sw = (shape.stroke_width * dpi).max(1.0);
                    Some((sp, sw))
                } else {
                    None
                }
            });

            match shape.kind {
                ShapeKind::Rectangle => {
                    if let Some(rect) = tiny_skia::Rect::from_xywh(
                        shape.x * dpi,
                        shape.y * dpi,
                        shape.width * dpi,
                        shape.height * dpi,
                    ) {
                        if shape.fill_color.a() > 0 {
                            pixmap.fill_rect(rect, &fill_paint, tiny_skia::Transform::identity(), None);
                        }
                        if let Some((sp, sw)) = &stroke_paint_and_width {
                            let st = tiny_skia::Stroke { width: *sw, ..Default::default() };
                            let path = tiny_skia::PathBuilder::from_rect(rect);
                            pixmap.stroke_path(&path, sp, &st, tiny_skia::Transform::identity(), None);
                        }
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
                            if shape.fill_color.a() > 0 {
                                pixmap.fill_path(&path, &fill_paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
                            }
                            if let Some((sp, sw)) = &stroke_paint_and_width {
                                let st = tiny_skia::Stroke { width: *sw, ..Default::default() };
                                pixmap.stroke_path(&path, sp, &st, tiny_skia::Transform::identity(), None);
                            }
                        }
                    }
                }
                ShapeKind::Line => {
                    if shape.points.len() >= 2 {
                        let mut pb = tiny_skia::PathBuilder::new();
                        pb.move_to(shape.points[0].x * dpi, shape.points[0].y * dpi);
                        pb.line_to(shape.points[1].x * dpi, shape.points[1].y * dpi);
                        if let Some(path) = pb.finish() {
                            let (sp, sw) = stroke_paint_and_width.as_ref().cloned().unwrap_or_else(|| {
                                (black_paint.clone(), stroke_w)
                            });
                            let st = tiny_skia::Stroke {
                                width: sw,
                                line_cap: tiny_skia::LineCap::Round,
                                ..Default::default()
                            };
                            pixmap.stroke_path(&path, &sp, &st, tiny_skia::Transform::identity(), None);
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
                        if shape.is_closed {
                            pb.close();
                        }
                        if let Some(path) = pb.finish() {
                            if shape.fill_color.a() > 0 {
                                pixmap.fill_path(&path, &fill_paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
                            }
                            if let Some((sp, sw)) = &stroke_paint_and_width {
                                let st = tiny_skia::Stroke {
                                    width: *sw,
                                    line_cap: tiny_skia::LineCap::Round,
                                    line_join: tiny_skia::LineJoin::Round,
                                    ..Default::default()
                                };
                                pixmap.stroke_path(&path, sp, &st, tiny_skia::Transform::identity(), None);
                            } else if !shape.is_closed && shape.fill_color.a() == 0 {
                                pixmap.stroke_path(&path, &black_paint, &black_stroke, tiny_skia::Transform::identity(), None);
                            }
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

    // 4. Import SVG into CanvasShapes with cached resvg texture
    pub fn import_svg_file(&mut self, path: &Path) -> Result<usize, Box<dyn std::error::Error>> {
        let content = std::fs::read(path)?;
        self.import_svg_bytes(&content)
    }

    pub fn import_svg_bytes(&mut self, bytes: &[u8]) -> Result<usize, Box<dyn std::error::Error>> {
        let opt = usvg::Options::default();
        let tree = usvg::Tree::from_data(bytes, &opt)?;

        let svg_w = tree.size().width();
        let svg_h = tree.size().height();
        let scale_to_in = if svg_w <= 36.0 && svg_h <= 36.0 {
            1.0
        } else {
            1.0 / 96.0
        };
        let w_in = svg_w * scale_to_in;
        let h_in = svg_h * scale_to_in;

        // Render high-resolution pixmap using resvg (150 DPI for crisp 60 FPS viewport)
        let target_dpi = 150.0_f32;
        let pix_w = ((w_in * target_dpi).round() as u32).clamp(32, 4096);
        let pix_h = ((h_in * target_dpi).round() as u32).clamp(32, 4096);
        let mut pixmap = tiny_skia::Pixmap::new(pix_w, pix_h).ok_or("Failed to allocate Pixmap")?;
        let sx = pix_w as f32 / svg_w;
        let sy = pix_h as f32 / svg_h;
        let ts = tiny_skia::Transform::from_scale(sx, sy);
        resvg::render(&tree, ts, &mut pixmap.as_mut());

        let mut imported = import_svg_data(bytes, self.current_color32());
        let count = imported.len();
        if count > 0 {
            let gid = self.next_group_id();
            let start_idx = self.shapes.len();
            let mut init_min_x = f32::MAX;
            let mut init_min_y = f32::MAX;
            let mut init_max_x = f32::MIN;
            let mut init_max_y = f32::MIN;
            for s in &mut imported {
                s.is_svg_path = true;
                s.group_id = Some(gid);
                s.precompute_triangles();
                init_min_x = init_min_x.min(s.x);
                init_min_y = init_min_y.min(s.y);
                init_max_x = init_max_x.max(s.x + s.width);
                init_max_y = init_max_y.max(s.y + s.height);
            }
            let init_w = (init_max_x - init_min_x).max(0.001);
            let init_h = (init_max_y - init_min_y).max(0.001);

            self.shapes.extend(imported);
            self.selected_indices = (start_idx..self.shapes.len()).collect();
            self.selected_index = Some(self.shapes.len() - 1);

            self.cached_svgs.push(CachedSvg {
                group_id: gid,
                init_min_x,
                init_min_y,
                init_width: init_w,
                init_height: init_h,
                orig_svg_width: w_in,
                orig_svg_height: h_in,
                orig_shape_count: count,
                pixmap,
                texture: None,
            });
        }
        Ok(count)
    }
}

// SVG Import parser using usvg
pub fn import_svg_str(xml: &str, default_color: egui::Color32) -> Vec<CanvasShape> {
    import_svg_data(xml.as_bytes(), default_color)
}

pub fn import_svg_data(bytes: &[u8], default_color: egui::Color32) -> Vec<CanvasShape> {
    let opt = usvg::Options::default();
    let tree = match usvg::Tree::from_data(bytes, &opt) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };

    let svg_w = tree.size().width();
    let svg_h = tree.size().height();
    let scale_to_in = if svg_w <= 36.0 && svg_h <= 36.0 {
        1.0
    } else {
        1.0 / 96.0
    };

    let mut usvg_paths = Vec::new();
    collect_group_paths(tree.root(), &mut usvg_paths);

    let mut result = Vec::new();

    for path in usvg_paths {
        let is_both_none = path.fill().is_none() && path.stroke().is_none();
        if !path.is_visible() && !is_both_none {
            continue;
        }

        let fill_color = if let Some(fill) = path.fill() {
            match fill.paint() {
                usvg::Paint::Color(c) => {
                    let a = (fill.opacity().get() * 255.0).round() as u8;
                    egui::Color32::from_rgba_unmultiplied(c.red, c.green, c.blue, a)
                }
                _ => default_color,
            }
        } else {
            egui::Color32::TRANSPARENT
        };

        let (stroke_color, stroke_width_in) = if let Some(stroke) = path.stroke() {
            let col = match stroke.paint() {
                usvg::Paint::Color(c) => {
                    let a = (stroke.opacity().get() * 255.0).round() as u8;
                    egui::Color32::from_rgba_unmultiplied(c.red, c.green, c.blue, a)
                }
                _ => egui::Color32::BLACK,
            };
            let w_in = (stroke.width().get() * scale_to_in).max(0.01);
            (Some(col), w_in)
        } else if is_both_none {
            // Cutting files with fill="none" and no stroke attribute default to a cut stroke
            (Some(default_color), 0.05)
        } else {
            (None, 0.0)
        };

        if fill_color.a() == 0 && stroke_color.is_none() {
            continue;
        }

        let ts = path.abs_transform();
        let transformed = if ts.is_identity() {
            Some(path.data().clone())
        } else {
            path.data().clone().transform(ts)
        };

        let data = match transformed {
            Some(d) => d,
            None => continue,
        };

        let subpaths = extract_subpaths(&data, scale_to_in);

        for (raw_segments, sampled_points, is_closed) in subpaths {
            if sampled_points.len() < 2 {
                continue;
            }

            let mut min_x = sampled_points[0].x;
            let mut min_y = sampled_points[0].y;
            let mut max_x = sampled_points[0].x;
            let mut max_y = sampled_points[0].y;
            for p in &sampled_points {
                min_x = min_x.min(p.x);
                min_y = min_y.min(p.y);
                max_x = max_x.max(p.x);
                max_y = max_y.max(p.y);
            }
            let width = (max_x - min_x).max(0.01);
            let height = (max_y - min_y).max(0.01);

            let (kind, final_points) = classify_subpath(&raw_segments, &sampled_points, is_closed);

            let mut shape = CanvasShape {
                kind,
                x: min_x,
                y: min_y,
                width,
                height,
                points: final_points,
                text: String::new(),
                fill_color,
                stroke_color,
                stroke_width: stroke_width_in,
                is_closed,
                is_svg_path: true,
                is_modified: false,
                triangles: Vec::new(),
                group_id: None,
                is_visible: true,
                is_locked: false,
            };
            shape.precompute_triangles();
            result.push(shape);
        }
    }

    result
}

fn collect_group_paths<'a>(g: &'a usvg::Group, out: &mut Vec<&'a usvg::Path>) {
    for child in g.children() {
        match child {
            usvg::Node::Group(sub_g) => collect_group_paths(sub_g, out),
            usvg::Node::Path(p) => {
                out.push(p);
            }
            usvg::Node::Text(t) => {
                collect_group_paths(t.flattened(), out);
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
enum RawSegment {
    MoveTo(egui::Pos2),
    LineTo(egui::Pos2),
    QuadTo(egui::Pos2, egui::Pos2),
    CubicTo(egui::Pos2, egui::Pos2, egui::Pos2),
    Close,
}

fn extract_subpaths(
    data: &usvg::tiny_skia_path::Path,
    scale: f32,
) -> Vec<(Vec<RawSegment>, Vec<egui::Pos2>, bool)> {
    let mut result = Vec::new();

    let mut cur_raw = Vec::new();
    let mut cur_pts = Vec::new();
    let mut cur_pt = egui::pos2(0.0, 0.0);
    let mut start_pt = egui::pos2(0.0, 0.0);
    let mut is_closed = false;

    let finish_subpath = |raw: &mut Vec<RawSegment>, pts: &mut Vec<egui::Pos2>, closed: &mut bool, res: &mut Vec<(Vec<RawSegment>, Vec<egui::Pos2>, bool)>| {
        if pts.len() >= 2 {
            res.push((std::mem::take(raw), std::mem::take(pts), *closed));
        } else {
            raw.clear();
            pts.clear();
        }
        *closed = false;
    };

    for seg in data.segments() {
        match seg {
            usvg::tiny_skia_path::PathSegment::MoveTo(p) => {
                if !cur_pts.is_empty() {
                    finish_subpath(&mut cur_raw, &mut cur_pts, &mut is_closed, &mut result);
                }
                cur_pt = egui::pos2(p.x * scale, p.y * scale);
                start_pt = cur_pt;
                cur_raw.push(RawSegment::MoveTo(cur_pt));
                cur_pts.push(cur_pt);
            }
            usvg::tiny_skia_path::PathSegment::LineTo(p) => {
                cur_pt = egui::pos2(p.x * scale, p.y * scale);
                cur_raw.push(RawSegment::LineTo(cur_pt));
                cur_pts.push(cur_pt);
            }
            usvg::tiny_skia_path::PathSegment::QuadTo(p1, p2) => {
                let q1 = egui::pos2(p1.x * scale, p1.y * scale);
                let q2 = egui::pos2(p2.x * scale, p2.y * scale);
                cur_raw.push(RawSegment::QuadTo(q1, q2));
                for step in 1..=8 {
                    let t = step as f32 / 8.0;
                    let it = 1.0 - t;
                    let pt = cur_pt * (it * it) + q1.to_vec2() * (2.0 * it * t) + q2.to_vec2() * (t * t);
                    cur_pts.push(pt);
                }
                cur_pt = q2;
            }
            usvg::tiny_skia_path::PathSegment::CubicTo(p1, p2, p3) => {
                let c1 = egui::pos2(p1.x * scale, p1.y * scale);
                let c2 = egui::pos2(p2.x * scale, p2.y * scale);
                let c3 = egui::pos2(p3.x * scale, p3.y * scale);
                cur_raw.push(RawSegment::CubicTo(c1, c2, c3));
                for step in 1..=12 {
                    let t = step as f32 / 12.0;
                    let it = 1.0 - t;
                    let pt = cur_pt * (it * it * it)
                        + c1.to_vec2() * (3.0 * it * it * t)
                        + c2.to_vec2() * (3.0 * it * t * t)
                        + c3.to_vec2() * (t * t * t);
                    cur_pts.push(pt);
                }
                cur_pt = c3;
            }
            usvg::tiny_skia_path::PathSegment::Close => {
                cur_raw.push(RawSegment::Close);
                is_closed = true;
                if (cur_pt - start_pt).length() > 0.001 {
                    cur_pts.push(start_pt);
                    cur_pt = start_pt;
                }
            }
        }
    }

    if !cur_pts.is_empty() {
        finish_subpath(&mut cur_raw, &mut cur_pts, &mut is_closed, &mut result);
    }

    result
}

fn classify_subpath(
    raw: &[RawSegment],
    pts: &[egui::Pos2],
    is_closed: bool,
) -> (ShapeKind, Vec<egui::Pos2>) {
    // 1. Line: MoveTo + 1 LineTo (open)
    if raw.len() == 2 && !is_closed {
        if let (RawSegment::MoveTo(p0), RawSegment::LineTo(p1)) = (&raw[0], &raw[1]) {
            return (ShapeKind::Line, vec![*p0, *p1]);
        }
    }

    // 2. Axis-aligned Rectangle: MoveTo + 3 LineTo + Close
    if is_closed && raw.len() == 5 {
        if let (
            RawSegment::MoveTo(p0),
            RawSegment::LineTo(p1),
            RawSegment::LineTo(p2),
            RawSegment::LineTo(p3),
            RawSegment::Close,
        ) = (&raw[0], &raw[1], &raw[2], &raw[3], &raw[4]) {
            let is_axis_aligned =
                ((p0.y - p1.y).abs() < 0.01 && (p1.x - p2.x).abs() < 0.01 && (p2.y - p3.y).abs() < 0.01 && (p3.x - p0.x).abs() < 0.01)
                || ((p0.x - p1.x).abs() < 0.01 && (p1.y - p2.y).abs() < 0.01 && (p2.x - p3.x).abs() < 0.01 && (p3.y - p0.y).abs() < 0.01);
            if is_axis_aligned {
                return (ShapeKind::Rectangle, vec![]);
            }
        }
    }

    // 3. Ellipse / Circle: MoveTo + 4 CubicTo + Close
    if is_closed && raw.len() == 6 {
        if let (
            RawSegment::MoveTo(_),
            RawSegment::CubicTo(_, _, _),
            RawSegment::CubicTo(_, _, _),
            RawSegment::CubicTo(_, _, _),
            RawSegment::CubicTo(_, _, _),
            RawSegment::Close,
        ) = (&raw[0], &raw[1], &raw[2], &raw[3], &raw[4], &raw[5]) {
            return (ShapeKind::Ellipse, vec![]);
        }
    }

    // 4. Default: Freehand polygon/polyline
    (ShapeKind::Freehand, pts.to_vec())
}

fn point_in_poly(p: egui::Pos2, poly: &[egui::Pos2]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let pi = poly[i];
        let pj = poly[j];
        if ((pi.y > p.y) != (pj.y > p.y))
            && (p.x < (pj.x - pi.x) * (p.y - pi.y) / (pj.y - pi.y + 1e-6) + pi.x)
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn triangulate_polygon(pts: &[egui::Pos2]) -> Vec<[usize; 3]> {
    let mut clean_pts = pts;
    if clean_pts.len() >= 4 && (clean_pts[0] - *clean_pts.last().unwrap()).length() < 0.001 {
        clean_pts = &clean_pts[..clean_pts.len() - 1];
    }

    let n = clean_pts.len();
    if n < 3 {
        return Vec::new();
    }
    if n == 3 {
        return vec![[0, 1, 2]];
    }

    let mut area = 0.0_f32;
    for i in 0..n {
        let j = (i + 1) % n;
        area += clean_pts[i].x * clean_pts[j].y - clean_pts[j].x * clean_pts[i].y;
    }
    if area.abs() < 1e-6 {
        return Vec::new();
    }
    let ccw = area > 0.0;

    let is_convex = |a: egui::Pos2, b: egui::Pos2, c: egui::Pos2| -> bool {
        let cross = (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x);
        if ccw { cross > 1e-6 } else { cross < -1e-6 }
    };

    let point_in_triangle = |p: egui::Pos2, a: egui::Pos2, b: egui::Pos2, c: egui::Pos2| -> bool {
        let cp1 = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        let cp2 = (c.x - b.x) * (p.y - b.y) - (c.y - b.y) * (p.x - b.x);
        let cp3 = (a.x - c.x) * (p.y - c.y) - (a.y - c.y) * (p.x - c.x);
        if ccw {
            cp1 >= -1e-6 && cp2 >= -1e-6 && cp3 >= -1e-6
        } else {
            cp1 <= 1e-6 && cp2 <= 1e-6 && cp3 <= 1e-6
        }
    };

    let mut indices: Vec<usize> = (0..n).collect();
    let mut triangles = Vec::with_capacity(n - 2);

    let mut count = 2 * indices.len();
    let mut i = 0;
    while indices.len() > 3 && count > 0 {
        count -= 1;
        let len = indices.len();
        let prev = indices[(i + len - 1) % len];
        let curr = indices[i % len];
        let next = indices[(i + 1) % len];

        let p_prev = clean_pts[prev];
        let p_curr = clean_pts[curr];
        let p_next = clean_pts[next];

        if is_convex(p_prev, p_curr, p_next) {
            let mut is_ear = true;
            for &idx in &indices {
                if idx != prev && idx != curr && idx != next {
                    if point_in_triangle(clean_pts[idx], p_prev, p_curr, p_next) {
                        is_ear = false;
                        break;
                    }
                }
            }
            if is_ear {
                triangles.push([prev, curr, next]);
                indices.remove(i % len);
                count = 2 * indices.len();
                continue;
            }
        }
        i = (i + 1) % indices.len();
    }

    if indices.len() == 3 {
        triangles.push([indices[0], indices[1], indices[2]]);
    } else if indices.len() > 3 {
        for k in 1..indices.len() - 1 {
            triangles.push([indices[0], indices[k], indices[k + 1]]);
        }
    }

    triangles
}

fn draw_filled_polygon(
    painter: &egui::Painter,
    screen_pts: &[egui::Pos2],
    fill_color: egui::Color32,
    stroke: Option<egui::Stroke>,
    cached_triangles: &[[usize; 3]],
) {
    if screen_pts.len() < 3 {
        if let Some(s) = stroke {
            if screen_pts.len() >= 2 {
                painter.add(egui::Shape::line(screen_pts.to_vec(), s));
            }
        }
        return;
    }

    if fill_color.a() > 0 {
        if !cached_triangles.is_empty() {
            let mut mesh = egui::Mesh::default();
            let mut clean_pts = screen_pts;
            if clean_pts.len() >= 4 && (clean_pts[0] - *clean_pts.last().unwrap()).length() < 0.001 {
                clean_pts = &clean_pts[..clean_pts.len() - 1];
            }
            for &pt in clean_pts {
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: pt,
                    uv: egui::epaint::WHITE_UV,
                    color: fill_color,
                });
            }
            for &[i0, i1, i2] in cached_triangles {
                if i0 < mesh.vertices.len() && i1 < mesh.vertices.len() && i2 < mesh.vertices.len() {
                    mesh.add_triangle(i0 as u32, i1 as u32, i2 as u32);
                }
            }
            painter.add(egui::Shape::mesh(mesh));
        } else {
            painter.add(egui::Shape::convex_polygon(screen_pts.to_vec(), fill_color, egui::Stroke::NONE));
        }
    }

    if let Some(s) = stroke {
        if s.width > 0.0 && s.color.a() > 0 {
            let mut closed_pts = screen_pts.to_vec();
            if let (Some(first), Some(last)) = (closed_pts.first(), closed_pts.last()) {
                if (*first - *last).length() > 0.5 {
                    closed_pts.push(*first);
                }
            }
            painter.add(egui::Shape::line(closed_pts, s));
        }
    }
}
impl eframe::App for StudioApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Load textures for cached SVGs if not loaded yet
        for cached in &mut self.cached_svgs {
            if cached.texture.is_none() {
                let img = egui::ColorImage::from_rgba_premultiplied(
                    [cached.pixmap.width() as usize, cached.pixmap.height() as usize],
                    cached.pixmap.data(),
                );
                cached.texture = Some(ctx.load_texture(
                    format!("cached_svg_group_{}", cached.group_id),
                    img,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }

        if self.paper_texture.is_none() {
            if let Some(ref img) = self.paper_image {
                self.paper_texture = Some(ctx.load_texture(
                    "paper_background",
                    img.clone(),
                    egui::TextureOptions::LINEAR,
                ));
            }
        }

        if self.screenshot_path.is_some() {
            ctx.request_repaint();
            self.frames_rendered += 1;
            if self.frames_rendered == 60 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            }
            if self.frames_rendered > 120 {
                std::process::exit(0);
            }
        }
        ctx.input(|i| {
            for event in &i.events {
                if let egui::Event::Screenshot { image, .. } = event {
                    if let Some(path) = &self.screenshot_path {
                        let _ = save_png(path, image);
                        std::process::exit(0);
                    }
                }
            }
        });

        // Keyboard shortcuts
        if !ctx.wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
                let mut to_remove = self.all_selected_indices();
                to_remove.sort_unstable();
                for &idx in to_remove.iter().rev() {
                    if idx < self.shapes.len() && !self.shapes[idx].is_locked {
                        self.shapes.remove(idx);
                    }
                }
                self.clear_selection();
            }
            if ctx.input(|i| (i.modifiers.command || i.modifiers.ctrl) && !i.modifiers.shift && i.key_pressed(egui::Key::G)) {
                self.group_selected();
            }
            if ctx.input(|i| (i.modifiers.command || i.modifiers.ctrl) && i.modifiers.shift && i.key_pressed(egui::Key::G)) {
                self.ungroup_selected();
            }
        }

        // Top menu
        egui::TopBottomPanel::top("top_menu").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New Design").clicked() {
                        self.shapes.clear();
                        self.cached_svgs.clear();
                        self.clear_selection();
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
                    ui.separator();
                    let sel_count = self.all_selected_indices().len();
                    if ui.add_enabled(sel_count >= 2, egui::Button::new("Group (Ctrl+G)")).clicked() {
                        self.group_selected();
                        ui.close_menu();
                    }
                    if ui.add_enabled(sel_count >= 1, egui::Button::new("Ungroup (Ctrl+Shift+G)")).clicked() {
                        self.ungroup_selected();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.add_enabled(sel_count >= 1, egui::Button::new("Bring to Front")).clicked() {
                        self.bring_to_front();
                        ui.close_menu();
                    }
                    if ui.add_enabled(sel_count >= 1, egui::Button::new("Bring Forward")).clicked() {
                        self.bring_forward();
                        ui.close_menu();
                    }
                    if ui.add_enabled(sel_count >= 1, egui::Button::new("Send Backward")).clicked() {
                        self.send_backward();
                        ui.close_menu();
                    }
                    if ui.add_enabled(sel_count >= 1, egui::Button::new("Send to Back")).clicked() {
                        self.send_to_back();
                        ui.close_menu();
                    }
                    ui.separator();
                    let has_sel = sel_count > 0;
                    if ui.add_enabled(has_sel, egui::Button::new("Delete Selected")).clicked() {
                        let mut to_remove = self.all_selected_indices();
                        to_remove.sort_unstable();
                        for &idx in to_remove.iter().rev() {
                            if idx < self.shapes.len() && !self.shapes[idx].is_locked {
                                self.shapes.remove(idx);
                            }
                        }
                        self.clear_selection();
                        ui.close_menu();
                    }
                    if ui.button("Clear All Shapes").clicked() {
                        self.shapes.clear();
                        self.cached_svgs.clear();
                        self.clear_selection();
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
        egui::SidePanel::right("right_panels").default_width(260.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for tab in ["Page Setup", "Layers", "Fill", "Text", "Transform"] {
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

                    ui.add_space(10.0);
                    ui.separator();
                    ui.heading("Paper Background");
                    if self.paper_image.is_some() {
                        ui.label("Custom background paper loaded.");
                        if ui.button("🗑 Clear Paper").clicked() {
                            self.paper_texture = None;
                            self.paper_image = None;
                            self.paper_bytes = None;
                            self.set_status("Paper background cleared");
                        }
                    } else {
                        if ui.button("🖼 Import Paper Background...").clicked() {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("Images (*.png, *.jpg, *.jpeg)", &["png", "jpg", "jpeg"])
                                .pick_file()
                            {
                                match std::fs::read(&path) {
                                    Ok(bytes) => {
                                        match image::load_from_memory(&bytes) {
                                            Ok(dyn_img) => {
                                                let rgba = dyn_img.to_rgba8();
                                                let (w, h) = rgba.dimensions();
                                                let color_img = egui::ColorImage::from_rgba_unmultiplied(
                                                    [w as usize, h as usize],
                                                    &rgba.into_raw(),
                                                );
                                                self.paper_image = Some(color_img);
                                                self.paper_texture = None;
                                                self.paper_bytes = Some(bytes);
                                                self.set_status("Imported paper background");
                                            }
                                            Err(e) => self.set_status(format!("Error decoding image: {}", e)),
                                        }
                                    }
                                    Err(e) => self.set_status(format!("Error reading file: {}", e)),
                                }
                            }
                        }
                    }
                }
                "Layers" => {
                    ui.heading("Layers");
                    ui.horizontal(|ui| {
                        let has_sel = !self.all_selected_indices().is_empty();
                        if ui.add_enabled(has_sel, egui::Button::new("⏏ Front")).on_hover_text("Bring to Front").clicked() {
                            self.bring_to_front();
                        }
                        if ui.add_enabled(has_sel, egui::Button::new("▲ Up")).on_hover_text("Bring Forward").clicked() {
                            self.bring_forward();
                        }
                        if ui.add_enabled(has_sel, egui::Button::new("▼ Down")).on_hover_text("Send Backward").clicked() {
                            self.send_backward();
                        }
                        if ui.add_enabled(has_sel, egui::Button::new("⏷ Back")).on_hover_text("Send to Back").clicked() {
                            self.send_to_back();
                        }
                    });
                    ui.horizontal(|ui| {
                        let sel_count = self.all_selected_indices().len();
                        if ui.add_enabled(sel_count >= 2, egui::Button::new("🔗 Group")).on_hover_text("Group Selected (Ctrl+G)").clicked() {
                            self.group_selected();
                        }
                        if ui.add_enabled(sel_count >= 1, egui::Button::new("🔓 Ungroup")).on_hover_text("Ungroup Selected (Ctrl+Shift+G)").clicked() {
                            self.ungroup_selected();
                        }
                        if ui.button("🎨 By Color").on_hover_text("Group shapes by fill/stroke color into cardstock layers").clicked() {
                            self.group_by_color();
                        }
                    });
                    ui.separator();

                    enum LayerRow {
                        Group { gid: u64, members: Vec<usize> },
                        Single { idx: usize },
                    }

                    let mut rows: Vec<LayerRow> = Vec::new();
                    let mut seen_groups = std::collections::BTreeSet::new();

                    for i in (0..self.shapes.len()).rev() {
                        if let Some(gid) = self.shapes[i].group_id {
                            if !seen_groups.contains(&gid) {
                                seen_groups.insert(gid);
                                let members: Vec<usize> = (0..self.shapes.len())
                                    .rev()
                                    .filter(|&m| self.shapes[m].group_id == Some(gid))
                                    .collect();
                                rows.push(LayerRow::Group { gid, members });
                            }
                        } else {
                            rows.push(LayerRow::Single { idx: i });
                        }
                    }

                    enum LayerAction {
                        ToggleExpand(u64),
                        ToggleGroupVis(u64, bool),
                        ToggleGroupLock(u64, bool),
                        SelectGroup(Vec<usize>, bool),
                        ToggleShapeVis(usize),
                        ToggleShapeLock(usize),
                        SelectShape(usize, bool),
                    }
                    let mut actions: Vec<LayerAction> = Vec::new();

                    egui::ScrollArea::vertical().max_height(450.0).show(ui, |ui| {
                        if rows.is_empty() {
                            ui.label("No shapes on mat.");
                        }
                        for row in &rows {
                            match row {
                                LayerRow::Group { gid, members } => {
                                    let is_expanded = self.expanded_groups.contains(gid);
                                    let all_vis = members.iter().all(|&m| self.shapes[m].is_visible);
                                    let all_lock = members.iter().all(|&m| self.shapes[m].is_locked);
                                    let is_sel = members.iter().any(|&m| self.is_selected(m));

                                    let swatch_color = members.iter().find_map(|&m| {
                                        let s = &self.shapes[m];
                                        if s.fill_color.a() > 0 {
                                            Some(s.fill_color)
                                        } else {
                                            s.stroke_color.filter(|c| c.a() > 0)
                                        }
                                    }).unwrap_or(egui::Color32::GRAY);

                                    ui.horizontal(|ui| {
                                        let expand_symbol = if is_expanded { "▼" } else { "▶" };
                                        if ui.small_button(expand_symbol).clicked() {
                                            actions.push(LayerAction::ToggleExpand(*gid));
                                        }

                                        let vis_icon = if all_vis { "👁" } else { "🚫" };
                                        if ui.small_button(vis_icon).clicked() {
                                            actions.push(LayerAction::ToggleGroupVis(*gid, !all_vis));
                                        }

                                        let lock_icon = if all_lock { "🔒" } else { "🔓" };
                                        if ui.small_button(lock_icon).clicked() {
                                            actions.push(LayerAction::ToggleGroupLock(*gid, !all_lock));
                                        }

                                        let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                                        ui.painter().rect_filled(rect, 2.0, swatch_color);
                                        ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.0_f32, egui::Color32::GRAY));

                                        let group_label = format!("Group {} ({} items)", gid, members.len());
                                        let resp = ui.selectable_label(is_sel, group_label);
                                        if resp.clicked() {
                                            let shift = ui.input(|inp| inp.modifiers.shift);
                                            actions.push(LayerAction::SelectGroup(members.clone(), shift));
                                        }
                                    });

                                    if is_expanded {
                                        let row_height = 20.0_f32;
                                        let member_count = members.len();
                                        egui::ScrollArea::vertical()
                                            .max_height(200.0)
                                            .id_source(format!("group_scroll_{}", gid))
                                            .show_rows(ui, row_height, member_count, |ui, row_range| {
                                                for r in row_range {
                                                    let m = members[r];
                                                    let shape = &self.shapes[m];
                                                    let s_sel = self.is_selected(m);
                                                    let kind_label = match shape.kind {
                                                        ShapeKind::Rectangle => "Rectangle",
                                                        ShapeKind::Ellipse => "Ellipse",
                                                        ShapeKind::Line => "Line",
                                                        ShapeKind::Freehand => if shape.is_svg_path { "SVG Path" } else { "Freehand" },
                                                        ShapeKind::Text => "Text",
                                                    };
                                                    let s_label = format!("  #{} {}", m, kind_label);
                                                    let s_vis = if shape.is_visible { "👁" } else { "🚫" };
                                                    let s_lock = if shape.is_locked { "🔒" } else { "🔓" };
                                                    let s_swatch = if shape.fill_color.a() > 0 {
                                                        shape.fill_color
                                                    } else if let Some(sc) = shape.stroke_color {
                                                        sc
                                                    } else {
                                                        egui::Color32::BLACK
                                                    };

                                                    ui.horizontal(|ui| {
                                                        ui.add_space(12.0);
                                                        if ui.small_button(s_vis).clicked() {
                                                            actions.push(LayerAction::ToggleShapeVis(m));
                                                        }
                                                        if ui.small_button(s_lock).clicked() {
                                                            actions.push(LayerAction::ToggleShapeLock(m));
                                                        }
                                                        let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                                                        ui.painter().rect_filled(rect, 2.0, s_swatch);
                                                        ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.0_f32, egui::Color32::GRAY));

                                                        if ui.selectable_label(s_sel, s_label).clicked() {
                                                            let shift = ui.input(|inp| inp.modifiers.shift);
                                                            actions.push(LayerAction::SelectShape(m, shift));
                                                        }
                                                    });
                                                }
                                            });
                                    }
                                }
                                LayerRow::Single { idx } => {
                                    let i = *idx;
                                    let shape = &self.shapes[i];
                                    let is_sel = self.is_selected(i);
                                    let kind_label = match shape.kind {
                                        ShapeKind::Rectangle => "Rectangle",
                                        ShapeKind::Ellipse => "Ellipse",
                                        ShapeKind::Line => "Line",
                                        ShapeKind::Freehand => if shape.is_svg_path { "SVG Path" } else { "Freehand" },
                                        ShapeKind::Text => "Text",
                                    };
                                    let vis_icon = if shape.is_visible { "👁" } else { "🚫" };
                                    let lock_icon = if shape.is_locked { "🔒" } else { "🔓" };
                                    let swatch_color = if shape.fill_color.a() > 0 {
                                        shape.fill_color
                                    } else if let Some(sc) = shape.stroke_color {
                                        sc
                                    } else {
                                        egui::Color32::BLACK
                                    };

                                    ui.horizontal(|ui| {
                                        ui.add_space(16.0);
                                        if ui.small_button(vis_icon).clicked() {
                                            actions.push(LayerAction::ToggleShapeVis(i));
                                        }
                                        if ui.small_button(lock_icon).clicked() {
                                            actions.push(LayerAction::ToggleShapeLock(i));
                                        }
                                        let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                                        ui.painter().rect_filled(rect, 2.0, swatch_color);
                                        ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.0_f32, egui::Color32::GRAY));

                                        if ui.selectable_label(is_sel, kind_label).clicked() {
                                            let shift = ui.input(|inp| inp.modifiers.shift);
                                            actions.push(LayerAction::SelectShape(i, shift));
                                        }
                                    });
                                }
                            }
                        }
                    });

                    // Apply actions
                    for action in actions {
                        match action {
                            LayerAction::ToggleExpand(gid) => {
                                if self.expanded_groups.contains(&gid) {
                                    self.expanded_groups.remove(&gid);
                                } else {
                                    self.expanded_groups.insert(gid);
                                }
                            }
                            LayerAction::ToggleGroupVis(gid, new_vis) => {
                                for s in &mut self.shapes {
                                    if s.group_id == Some(gid) {
                                        s.is_visible = new_vis;
                                    }
                                }
                            }
                            LayerAction::ToggleGroupLock(gid, new_lock) => {
                                for s in &mut self.shapes {
                                    if s.group_id == Some(gid) {
                                        s.is_locked = new_lock;
                                    }
                                }
                            }
                            LayerAction::SelectGroup(members, shift) => {
                                if shift {
                                    let any_sel = members.iter().any(|&m| self.is_selected(m));
                                    for m in &members {
                                        if any_sel {
                                            self.selected_indices.remove(m);
                                        } else {
                                            self.selected_indices.insert(*m);
                                        }
                                    }
                                } else {
                                    self.clear_selection();
                                    for m in &members {
                                        self.selected_indices.insert(*m);
                                    }
                                }
                                self.selected_index = members.first().copied();
                                self.sync_active_properties();
                            }
                            LayerAction::ToggleShapeVis(i) => {
                                if let Some(s) = self.shapes.get_mut(i) {
                                    s.is_visible = !s.is_visible;
                                }
                            }
                            LayerAction::ToggleShapeLock(i) => {
                                if let Some(s) = self.shapes.get_mut(i) {
                                    s.is_locked = !s.is_locked;
                                }
                            }
                            LayerAction::SelectShape(i, shift) => {
                                if shift {
                                    self.toggle_select_shape(i);
                                } else {
                                    self.select_shape(i, false);
                                }
                            }
                        }
                    }
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

            // Mat background and paper image (if loaded)
            painter.rect_filled(mat_rect, 2.0, egui::Color32::WHITE);
            if let Some(paper_tex) = &self.paper_texture {
                painter.image(
                    paper_tex.id(),
                    mat_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
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
                                s.is_visible && s.hit_test(mat_pos, hit_tol)
                            }).map(|(i, _)| i);
                            let is_shift = ctx.input(|i| i.modifiers.shift);
                            if let Some(idx) = hit {
                                if is_shift {
                                    self.toggle_select_shape(idx);
                                } else if !self.is_selected(idx) {
                                    self.select_shape(idx, false);
                                }
                                self.is_dragging_shape = !self.shapes[idx].is_locked;
                                self.marquee_start = None;
                            } else {
                                if !is_shift {
                                    self.clear_selection();
                                }
                                self.marquee_start = Some(mat_pos);
                                self.is_dragging_shape = false;
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
                            let delta_in = response.drag_delta() / px_per_in;
                            for idx in self.all_selected_indices() {
                                if let Some(shape) = self.shapes.get_mut(idx) {
                                    if !shape.is_locked {
                                        shape.translate(delta_in);
                                    }
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
                                stroke_color: None,
                                stroke_width: 0.0,
                                is_closed: false,
                                is_svg_path: false,
                                is_modified: false,
                                triangles: vec![],
                                group_id: None,
                                is_visible: true,
                                is_locked: false,
                            });
                            self.select_shape(self.shapes.len() - 1, false);
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

                if let Some(start) = self.marquee_start.take() {
                    let end = curr_mat.unwrap_or(start);
                    let min_x = start.x.min(end.x);
                    let min_y = start.y.min(end.y);
                    let max_x = start.x.max(end.x);
                    let max_y = start.y.max(end.y);
                    if (max_x - min_x) > 0.05 || (max_y - min_y) > 0.05 {
                        let box_rect = egui::Rect::from_min_max(egui::pos2(min_x, min_y), egui::pos2(max_x, max_y));
                        for (i, shape) in self.shapes.iter().enumerate() {
                            if shape.is_visible && shape.bounding_rect().intersects(box_rect) {
                                self.selected_indices.insert(i);
                                self.selected_index = Some(i);
                            }
                        }
                        self.sync_active_properties();
                    }
                }

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
                                    stroke_color: Some(egui::Color32::BLACK),
                                    stroke_width: 0.05,
                                    is_closed: true,
                                    is_svg_path: false,
                                    is_modified: false,
                                    triangles: vec![],
                                    group_id: None,
                                    is_visible: true,
                                    is_locked: false,
                                });
                                self.select_shape(self.shapes.len() - 1, false);
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
                                    stroke_color: Some(egui::Color32::BLACK),
                                    stroke_width: 0.05,
                                    is_closed: true,
                                    is_svg_path: false,
                                    is_modified: false,
                                    triangles: vec![],
                                    group_id: None,
                                    is_visible: true,
                                    is_locked: false,
                                });
                                self.select_shape(self.shapes.len() - 1, false);
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
                                    fill_color: egui::Color32::TRANSPARENT,
                                    stroke_color: Some(current_fill),
                                    stroke_width: 0.05,
                                    is_closed: false,
                                    is_svg_path: false,
                                    is_modified: false,
                                    triangles: vec![],
                                    group_id: None,
                                    is_visible: true,
                                    is_locked: false,
                                });
                                self.select_shape(self.shapes.len() - 1, false);
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
                        let mut shape = CanvasShape {
                            kind: ShapeKind::Freehand,
                            x: min_x,
                            y: min_y,
                            width: (max_x - min_x).max(0.05),
                            height: (max_y - min_y).max(0.05),
                            points: std::mem::take(&mut self.freehand_points),
                            text: String::new(),
                            fill_color: egui::Color32::TRANSPARENT,
                            stroke_color: Some(current_fill),
                            stroke_width: 0.05,
                            is_closed: false,
                            is_svg_path: false,
                            is_modified: false,
                            triangles: vec![],
                            group_id: None,
                            is_visible: true,
                            is_locked: false,
                        };
                        shape.precompute_triangles();
                        self.shapes.push(shape);
                        self.select_shape(self.shapes.len() - 1, false);
                    } else {
                        self.freehand_points.clear();
                    }
                }
            }

            // 0. Render cached SVGs (exact vector rendering with EvenOdd cutouts, gradients, layers bound to live group transforms)
            let mut active_cached_groups = std::collections::BTreeSet::new();
            for cached in &self.cached_svgs {
                if let Some(tex) = &cached.texture {
                    if let Some((min_x, min_y, max_x, max_y, count, any_visible)) = self.get_group_bounds(cached.group_id) {
                        if count == cached.orig_shape_count && any_visible {
                            active_cached_groups.insert(cached.group_id);
                            let live_w = (max_x - min_x).max(0.001);
                            let live_h = (max_y - min_y).max(0.001);
                            let scale_x = live_w / cached.init_width.max(0.001);
                            let scale_y = live_h / cached.init_height.max(0.001);
                            let live_svg_x = min_x - (cached.init_min_x * scale_x);
                            let live_svg_y = min_y - (cached.init_min_y * scale_y);
                            let live_svg_w = cached.orig_svg_width * scale_x;
                            let live_svg_h = cached.orig_svg_height * scale_y;

                            let screen_min = mat_to_screen(egui::pos2(live_svg_x, live_svg_y));
                            let screen_size = egui::vec2(live_svg_w * px_per_in, live_svg_h * px_per_in);
                            let screen_rect = egui::Rect::from_min_size(screen_min, screen_size);
                            painter.image(
                                tex.id(),
                                screen_rect,
                                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                        }
                    }
                }
            }

            // Render shapes over the grid using fill_color and stroke settings
            let black_stroke = egui::Stroke::new(2.0_f32, egui::Color32::BLACK);
            let sel_set = self.selected_indices.clone();
            let sel_single = self.selected_index;
            for (i, shape) in self.shapes.iter_mut().enumerate() {
                if !shape.is_visible {
                    continue;
                }
                let is_selected = sel_set.contains(&i) || sel_single == Some(i);

                let is_in_active_cached_group = shape.group_id.map_or(false, |gid| active_cached_groups.contains(&gid));
                if is_in_active_cached_group {
                    if is_selected {
                        // Highlight cut contour outline of selected shape over the texture
                        let highlight_stroke = egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(0, 150, 255));
                        if shape.points.len() >= 2 {
                            let screen_pts: Vec<egui::Pos2> = shape.points.iter().map(|&p| mat_to_screen(p)).collect();
                            if shape.is_closed {
                                let mut closed = screen_pts;
                                if let (Some(f), Some(l)) = (closed.first(), closed.last()) {
                                    if (*f - *l).length() > 0.5 { closed.push(*f); }
                                }
                                painter.add(egui::Shape::line(closed, highlight_stroke));
                            } else {
                                painter.add(egui::Shape::line(screen_pts, highlight_stroke));
                            }
                        }
                    }
                    continue;
                }

                let stroke_opt = shape.stroke_color.and_then(|c| {
                    if c.a() > 0 {
                        let sw = (shape.stroke_width * px_per_in).max(1.0);
                        Some(egui::Stroke::new(sw, c))
                    } else {
                        None
                    }
                });

                match shape.kind {
                    ShapeKind::Rectangle => {
                        let screen_r = egui::Rect::from_min_size(
                            mat_to_screen(egui::pos2(shape.x, shape.y)),
                            egui::vec2(shape.width * px_per_in, shape.height * px_per_in),
                        );
                        if shape.fill_color.a() > 0 {
                            painter.rect_filled(screen_r, 0.0, shape.fill_color);
                        }
                        if let Some(s) = stroke_opt {
                            painter.rect_stroke(screen_r, 0.0, s);
                        }
                    }
                    ShapeKind::Ellipse => {
                        let screen_min = mat_to_screen(egui::pos2(shape.x, shape.y));
                        let screen_size = egui::vec2(shape.width * px_per_in, shape.height * px_per_in);
                        let center = screen_min + screen_size * 0.5;
                        let rx = (screen_size.x * 0.5).abs();
                        let ry = (screen_size.y * 0.5).abs();
                        if rx > 0.5 && ry > 0.5 {
                            let n = 48;
                            let pts: Vec<egui::Pos2> = (0..n).map(|k| {
                                let angle = (k as f32) / (n as f32) * std::f32::consts::TAU;
                                egui::pos2(center.x + rx * angle.cos(), center.y + ry * angle.sin())
                            }).collect();
                            painter.add(egui::Shape::convex_polygon(
                                pts,
                                shape.fill_color,
                                stroke_opt.unwrap_or(egui::Stroke::NONE),
                            ));
                        }
                    }
                    ShapeKind::Line => {
                        if shape.points.len() >= 2 {
                            let p1 = mat_to_screen(shape.points[0]);
                            let p2 = mat_to_screen(shape.points[1]);
                            let s = stroke_opt.unwrap_or_else(|| {
                                if shape.fill_color.a() > 0 {
                                    egui::Stroke::new(2.0_f32, shape.fill_color)
                                } else {
                                    egui::Stroke::new(2.0_f32, egui::Color32::BLACK)
                                }
                            });
                            painter.line_segment([p1, p2], s);
                        }
                    }
                    ShapeKind::Freehand => {
                        if shape.points.len() >= 2 {
                            let screen_pts: Vec<egui::Pos2> = shape.points.iter().map(|&p| mat_to_screen(p)).collect();
                            if shape.is_closed || shape.fill_color.a() > 0 {
                                draw_filled_polygon(&painter, &screen_pts, shape.fill_color, stroke_opt, &shape.triangles);
                            } else if let Some(s) = stroke_opt {
                                painter.add(egui::Shape::line(screen_pts, s));
                            } else {
                                painter.add(egui::Shape::line(screen_pts, black_stroke));
                            }
                        } else if let Some(&p) = shape.points.first() {
                            let sp = mat_to_screen(p);
                            let col = shape.stroke_color.unwrap_or(shape.fill_color);
                            painter.circle_filled(sp, 2.0_f32, col);
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

            // Live marquee drag selection rectangle
            if let Some(start) = self.marquee_start {
                let curr_pos = response.interact_pointer_pos().or_else(|| ctx.input(|i| i.pointer.latest_pos()));
                if let Some(pos) = curr_pos {
                    let s_r = egui::Rect::from_two_pos(mat_to_screen(start), pos);
                    painter.rect_filled(s_r, 0.0, egui::Color32::from_rgba_unmultiplied(0, 150, 255, 30));
                    draw_dashed_rect(&painter, s_r, egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(0, 150, 255)), 5.0, 3.0);
                }
            }

            // Dashed bounding box for all selected shapes (combined bounds for groups/multi-selection)
            let sel_indices = self.all_selected_indices();
            if !sel_indices.is_empty() {
                let mut min_x = f32::MAX;
                let mut min_y = f32::MAX;
                let mut max_x = f32::MIN;
                let mut max_y = f32::MIN;
                let mut any_visible = false;
                for &idx in &sel_indices {
                    if let Some(shape) = self.shapes.get(idx) {
                        if shape.is_visible {
                            any_visible = true;
                            let r = shape.bounding_rect();
                            min_x = min_x.min(r.min.x);
                            min_y = min_y.min(r.min.y);
                            max_x = max_x.max(r.max.x);
                            max_y = max_y.max(r.max.y);
                        }
                    }
                }
                if any_visible && max_x > min_x && max_y > min_y {
                    let screen_min = mat_to_screen(egui::pos2(min_x, min_y));
                    let screen_size = egui::vec2((max_x - min_x) * px_per_in, (max_y - min_y) * px_per_in);
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
            stroke_color: Some(egui::Color32::BLACK),
            stroke_width: 0.05,
            is_closed: true,
            is_svg_path: false,
            is_modified: false,
            triangles: vec![],
            group_id: None,
            is_visible: true,
            is_locked: false,
        });
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Line,
            x: 0.0,
            y: 0.0,
            width: 5.0,
            height: 5.0,
            points: vec![egui::pos2(0.0, 0.0), egui::pos2(5.0, 5.0)],
            text: String::new(),
            fill_color: egui::Color32::TRANSPARENT,
            stroke_color: Some(egui::Color32::BLACK),
            stroke_width: 0.05,
            is_closed: false,
            is_svg_path: false,
            is_modified: false,
            triangles: vec![],
            group_id: None,
            is_visible: true,
            is_locked: false,
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
            stroke_color: Some(egui::Color32::BLACK),
            stroke_width: 0.05,
            is_closed: true,
            is_svg_path: false,
            is_modified: false,
            triangles: vec![],
            group_id: None,
            is_visible: true,
            is_locked: false,
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
            stroke_color: Some(egui::Color32::BLACK),
            stroke_width: 0.05,
            is_closed: true,
            is_svg_path: false,
            is_modified: false,
            triangles: vec![],
            group_id: None,
            is_visible: true,
            is_locked: false,
        });
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Line,
            x: 1.0,
            y: 5.0,
            width: 3.0,
            height: 0.0,
            points: vec![egui::pos2(1.0, 5.0), egui::pos2(4.0, 5.0)],
            text: String::new(),
            fill_color: egui::Color32::TRANSPARENT,
            stroke_color: Some(egui::Color32::BLACK),
            stroke_width: 0.05,
            is_closed: false,
            is_svg_path: false,
            is_modified: false,
            triangles: vec![],
            group_id: None,
            is_visible: true,
            is_locked: false,
        });
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Freehand,
            x: 2.0,
            y: 6.0,
            width: 2.0,
            height: 2.0,
            points: vec![egui::pos2(2.0, 6.0), egui::pos2(3.0, 7.0), egui::pos2(4.0, 6.0)],
            text: String::new(),
            fill_color: egui::Color32::TRANSPARENT,
            stroke_color: Some(egui::Color32::BLACK),
            stroke_width: 0.05,
            is_closed: false,
            is_svg_path: false,
            is_modified: false,
            triangles: vec![],
            group_id: None,
            is_visible: true,
            is_locked: false,
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
            stroke_color: None,
            stroke_width: 0.0,
            is_closed: false,
            is_svg_path: false,
            is_modified: false,
            triangles: vec![],
            group_id: None,
            is_visible: true,
            is_locked: false,
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
            stroke_color: Some(egui::Color32::BLACK),
            stroke_width: 0.05,
            is_closed: true,
            is_svg_path: false,
            is_modified: false,
            triangles: vec![],
            group_id: None,
            is_visible: true,
            is_locked: false,
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
            stroke_color: Some(egui::Color32::BLACK),
            stroke_width: 0.05,
            is_closed: true,
            is_svg_path: false,
            is_modified: false,
            triangles: vec![],
            group_id: None,
            is_visible: true,
            is_locked: false,
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
            stroke_color: Some(egui::Color32::BLACK),
            stroke_width: 0.05,
            is_closed: true,
            is_svg_path: false,
            is_modified: false,
            triangles: vec![],
            group_id: None,
            is_visible: true,
            is_locked: false,
        });
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Line,
            x: 1.0,
            y: 1.0,
            width: 2.0,
            height: 2.0,
            points: vec![egui::pos2(1.0, 1.0), egui::pos2(3.0, 3.0)],
            text: String::new(),
            fill_color: egui::Color32::TRANSPARENT,
            stroke_color: Some(egui::Color32::BLACK),
            stroke_width: 0.05,
            is_closed: false,
            is_svg_path: false,
            is_modified: false,
            triangles: vec![],
            group_id: None,
            is_visible: true,
            is_locked: false,
        });
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Freehand,
            x: 0.0,
            y: 0.0,
            width: 2.0,
            height: 2.0,
            points: vec![egui::pos2(0.0, 0.0), egui::pos2(1.0, 2.0), egui::pos2(2.0, 1.0)],
            text: String::new(),
            fill_color: egui::Color32::from_rgb(0, 0, 0),
            stroke_color: Some(egui::Color32::BLACK),
            stroke_width: 0.05,
            is_closed: false,
            is_svg_path: false,
            is_modified: false,
            triangles: vec![],
            group_id: None,
            is_visible: true,
            is_locked: false,
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

    #[test]
    fn test_import_complex_layered_svg() {
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"12in\" height=\"12in\" viewBox=\"0 0 1152 1152\">\
            <style>\
                .red-fill { fill: #e6194b; }\
                .no-stroke { stroke: none; }\
            </style>\
            <!-- Layer 1: Background banner with group transform -->\
            <g transform=\"translate(96, 96) scale(1.0)\">\
                <path class=\"red-fill no-stroke\" d=\"M 0 0 C 96 192, 192 192, 288 0 C 384 192, 480 192, 576 0 L 576 384 L 0 384 Z\" />\
            </g>\
            <!-- Layer 2: Foreground detail with nested transform and CSS stroke -->\
            <g transform=\"translate(192, 192)\">\
                <g transform=\"rotate(0)\">\
                    <path d=\"M 0 0 L 192 192 L 0 192 Z\" fill=\"#3cb44b\" stroke=\"#000075\" stroke-width=\"9.6\" />\
                </g>\
            </g>\
        </svg>";

        let shapes = import_svg_str(svg, egui::Color32::GRAY);
        assert_eq!(shapes.len(), 2);

        // Layer 1
        assert_eq!(shapes[0].kind, ShapeKind::Freehand);
        assert_eq!(shapes[0].fill_color, egui::Color32::from_rgb(0xe6, 0x19, 0x4b));
        // Verify stroke is None because of stroke: none
        assert_eq!(shapes[0].stroke_color, None);
        assert!(shapes[0].is_closed);
        // Bounds in inches (96px = 1.0in, 576px = 6.0in)
        assert!((shapes[0].x - 1.0).abs() < 0.05);
        assert!((shapes[0].y - 1.0).abs() < 0.05);
        assert!((shapes[0].width - 6.0).abs() < 0.05);

        // Layer 2
        assert_eq!(shapes[1].kind, ShapeKind::Freehand);
        assert_eq!(shapes[1].fill_color, egui::Color32::from_rgb(0x3c, 0xb4, 0x4b));
        assert_eq!(shapes[1].stroke_color, Some(egui::Color32::from_rgb(0x00, 0x00, 0x75)));
        // 9.6px / 96 = 0.1in stroke width
        assert!((shapes[1].stroke_width - 0.1).abs() < 0.01);
        assert!((shapes[1].x - 2.0).abs() < 0.05);
        assert!((shapes[1].y - 2.0).abs() < 0.05);
    }

    #[test]
    fn test_cached_svg_rendering() {
        let mut app = StudioApp::new(None);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="4in" height="4in" viewBox="0 0 400 400">
            <rect x="50" y="50" width="300" height="300" fill="#ff5500" />
            <circle cx="200" cy="200" r="80" fill="#00aaff" />
        </svg>"##;
        let count = app.import_svg_bytes(svg.as_bytes()).expect("import failed");
        assert_eq!(count, 2);
        assert_eq!(app.cached_svgs.len(), 1);
        assert!(app.cached_svgs[0].pixmap.width() > 0);
        assert!(app.cached_svgs[0].pixmap.height() > 0);
        assert_eq!(app.shapes.len(), 2);
        assert!(app.shapes[0].is_svg_path);
        assert!(app.shapes[1].is_svg_path);
    }
    #[test]
    fn test_grouping_and_ungrouping() {
        let mut app = StudioApp::new(None);
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Rectangle,
            x: 1.0, y: 1.0, width: 2.0, height: 2.0, points: vec![],
            text: String::new(), fill_color: egui::Color32::RED, stroke_color: None, stroke_width: 0.0,
            is_closed: true, is_svg_path: false, is_modified: false, triangles: vec![],
            group_id: None, is_visible: true, is_locked: false,
        });
        app.shapes.push(CanvasShape {
            kind: ShapeKind::Ellipse,
            x: 4.0, y: 4.0, width: 2.0, height: 2.0, points: vec![],
            text: String::new(), fill_color: egui::Color32::BLUE, stroke_color: None, stroke_width: 0.0,
            is_closed: true, is_svg_path: false, is_modified: false, triangles: vec![],
            group_id: None, is_visible: true, is_locked: false,
        });

        app.select_shape(0, false);
        app.select_shape(1, true); // multi-select
        assert_eq!(app.all_selected_indices().len(), 2);

        app.group_selected();
        assert!(app.shapes[0].group_id.is_some());
        assert_eq!(app.shapes[0].group_id, app.shapes[1].group_id);

        // Selecting shape 0 should now automatically select shape 1 via group_id
        app.clear_selection();
        app.select_shape(0, false);
        assert_eq!(app.all_selected_indices().len(), 2);

        // Ungroup
        app.ungroup_selected();
        assert!(app.shapes[0].group_id.is_none());
        assert!(app.shapes[1].group_id.is_none());
    }

    #[test]
    fn test_layer_reordering() {
        let mut app = StudioApp::new(None);
        for i in 0..3 {
            app.shapes.push(CanvasShape {
                kind: ShapeKind::Rectangle,
                x: i as f32, y: 0.0, width: 1.0, height: 1.0, points: vec![],
                text: format!("Shape {}", i), fill_color: egui::Color32::RED, stroke_color: None, stroke_width: 0.0,
                is_closed: true, is_svg_path: false, is_modified: false, triangles: vec![],
                group_id: None, is_visible: true, is_locked: false,
            });
        }
        // Initial order: Shape 0, Shape 1, Shape 2
        app.select_shape(0, false);
        app.bring_to_front();
        assert_eq!(app.shapes[2].text, "Shape 0");

        app.send_to_back();
        assert_eq!(app.shapes[0].text, "Shape 0");

        app.bring_forward();
        assert_eq!(app.shapes[1].text, "Shape 0");

        app.send_backward();
        assert_eq!(app.shapes[0].text, "Shape 0");
    }

    #[test]
    fn test_svg_import_grouped() {
        let mut app = StudioApp::new(None);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="4in" height="4in" viewBox="0 0 400 400">
            <rect x="10" y="10" width="100" height="100" fill="red" />
            <circle cx="200" cy="200" r="50" fill="blue" />
        </svg>"##;
        app.import_svg_bytes(svg.as_bytes()).unwrap();
        assert_eq!(app.shapes.len(), 2);
        assert!(app.shapes[0].group_id.is_some());
        assert_eq!(app.shapes[0].group_id, app.shapes[1].group_id);
    }

    #[test]
    fn test_cached_svg_dynamic_bounds() {
        let mut app = StudioApp::new(None);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="4in" height="4in" viewBox="0 0 400 400">
            <rect x="10" y="10" width="100" height="100" fill="red" />
            <circle cx="200" cy="200" r="50" fill="blue" />
        </svg>"##;
        app.import_svg_bytes(svg.as_bytes()).unwrap();
        assert_eq!(app.cached_svgs.len(), 1);
        let gid = app.cached_svgs[0].group_id;

        let bounds_before = app.get_group_bounds(gid).expect("group bounds found");
        let (min_x1, min_y1, max_x1, max_y1, count1, vis1) = bounds_before;
        assert_eq!(count1, 2);
        assert!(vis1);

        // Translate the shapes (e.g. user drags the group by 3 inches right, 2 inches down)
        let delta = egui::vec2(3.0, 2.0);
        for s in &mut app.shapes {
            if s.group_id == Some(gid) {
                s.translate(delta);
            }
        }

        let bounds_after = app.get_group_bounds(gid).expect("group bounds found");
        let (min_x2, min_y2, max_x2, max_y2, count2, vis2) = bounds_after;
        assert_eq!(count2, 2);
        assert!(vis2);
        assert!((min_x2 - (min_x1 + 3.0)).abs() < 0.001);
        assert!((min_y2 - (min_y1 + 2.0)).abs() < 0.001);
        assert!((max_x2 - (max_x1 + 3.0)).abs() < 0.001);
        assert!((max_y2 - (max_y1 + 2.0)).abs() < 0.001);
    }

    #[test]
    fn test_group_by_color() {
        let mut app = StudioApp::new(None);
        // Create 2 red shapes and 2 blue shapes
        for i in 0..4 {
            let col = if i < 2 { egui::Color32::RED } else { egui::Color32::BLUE };
            app.shapes.push(CanvasShape {
                kind: ShapeKind::Rectangle,
                x: i as f32, y: 0.0, width: 1.0, height: 1.0, points: vec![],
                text: format!("Shape {}", i), fill_color: col, stroke_color: None, stroke_width: 0.0,
                is_closed: true, is_svg_path: false, is_modified: false, triangles: vec![],
                group_id: None, is_visible: true, is_locked: false,
            });
        }

        app.group_by_color();

        let gid_red_0 = app.shapes[0].group_id;
        let gid_red_1 = app.shapes[1].group_id;
        let gid_blue_0 = app.shapes[2].group_id;
        let gid_blue_1 = app.shapes[3].group_id;

        assert!(gid_red_0.is_some());
        assert_eq!(gid_red_0, gid_red_1);

        assert!(gid_blue_0.is_some());
        assert_eq!(gid_blue_0, gid_blue_1);

        assert_ne!(gid_red_0, gid_blue_0);
    }
}