use std::path::{Path, PathBuf};
use egui::{Pos2, Rect};
use image::RgbaImage;

/// Bounding rectangle in image pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub min_x: u32,
    pub min_y: u32,
    pub max_x: u32,
    pub max_y: u32,
}

impl PixelRect {
    pub fn new(min_x: u32, min_y: u32, max_x: u32, max_y: u32) -> Self {
        Self { min_x, min_y, max_x, max_y }
    }

    pub fn width(&self) -> u32 {
        self.max_x.saturating_sub(self.min_x) + 1
    }

    pub fn height(&self) -> u32 {
        self.max_y.saturating_sub(self.min_y) + 1
    }

    pub fn to_egui_rect(&self) -> Rect {
        Rect::from_min_max(
            Pos2::new(self.min_x as f32, self.min_y as f32),
            Pos2::new(self.max_x as f32, self.max_y as f32),
        )
    }

    pub fn from_egui_rect(rect: Rect, img_w: u32, img_h: u32) -> Self {
        let min_x = (rect.min.x.round() as i64).clamp(0, img_w.saturating_sub(1) as i64) as u32;
        let min_y = (rect.min.y.round() as i64).clamp(0, img_h.saturating_sub(1) as i64) as u32;
        let max_x = (rect.max.x.round() as i64).clamp(min_x as i64, img_w.saturating_sub(1) as i64) as u32;
        let max_y = (rect.max.y.round() as i64).clamp(min_y as i64, img_h.saturating_sub(1) as i64) as u32;
        Self { min_x, min_y, max_x, max_y }
    }
}

/// Drag handle types for interactive crop box editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CropDragHandle {
    Move,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Top,
    Bottom,
    Left,
    Right,
}

/// Tracing parameters for vtracer
#[derive(Debug, Clone)]
pub struct TracingParams {
    pub color_count: usize,
    pub remove_white_background: bool,
    pub crop_rect: PixelRect,
}

impl Default for TracingParams {
    fn default() -> Self {
        Self {
            color_count: 8,
            remove_white_background: true,
            crop_rect: PixelRect::new(0, 0, 0, 0),
        }
    }
}

/// Interactive state for the image tracer in the UI
pub struct ImageTracerState {
    pub is_active: bool,
    pub source_path: Option<PathBuf>,
    pub image_data: Option<RgbaImage>,
    pub texture: Option<egui::TextureHandle>,
    pub crop_rect: Rect, // In image pixel coordinates
    pub auto_crop_rect: Rect,
    pub color_count: usize,
    pub remove_white_background: bool,
    pub active_drag_handle: Option<CropDragHandle>,
    pub drag_start_mouse: Pos2,
    pub drag_start_rect: Rect,
}

impl Default for ImageTracerState {
    fn default() -> Self {
        Self {
            is_active: false,
            source_path: None,
            image_data: None,
            texture: None,
            crop_rect: Rect::NOTHING,
            auto_crop_rect: Rect::NOTHING,
            color_count: 8,
            remove_white_background: true,
            active_drag_handle: None,
            drag_start_mouse: Pos2::ZERO,
            drag_start_rect: Rect::NOTHING,
        }
    }
}

impl ImageTracerState {
    /// Load an image from disk and initialize tracing state
    pub fn load_image(&mut self, path: &Path, ctx: &egui::Context) -> Result<(), Box<dyn std::error::Error>> {
        let dyn_img = image::open(path)?;
        let rgba = dyn_img.to_rgba8();
        let (w, h) = rgba.dimensions();

        let auto_crop = auto_crop_catalog_card(&rgba);
        let egui_crop = auto_crop.to_egui_rect();

        let color_img = egui::ColorImage::from_rgba_unmultiplied(
            [w as usize, h as usize],
            rgba.as_raw(),
        );
        let texture = ctx.load_texture("catalog_card_trace", color_img, egui::TextureOptions::LINEAR);

        self.is_active = true;
        self.source_path = Some(path.to_path_buf());
        self.crop_rect = egui_crop;
        self.auto_crop_rect = egui_crop;
        self.texture = Some(texture);
        self.image_data = Some(rgba);
        self.active_drag_handle = None;

        Ok(())
    }

    /// Reset crop box to the auto-calculated artwork crop rectangle
    pub fn snap_to_auto_crop(&mut self) {
        if self.auto_crop_rect.is_positive() {
            self.crop_rect = self.auto_crop_rect;
        }
    }

    /// Reset crop box to the full image
    pub fn reset_to_full_image(&mut self) {
        if let Some(ref img) = self.image_data {
            let (w, h) = img.dimensions();
            self.crop_rect = Rect::from_min_max(
                Pos2::ZERO,
                Pos2::new(w.saturating_sub(1) as f32, h.saturating_sub(1) as f32),
            );
        }
    }

    /// Close the tracing session
    pub fn close(&mut self) {
        self.is_active = false;
        self.source_path = None;
        self.image_data = None;
        self.texture = None;
        self.crop_rect = Rect::NOTHING;
        self.auto_crop_rect = Rect::NOTHING;
        self.active_drag_handle = None;
    }

    /// Hit-test mouse position against crop box handles and interior.
    pub fn hit_test_handle(&self, mouse_pos: Pos2, crop_screen: Rect) -> Option<CropDragHandle> {
        let r = 10.0;
        let edge_tol = 8.0;

        // Check corners first
        if mouse_pos.distance(crop_screen.left_top()) <= r {
            return Some(CropDragHandle::TopLeft);
        }
        if mouse_pos.distance(crop_screen.right_top()) <= r {
            return Some(CropDragHandle::TopRight);
        }
        if mouse_pos.distance(crop_screen.left_bottom()) <= r {
            return Some(CropDragHandle::BottomLeft);
        }
        if mouse_pos.distance(crop_screen.right_bottom()) <= r {
            return Some(CropDragHandle::BottomRight);
        }

        // Check edges
        if (mouse_pos.y - crop_screen.top()).abs() <= edge_tol
            && mouse_pos.x >= crop_screen.left() - edge_tol
            && mouse_pos.x <= crop_screen.right() + edge_tol
        {
            return Some(CropDragHandle::Top);
        }
        if (mouse_pos.y - crop_screen.bottom()).abs() <= edge_tol
            && mouse_pos.x >= crop_screen.left() - edge_tol
            && mouse_pos.x <= crop_screen.right() + edge_tol
        {
            return Some(CropDragHandle::Bottom);
        }
        if (mouse_pos.x - crop_screen.left()).abs() <= edge_tol
            && mouse_pos.y >= crop_screen.top() - edge_tol
            && mouse_pos.y <= crop_screen.bottom() + edge_tol
        {
            return Some(CropDragHandle::Left);
        }
        if (mouse_pos.x - crop_screen.right()).abs() <= edge_tol
            && mouse_pos.y >= crop_screen.top() - edge_tol
            && mouse_pos.y <= crop_screen.bottom() + edge_tol
        {
            return Some(CropDragHandle::Right);
        }

        // Inside crop box
        if crop_screen.contains(mouse_pos) {
            return Some(CropDragHandle::Move);
        }

        None
    }

    pub fn cursor_for_handle(handle: CropDragHandle) -> egui::CursorIcon {
        match handle {
            CropDragHandle::TopLeft | CropDragHandle::BottomRight => egui::CursorIcon::ResizeNwSe,
            CropDragHandle::TopRight | CropDragHandle::BottomLeft => egui::CursorIcon::ResizeNeSw,
            CropDragHandle::Top | CropDragHandle::Bottom => egui::CursorIcon::ResizeVertical,
            CropDragHandle::Left | CropDragHandle::Right => egui::CursorIcon::ResizeHorizontal,
            CropDragHandle::Move => egui::CursorIcon::Move,
        }
    }

    /// Update crop box during mouse dragging
    pub fn update_drag(&mut self, current_mouse: Pos2, img_screen_rect: Rect, img_w: u32, img_h: u32) {
        if let Some(handle) = self.active_drag_handle {
            if img_screen_rect.width() <= 0.0 || img_screen_rect.height() <= 0.0 {
                return;
            }
            let delta = current_mouse - self.drag_start_mouse;
            let sx = img_w as f32 / img_screen_rect.width();
            let sy = img_h as f32 / img_screen_rect.height();
            let dx = delta.x * sx;
            let dy = delta.y * sy;
            let orig = self.drag_start_rect;

            match handle {
                CropDragHandle::Move => {
                    let w = orig.width();
                    let h = orig.height();
                    let min_x = (orig.min.x + dx).clamp(0.0, (img_w as f32 - w).max(0.0));
                    let min_y = (orig.min.y + dy).clamp(0.0, (img_h as f32 - h).max(0.0));
                    self.crop_rect = Rect::from_min_size(Pos2::new(min_x, min_y), orig.size());
                }
                CropDragHandle::TopLeft => {
                    let min_x = (orig.min.x + dx).clamp(0.0, orig.max.x - 10.0);
                    let min_y = (orig.min.y + dy).clamp(0.0, orig.max.y - 10.0);
                    self.crop_rect = Rect::from_min_max(Pos2::new(min_x, min_y), orig.max);
                }
                CropDragHandle::TopRight => {
                    let max_x = (orig.max.x + dx).clamp(orig.min.x + 10.0, img_w as f32);
                    let min_y = (orig.min.y + dy).clamp(0.0, orig.max.y - 10.0);
                    self.crop_rect = Rect::from_min_max(Pos2::new(orig.min.x, min_y), Pos2::new(max_x, orig.max.y));
                }
                CropDragHandle::BottomLeft => {
                    let min_x = (orig.min.x + dx).clamp(0.0, orig.max.x - 10.0);
                    let max_y = (orig.max.y + dy).clamp(orig.min.y + 10.0, img_h as f32);
                    self.crop_rect = Rect::from_min_max(Pos2::new(min_x, orig.min.y), Pos2::new(orig.max.x, max_y));
                }
                CropDragHandle::BottomRight => {
                    let max_x = (orig.max.x + dx).clamp(orig.min.x + 10.0, img_w as f32);
                    let max_y = (orig.max.y + dy).clamp(orig.min.y + 10.0, img_h as f32);
                    self.crop_rect = Rect::from_min_max(orig.min, Pos2::new(max_x, max_y));
                }
                CropDragHandle::Top => {
                    let min_y = (orig.min.y + dy).clamp(0.0, orig.max.y - 10.0);
                    self.crop_rect = Rect::from_min_max(Pos2::new(orig.min.x, min_y), orig.max);
                }
                CropDragHandle::Bottom => {
                    let max_y = (orig.max.y + dy).clamp(orig.min.y + 10.0, img_h as f32);
                    self.crop_rect = Rect::from_min_max(orig.min, Pos2::new(orig.max.x, max_y));
                }
                CropDragHandle::Left => {
                    let min_x = (orig.min.x + dx).clamp(0.0, orig.max.x - 10.0);
                    self.crop_rect = Rect::from_min_max(Pos2::new(min_x, orig.min.y), orig.max);
                }
                CropDragHandle::Right => {
                    let max_x = (orig.max.x + dx).clamp(orig.min.x + 10.0, img_w as f32);
                    self.crop_rect = Rect::from_min_max(orig.min, Pos2::new(max_x, orig.max.y));
                }
            }
        }
    }
}

// ----------------------------------------------------------------------------
// Heuristic Auto-Cropper Algorithm for "Original Overlays" Catalog Cards
// ----------------------------------------------------------------------------

/// Determines if a pixel matches an olive-green, brown, or dark physical outer frame.
pub fn is_outer_frame_pixel(r: u8, g: u8, b: u8) -> bool {
    let lum = 0.299 * (r as f32) + 0.587 * (g as f32) + 0.114 * (b as f32);
    if lum > 190.0 {
        return false; // Definitely too bright to be olive or brown frame
    }
    // Olive green: green component dominant, blue low
    let is_olive = g > b.saturating_add(10) && g >= r.saturating_sub(25) && lum < 170.0;
    // Brown: red component dominant over blue and green
    let is_brown = r > b.saturating_add(20) && r >= g && lum < 160.0;
    // Dark border/shadow
    let is_dark = lum < 100.0;

    is_olive || is_brown || is_dark
}

/// Determines if a pixel represents the white / off-white paper base of the card.
pub fn is_card_base_pixel(r: u8, g: u8, b: u8) -> bool {
    let lum = 0.299 * (r as f32) + 0.587 * (g as f32) + 0.114 * (b as f32);
    let max_c = r.max(g).max(b);
    let min_c = r.min(g).min(b);
    let spread = max_c - min_c;

    // Card paper is bright with low saturation
    lum > 155.0 && spread < 45 && !is_outer_frame_pixel(r, g, b)
}

/// Determines if a pixel inside the card artwork area is high-contrast artwork
/// (dark cutlines, colored fills, text, or illustrations).
pub fn is_artwork_pixel(r: u8, g: u8, b: u8) -> bool {
    let lum = 0.299 * (r as f32) + 0.587 * (g as f32) + 0.114 * (b as f32);
    let max_c = r.max(g).max(b);
    let min_c = r.min(g).min(b);
    let spread = max_c - min_c;

    // Dark lines/shading, or distinct colored fills (red/blue/etc.)
    lum < 165.0 || spread > 32
}

/// Algorithmic auto-cropper tailored for "Original Overlays" catalog cards:
/// a) Ignores olive-green/brown physical outer frames.
/// b) Detects the bounds of the central white rectangle (the card base).
/// c) Masks out/ignores top ~15% containing header text (e.g., "V.120 Original Overlays...").
/// d) Masks out/ignores bottom-right corner containing the price tag (e.g., "$4.00", "$5.00").
/// e) Calculates a tight bounding box around the remaining high-contrast central artwork.
pub fn auto_crop_catalog_card(img: &RgbaImage) -> PixelRect {
    let (width, height) = img.dimensions();
    if width < 10 || height < 10 {
        return PixelRect::new(0, 0, width.saturating_sub(1), height.saturating_sub(1));
    }

    // Step a & b: Detect the central white rectangle (card base)
    let mut row_card_counts = vec![0u32; height as usize];
    let mut col_card_counts = vec![0u32; width as usize];

    for y in 0..height {
        for x in 0..width {
            let p = img.get_pixel(x, y);
            if p[3] > 64 && is_card_base_pixel(p[0], p[1], p[2]) {
                row_card_counts[y as usize] += 1;
                col_card_counts[x as usize] += 1;
            }
        }
    }

    // A row or column is part of the card if it contains a significant amount of white card paper
    let row_thresh = (width as f32 * 0.15).max(10.0) as u32;
    let col_thresh = (height as f32 * 0.15).max(10.0) as u32;

    let card_min_y = row_card_counts.iter().position(|&c| c >= row_thresh).unwrap_or(0) as u32;
    let card_max_y = row_card_counts.iter().rposition(|&c| c >= row_thresh).unwrap_or(height as usize - 1) as u32;

    let card_min_x = col_card_counts.iter().position(|&c| c >= col_thresh).unwrap_or(0) as u32;
    let card_max_x = col_card_counts.iter().rposition(|&c| c >= col_thresh).unwrap_or(width as usize - 1) as u32;

    let card_w = card_max_x.saturating_sub(card_min_x) + 1;
    let card_h = card_max_y.saturating_sub(card_min_y) + 1;

    // Step c: Mask out top ~15% containing header text
    let header_height = ((card_h as f32) * 0.15).round() as u32;
    let art_search_min_y = card_min_y + header_height;

    // Small boundary margin so we don't catch paper cut edges
    let edge_margin_x = ((card_w as f32) * 0.02).round().max(2.0) as u32;
    let edge_margin_y = ((card_h as f32) * 0.02).round().max(2.0) as u32;
    let art_search_min_x = card_min_x + edge_margin_x;
    let art_search_max_x = card_max_x.saturating_sub(edge_margin_x);
    let art_search_max_y = card_max_y.saturating_sub(edge_margin_y);

    // Step d: Mask out bottom-right corner containing the price tag
    // Bottom ~20% of card height, right ~28% of card width
    let price_w = ((card_w as f32) * 0.28).round() as u32;
    let price_h = ((card_h as f32) * 0.20).round() as u32;
    let price_mask_min_x = card_max_x.saturating_sub(price_w);
    let price_mask_min_y = card_max_y.saturating_sub(price_h);

    // Step e: Calculate a tight bounding box around the remaining high-contrast central artwork
    let mut art_min_x = u32::MAX;
    let mut art_min_y = u32::MAX;
    let mut art_max_x = 0u32;
    let mut art_max_y = 0u32;
    let mut art_pixel_count = 0u32;

    for y in art_search_min_y..=art_search_max_y {
        for x in art_search_min_x..=art_search_max_x {
            // Mask out price tag corner
            if x >= price_mask_min_x && y >= price_mask_min_y {
                continue;
            }

            let p = img.get_pixel(x, y);
            if p[3] > 64 && is_artwork_pixel(p[0], p[1], p[2]) {
                art_min_x = art_min_x.min(x);
                art_min_y = art_min_y.min(y);
                art_max_x = art_max_x.max(x);
                art_max_y = art_max_y.max(y);
                art_pixel_count += 1;
            }
        }
    }

    // If no artwork detected or very sparse noise, fall back to the central valid region
    if art_pixel_count < 8 || art_min_x > art_max_x || art_min_y > art_max_y {
        return PixelRect::new(
            art_search_min_x,
            art_search_min_y,
            art_search_max_x,
            art_search_max_y,
        );
    }

    // Add modest padding (8 pixels) around artwork, clamped to the card search area
    let padding = 8u32;
    let final_min_x = art_min_x.saturating_sub(padding).max(art_search_min_x);
    let final_min_y = art_min_y.saturating_sub(padding).max(art_search_min_y);
    let final_max_x = (art_max_x + padding).min(art_search_max_x);
    let final_max_y = (art_max_y + padding).min(art_search_max_y);

    PixelRect::new(final_min_x, final_min_y, final_max_x, final_max_y)
}

// ----------------------------------------------------------------------------
// vtracer Vectorization Pipeline
// ----------------------------------------------------------------------------

/// Trace a cropped region of an image into SVG markup using `vtracer`.
pub fn trace_image_to_svg(
    img: &RgbaImage,
    params: &TracingParams,
) -> Result<String, Box<dyn std::error::Error>> {
    let crop = params.crop_rect;
    let w = crop.width();
    let h = crop.height();
    if w == 0 || h == 0 {
        return Err("Crop region has zero width or height".into());
    }

    // Crop the image strictly to the defined crop rectangle
    let mut cropped = image::imageops::crop_imm(img, crop.min_x, crop.min_y, w, h).to_image();

    // Iterate through cropped image buffer and apply strict binary threshold:
    // Convert to grayscale/luminance: if below 128.0, force to pure #000000 ([0, 0, 0, 255]),
    // otherwise force to pure #ffffff ([255, 255, 255, 255]).
    // The image must contain ONLY exactly two hex colors (#000000 and #ffffff).
    for pixel in cropped.pixels_mut() {
        let r = pixel[0] as f32;
        let g = pixel[1] as f32;
        let b = pixel[2] as f32;
        let a = pixel[3];
        let lum = 0.299 * r + 0.587 * g + 0.114 * b;
        if a >= 64 && lum < 128.0 {
            *pixel = image::Rgba([0, 0, 0, 255]);
        } else {
            *pixel = image::Rgba([255, 255, 255, 255]);
        }
    }

    // Configure vtracer with aggressive parameters
    let mut cfg = vtracer::Config::from_preset(vtracer::Preset::Bw);
    cfg.hierarchical = vtracer::config::Hierarchical::Stacked;
    cfg.color_precision = 8;
    cfg.filter_speckle = 14;
    cfg.max_colors = Some(2);
    cfg.corner_threshold = 60;
    cfg.length_threshold = 4.0;
    cfg.splice_threshold = 45;

    let pipeline = cfg.build()?;
    let color_img = vtracer::ColorImage {
        pixels: cropped.into_raw(),
        width: w as usize,
        height: h as usize,
    };

    let svg = pipeline.to_svg(&color_img)?;
    Ok(svg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn test_is_outer_frame_pixel() {
        // Olive green
        assert!(is_outer_frame_pixel(110, 125, 60));
        // Brown
        assert!(is_outer_frame_pixel(130, 80, 50));
        // Dark frame
        assert!(is_outer_frame_pixel(30, 30, 30));
        // White paper (should NOT be frame)
        assert!(!is_outer_frame_pixel(245, 245, 240));
    }

    #[test]
    fn test_is_card_base_pixel() {
        assert!(is_card_base_pixel(250, 250, 248));
        assert!(is_card_base_pixel(230, 228, 225));
        // Dark artwork or frame should not be card base
        assert!(!is_card_base_pixel(30, 30, 30));
        assert!(!is_card_base_pixel(110, 125, 60));
    }

    #[test]
    fn test_is_artwork_pixel() {
        // Black cutline
        assert!(is_artwork_pixel(20, 20, 20));
        // Red cutline
        assert!(is_artwork_pixel(220, 30, 30));
        // White background should not be artwork
        assert!(!is_artwork_pixel(250, 250, 250));
    }

    #[test]
    fn test_auto_crop_catalog_card_synthetic() {
        // Create a 200x200 synthetic catalog card image:
        // - 0..20: olive frame (color [110, 125, 60, 255])
        // - 20..180: white card base (color [245, 245, 245, 255])
        // - Top 15% of card (y: 20..44): header text (e.g. black text [0, 0, 0, 255])
        // - Bottom-right of card (x: 140..180, y: 150..180): price tag [0, 0, 0, 255]
        // - Central artwork (x: 80..120, y: 80..120): artwork [200, 20, 20, 255]
        let width = 200u32;
        let height = 200u32;
        let mut img = RgbaImage::new(width, height);

        for y in 0..height {
            for x in 0..width {
                if x < 20 || x >= 180 || y < 20 || y >= 180 {
                    // Olive frame
                    img.put_pixel(x, y, Rgba([110, 125, 60, 255]));
                } else {
                    // White card base
                    img.put_pixel(x, y, Rgba([245, 245, 245, 255]));
                }
            }
        }

        // Add header text in top 15% of card (y in 25..35, x in 40..160)
        for y in 25..35 {
            for x in 40..160 {
                img.put_pixel(x, y, Rgba([20, 20, 20, 255]));
            }
        }

        // Add price tag in bottom-right (y in 155..170, x in 150..170)
        for y in 155..170 {
            for x in 150..170 {
                img.put_pixel(x, y, Rgba([20, 20, 20, 255]));
            }
        }

        // Add central artwork (x in 80..120, y in 80..120)
        for y in 80..120 {
            for x in 80..120 {
                img.put_pixel(x, y, Rgba([200, 20, 20, 255]));
            }
        }

        let crop = auto_crop_catalog_card(&img);

        // The auto crop MUST:
        // 1. Exclude the olive frame (min_x >= 20, min_y >= 20, max_x <= 180, max_y <= 180)
        // 2. Exclude the top header (min_y > 40)
        // 3. Exclude the bottom-right price tag
        // 4. Bound the central artwork tightly around 80..120 (plus padding)
        assert!(crop.min_x >= 20, "min_x {} should be inside card", crop.min_x);
        assert!(crop.min_y > 40, "min_y {} should ignore header", crop.min_y);
        assert!(crop.max_x < 150, "max_x {} should ignore price tag", crop.max_x);
        assert!(crop.max_y < 155, "max_y {} should ignore price tag", crop.max_y);

        // Should tightly encompass the artwork (80..120 with ~8px padding)
        assert!(crop.min_x <= 80);
        assert!(crop.min_y <= 80);
        assert!(crop.max_x >= 120);
        assert!(crop.max_y >= 120);
    }

    #[test]
    fn test_vtracer_vectorization_pipeline() {
        let width = 64u32;
        let height = 64u32;
        let mut img = RgbaImage::new(width, height);

        // White background with a red square in the middle
        for y in 0..height {
            for x in 0..width {
                if x >= 16 && x < 48 && y >= 16 && y < 48 {
                    img.put_pixel(x, y, Rgba([255, 0, 0, 255]));
                } else {
                    img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
                }
            }
        }

        let params = TracingParams {
            color_count: 4,
            remove_white_background: true,
            crop_rect: PixelRect::new(10, 10, 54, 54),
        };

        let svg = trace_image_to_svg(&img, &params).expect("vtracer trace should succeed");
        assert!(svg.contains("<svg"), "Output must contain <svg");
        assert!(svg.contains("</svg>"), "Output must contain </svg>");
        assert!(svg.contains("path"), "Output must contain vector paths");
    }
}
