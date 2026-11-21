use eframe::egui;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let screenshot_path = args.windows(2).find(|w| w[0] == "--screenshot").map(|w| w[1].clone());
    let mut viewport = egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]).with_title("Silhouette Studio");
    if screenshot_path.is_some() { viewport = viewport.with_active(false); }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("Silhouette Studio", options, Box::new(move |_cc| Box::new(StudioApp::new(screenshot_path))))
}

#[derive(PartialEq, Clone, Copy)]
enum Tool { Select, Line, Rectangle, Ellipse, Text, Freehand }

struct StudioApp {
    active_tool: Tool,
    right_panel: &'static str,
    zoom: f32,
    pan: egui::Vec2,
    mat_width_in: f32,
    mat_height_in: f32,
    fill_color: [f32; 3],
    text_sample: String,
    screenshot_path: Option<String>,
    frames_rendered: u32,
}

impl StudioApp {
    fn new(screenshot_path: Option<String>) -> Self {
        Self { active_tool: Tool::Select, right_panel: "Page Setup", zoom: 1.0, pan: egui::Vec2::ZERO, mat_width_in: 12.0, mat_height_in: 12.0, fill_color: [0.85, 0.2, 0.35], text_sample: "Sample Design".to_string(), screenshot_path, frames_rendered: 0 }
    }
}

impl eframe::App for StudioApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.screenshot_path.is_some() {
            ctx.request_repaint();
            self.frames_rendered += 1;
            if self.frames_rendered == 60 { ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot); }
            if self.frames_rendered > 120 { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
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
        egui::TopBottomPanel::top("top_menu").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New Design").clicked() { ui.close_menu(); }
                    if ui.button("Quit").clicked() { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
                });
                ui.menu_button("Edit", |ui| {
                    if ui.button("Reset View").clicked() { self.zoom = 1.0; self.pan = egui::Vec2::ZERO; ui.close_menu(); }
                });
                ui.menu_button("View", |ui| {
                    if ui.button("Zoom In").clicked() { self.zoom = (self.zoom * 1.2).min(5.0); ui.close_menu(); }
                    if ui.button("Zoom Out").clicked() { self.zoom = (self.zoom / 1.2).max(0.25); ui.close_menu(); }
                });
                ui.separator();
                ui.label(format!("Zoom: {:.0}%", self.zoom * 100.0));
            });
        });
        egui::SidePanel::left("left_tools").default_width(110.0).show(ctx, |ui| {
            ui.heading("Tools");
            ui.separator();
            ui.selectable_value(&mut self.active_tool, Tool::Select, "Select");
            ui.selectable_value(&mut self.active_tool, Tool::Line, "Line");
            ui.selectable_value(&mut self.active_tool, Tool::Rectangle, "Rectangle");
            ui.selectable_value(&mut self.active_tool, Tool::Ellipse, "Ellipse");
            ui.selectable_value(&mut self.active_tool, Tool::Text, "Text");
            ui.selectable_value(&mut self.active_tool, Tool::Freehand, "Freehand");
        });
        egui::SidePanel::right("right_panels").default_width(240.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for tab in ["Page Setup", "Fill", "Text", "Transform"] {
                    if ui.selectable_label(self.right_panel == tab, tab).clicked() { self.right_panel = tab; }
                }
            });
            ui.separator();
            match self.right_panel {
                "Page Setup" => {
                    ui.heading("Page Setup");
                    ui.add(egui::Slider::new(&mut self.mat_width_in, 4.0..=24.0).text("Width (in)"));
                    ui.add(egui::Slider::new(&mut self.mat_height_in, 4.0..=24.0).text("Height (in)"));
                }
                "Fill" => { ui.heading("Color Fill"); ui.color_edit_button_rgb(&mut self.fill_color); }
                "Text" => { ui.heading("Text Style"); ui.text_edit_singleline(&mut self.text_sample); }
                _ => { ui.heading("Transform"); ui.label("Select an object on the mat to transform."); }
            }
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            let (response, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
            let rect = response.rect;
            if response.dragged() { self.pan += response.drag_delta(); }
            if response.hovered() {
                let scroll = ctx.input(|i| i.raw_scroll_delta.y);
                if scroll != 0.0 { self.zoom = (self.zoom * (1.0 + scroll * 0.001)).clamp(0.25, 5.0); }
            }
            painter.rect_filled(rect, 0.0, egui::Color32::from_gray(42));
            let px_per_in = 40.0 * self.zoom;
            let mat_size = egui::vec2(self.mat_width_in * px_per_in, self.mat_height_in * px_per_in);
            let mat_min = rect.center() - mat_size * 0.5 + self.pan;
            let mat_rect = egui::Rect::from_min_size(mat_min, mat_size);
            painter.rect_filled(mat_rect, 2.0, egui::Color32::WHITE);
            painter.rect_stroke(mat_rect, 2.0, egui::Stroke::new(2.0, egui::Color32::from_rgb(0, 150, 214)));
            let grid_stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(225));
            for c in 1..(self.mat_width_in.ceil() as i32) {
                let x = mat_rect.min.x + (c as f32) * px_per_in;
                painter.line_segment([egui::pos2(x, mat_rect.min.y), egui::pos2(x, mat_rect.max.y)], grid_stroke);
            }
            for r in 1..(self.mat_height_in.ceil() as i32) {
                let y = mat_rect.min.y + (r as f32) * px_per_in;
                painter.line_segment([egui::pos2(mat_rect.min.x, y), egui::pos2(mat_rect.max.x, y)], grid_stroke);
            }
        });
    }
}

fn save_png(path: &str, image: &egui::ColorImage) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = std::path::Path::new(path).parent() { std::fs::create_dir_all(parent)?; }
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
