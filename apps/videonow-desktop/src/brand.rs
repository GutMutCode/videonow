//! The VideoNow mark and colours.

static MARK: &[u8] = include_bytes!("../../../assets/brand/videonow-mark.png");

/// The application icon.
pub fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(MARK).unwrap_or_default()
}

/// The mark as a texture for the top bar.
pub fn mark_texture(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory_with_format(MARK, image::ImageFormat::Png).ok()?.to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let color = egui::ColorImage::from_rgba_unmultiplied([w, h], img.as_raw());
    Some(ctx.load_texture("videonow-mark", color, egui::TextureOptions::LINEAR))
}

/// The accent colour of the workspace tabs and the agent's entries.
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x4f, 0x8c, 0xff);
/// Background of the shell's bars.
pub const BAR: egui::Color32 = egui::Color32::from_rgb(0x1b, 0x1d, 0x22);
pub const BAR_LINE: egui::Color32 = egui::Color32::from_rgb(0x2c, 0x2f, 0x36);
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(0xe6, 0xe8, 0xec);
pub const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(0x9a, 0xa0, 0xab);
pub const WARN: egui::Color32 = egui::Color32::from_rgb(0xf2, 0xb1, 0x34);
pub const AGENT: egui::Color32 = egui::Color32::from_rgb(0xb3, 0x86, 0xff);
pub const HUMAN: egui::Color32 = egui::Color32::from_rgb(0x5c, 0xc8, 0x8e);
