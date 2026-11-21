use eframe::egui;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let screenshot_path = args.windows(2).find(|w| w[0] == "--screenshot").map(|w| w[1].clone());
    let mut viewport = egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]).with_title("Silhouette Studio");
    if screenshot_path.is_some() { viewport = viewport.with_active(false); }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("Silhouette Studio", options, Box::new(move |_cc| Box::new(StudioApp::new(screenshot_path))))
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Tool {
    Select,
    Line,
    Rectangle,
    Ellipse,
    Text,
    Freehand,
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
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

impl CanvasShape {
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
            });
        });

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

            // 2. Render shapes over the grid using fill_color and 2px black stroke
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

            // 3. Dashed bounding box for selected shape
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
    if let Some(parent) = std::path::Path::new(path).parent() {
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
