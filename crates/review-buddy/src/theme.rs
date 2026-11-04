//! GitHub Primer colors, light and dark, and the diff's metrics.

use egui::Color32;

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub canvas: Color32,
    pub subtle: Color32,
    pub border: Color32,
    pub muted_border: Color32,
    pub text: Color32,
    pub muted_text: Color32,
    pub accent: Color32,
    pub add_bg: Color32,
    pub add_num_bg: Color32,
    pub add_word: Color32,
    pub del_bg: Color32,
    pub del_num_bg: Color32,
    pub del_word: Color32,
    pub hunk_bg: Color32,
    pub hunk_num_bg: Color32,
    pub empty_bg: Color32,
    pub selected_bg: Color32,
    pub selected_num_bg: Color32,
    pub text_selection: Color32,
    pub add_marker: Color32,
    pub del_marker: Color32,
    pub ai_accent: Color32,
    pub ai_bg: Color32,
    pub pending_bg: Color32,
    pub attention: Color32,
    pub success: Color32,
    pub syn_keyword: Color32,
    pub syn_type: Color32,
    pub syn_string: Color32,
    pub syn_number: Color32,
    pub syn_comment: Color32,
    pub syn_function: Color32,
    pub syn_tag: Color32,
}

fn hex(h: u32) -> Color32 {
    Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
}

/// `fg` at `alpha` pre-composited over `bg` (opaque fills are cheaper and match Primer).
fn over(fg: u32, alpha: f32, bg: u32) -> Color32 {
    let f = hex(fg);
    let b = hex(bg);
    let mix = |x: u8, y: u8| (x as f32 * alpha + y as f32 * (1.0 - alpha)).round() as u8;
    Color32::from_rgb(mix(f.r(), b.r()), mix(f.g(), b.g()), mix(f.b(), b.b()))
}

impl Palette {
    pub fn new(dark: bool) -> Palette {
        let canvas = if dark { 0x0D1117 } else { 0xFFFFFF };
        let d = |light: u32, dark_c: u32, la: f32, da: f32| {
            if dark { over(dark_c, da, canvas) } else { over(light, la, canvas) }
        };
        let s = |light: u32, dark_c: u32| if dark { hex(dark_c) } else { hex(light) };
        Palette {
            dark,
            canvas: hex(canvas),
            subtle: s(0xF6F8FA, 0x151B23),
            border: s(0xD1D9E0, 0x3D444D),
            muted_border: d(0xD1D9E0, 0x3D444D, 0.7, 0.7),
            text: s(0x1F2328, 0xF0F6FC),
            muted_text: s(0x59636E, 0x9198A1),
            accent: s(0x0969DA, 0x4493F8),
            add_bg: d(0xDAFBE1, 0x2EA043, 0.55, 0.15),
            add_num_bg: d(0xACEEBB, 0x3FB950, 0.55, 0.3),
            add_word: d(0xACEEBB, 0x2EA043, 1.0, 0.4),
            del_bg: d(0xFFEBE9, 0xF85149, 1.0, 0.1),
            del_num_bg: d(0xFFCECB, 0xF85149, 0.75, 0.3),
            del_word: d(0xFF8182, 0xF85149, 0.4, 0.4),
            hunk_bg: d(0xDDF4FF, 0x388BFD, 1.0, 0.1),
            hunk_num_bg: d(0xB6E3FF, 0x388BFD, 1.0, 0.25),
            empty_bg: d(0xF6F8FA, 0x151B23, 1.0, 0.6),
            selected_bg: d(0xFFF8C5, 0xBB8009, 1.0, 0.15),
            selected_num_bg: d(0xFFF1A6, 0xBB8009, 1.0, 0.3),
            text_selection: if dark {
                Color32::from_rgba_unmultiplied(0x38, 0x8B, 0xFD, 90)
            } else {
                Color32::from_rgba_unmultiplied(0x09, 0x69, 0xDA, 56)
            },
            add_marker: s(0x1A7F37, 0x3FB950),
            del_marker: s(0xD1242F, 0xF85149),
            ai_accent: s(0x8250DF, 0xAB7DF8),
            ai_bg: d(0xFBEFFF, 0xAB7DF8, 1.0, 0.1),
            pending_bg: d(0xFFF8C5, 0xBB8009, 1.0, 0.15),
            attention: s(0x9A6700, 0xD29922),
            success: s(0x1A7F37, 0x3FB950),
            syn_keyword: s(0xCF222E, 0xFF7B72),
            syn_type: s(0x953800, 0xFFA657),
            syn_string: s(0x0A3069, 0xA5D6FF),
            syn_number: s(0x0550AE, 0x79C0FF),
            syn_comment: s(0x59636E, 0x9198A1),
            syn_function: s(0x6639BA, 0xD2A8FF),
            syn_tag: s(0x116329, 0x7EE787),
        }
    }

    pub fn current(ctx: &egui::Context) -> Palette {
        Palette::new(ctx.global_style().visuals.dark_mode)
    }
}

/// Row heights and widths shared by drawing and layout. Monospaced, so widths are arithmetic.
#[derive(Clone, Copy, Debug)]
pub struct DiffMetrics {
    pub font_size: f32,
    pub char_width: f32,
    pub line_height: f32,
    pub file_header_height: f32,
    pub hunk_header_height: f32,
    pub placeholder_height: f32,
    pub marker_width: f32,
}

impl DiffMetrics {
    pub fn new(ctx: &egui::Context, font_size: f32) -> DiffMetrics {
        let font = egui::FontId::monospace(font_size);
        let char_width = ctx.fonts_mut(|f| f.glyph_width(&font, 'M'));
        DiffMetrics {
            font_size,
            char_width,
            line_height: (font_size * 1.65).ceil(),
            file_header_height: 38.0,
            hunk_header_height: 24.0,
            placeholder_height: 64.0,
            marker_width: 22.0,
        }
    }

    pub fn font(&self) -> egui::FontId {
        egui::FontId::monospace(self.font_size)
    }

    /// Width of a line-number column for numbers up to `max_line`.
    pub fn number_column_width(&self, max_line: i64) -> f32 {
        let digits = max_line.to_string().len().max(3);
        digits as f32 * self.char_width + 20.0
    }
}
