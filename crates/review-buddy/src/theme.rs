//! Colors in the spirit of Zed's One Dark / One Light themes (values from Zed's
//! `assets/themes/one/one.json`), egui visuals built from them, and the diff's metrics.

use egui::{Color32, CornerRadius, Stroke};

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    /// The editor surface: the diff, cards' insides, text fields.
    pub canvas: Color32,
    /// Side panels (inbox, file tree, Discuss).
    pub panel: Color32,
    /// Title bar and status bar.
    pub chrome: Color32,
    /// Popovers, modals, menus.
    pub elevated: Color32,
    /// Filled controls and file headers (`element.background`).
    pub subtle: Color32,
    pub hover: Color32,
    pub active: Color32,
    /// The selected segment / tab / row.
    pub selected_control: Color32,
    pub border: Color32,
    /// Hairlines between panels and rows (`border.variant`).
    pub muted_border: Color32,
    pub focus_border: Color32,
    pub text: Color32,
    pub muted_text: Color32,
    pub placeholder: Color32,
    pub accent: Color32,
    pub accent_bg: Color32,
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
    pub attention: Color32,
    pub success: Color32,
    pub syn_keyword: Color32,
    pub syn_type: Color32,
    pub syn_string: Color32,
    pub syn_number: Color32,
    pub syn_comment: Color32,
    pub syn_function: Color32,
    pub syn_tag: Color32,
    /// Corner radius of controls; cards and popovers use `radius + 2`.
    pub radius: u8,
}

fn hex(h: u32) -> Color32 {
    Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
}

/// `fg` at `alpha` pre-composited over `bg` (opaque fills are cheaper and predictable).
pub fn over(fg: Color32, alpha: f32, bg: Color32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 * alpha + y as f32 * (1.0 - alpha)).round() as u8;
    Color32::from_rgb(mix(fg.r(), bg.r()), mix(fg.g(), bg.g()), mix(fg.b(), bg.b()))
}

impl Palette {
    pub fn new(dark: bool) -> Palette {
        if dark { Self::one_dark() } else { Self::one_light() }
    }

    fn one_dark() -> Palette {
        let canvas = hex(0x282C33);
        let (created, deleted, info, modified, purple) = (hex(0xA1C181), hex(0xD07277), hex(0x74ADE8), hex(0xDEC184), hex(0xB477CF));
        Palette {
            dark: true,
            canvas,
            panel: hex(0x2F343E),
            chrome: hex(0x3B414D),
            elevated: hex(0x2F343E),
            subtle: hex(0x2E343E),
            hover: hex(0x363C46),
            active: hex(0x454A56),
            selected_control: hex(0x454A56),
            border: hex(0x464B57),
            muted_border: hex(0x363C46),
            focus_border: hex(0x47679E),
            text: hex(0xDCE0E5),
            muted_text: hex(0xA9AFBC),
            placeholder: hex(0x878A98),
            accent: info,
            accent_bg: over(info, 0.16, hex(0x2F343E)),
            add_bg: over(created, 0.10, canvas),
            add_num_bg: over(created, 0.16, canvas),
            add_word: over(created, 0.28, canvas),
            del_bg: over(deleted, 0.10, canvas),
            del_num_bg: over(deleted, 0.16, canvas),
            del_word: over(deleted, 0.30, canvas),
            hunk_bg: over(info, 0.08, canvas),
            hunk_num_bg: over(info, 0.14, canvas),
            empty_bg: over(hex(0x2F343E), 0.7, canvas),
            selected_bg: over(modified, 0.10, canvas),
            selected_num_bg: over(modified, 0.20, canvas),
            text_selection: Color32::from_rgba_unmultiplied(0x74, 0xAD, 0xE8, 0x3D),
            add_marker: created,
            del_marker: deleted,
            ai_accent: purple,
            ai_bg: over(purple, 0.09, canvas),
            attention: modified,
            success: created,
            syn_keyword: purple,
            syn_type: hex(0x6EB4BF),
            syn_string: created,
            syn_number: hex(0xBF956A),
            syn_comment: hex(0x5D636F),
            syn_function: hex(0x73ADE9),
            syn_tag: info,
            radius: 4,
        }
    }

    fn one_light() -> Palette {
        let canvas = hex(0xFAFAFA);
        let (created, deleted, info, modified, purple) = (hex(0x669F59), hex(0xD36151), hex(0x5C78E2), hex(0xA48819), hex(0xA449AB));
        Palette {
            dark: false,
            canvas,
            panel: hex(0xEBEBEC),
            chrome: hex(0xDCDCDD),
            elevated: hex(0xEBEBEC),
            subtle: hex(0xEBEBEC),
            hover: hex(0xDFDFE0),
            active: hex(0xCACACA),
            selected_control: canvas,
            border: hex(0xC9C9CA),
            muted_border: hex(0xDFDFE0),
            focus_border: hex(0x7D82E8),
            text: hex(0x242529),
            muted_text: hex(0x58585A),
            placeholder: hex(0x7E8086),
            accent: info,
            accent_bg: hex(0xE2E2FA),
            add_bg: hex(0xE9F1E6),
            add_num_bg: hex(0xDFEADB),
            add_word: hex(0xC8DCC1),
            del_bg: hex(0xFCEAE6),
            del_num_bg: hex(0xFBDFD9),
            del_word: hex(0xF6C6BD),
            hunk_bg: hex(0xEDEDFB),
            hunk_num_bg: hex(0xE2E2FA),
            empty_bg: hex(0xF2F2F3),
            selected_bg: hex(0xFAF2E6),
            selected_num_bg: over(modified, 0.22, canvas),
            text_selection: Color32::from_rgba_unmultiplied(0x5C, 0x78, 0xE2, 0x3D),
            add_marker: created,
            del_marker: deleted,
            ai_accent: purple,
            ai_bg: over(purple, 0.07, canvas),
            attention: modified,
            success: created,
            syn_keyword: purple,
            syn_type: hex(0x3882B7),
            syn_string: hex(0x649F57),
            syn_number: hex(0xAD6E25),
            syn_comment: hex(0xA2A3A7),
            syn_function: hex(0x5B79E3),
            syn_tag: info,
            radius: 4,
        }
    }

    pub fn current(ctx: &egui::Context) -> Palette {
        Palette::new(ctx.global_style().visuals.dark_mode)
    }

    /// Tinted button colors (Zed's "tinted" style): a faint fill of `c`, a stronger border,
    /// and `c` as the text color.
    pub fn tinted(&self, c: Color32) -> (Color32, Color32, Color32) {
        if self.dark {
            (over(c, 0.16, self.panel), over(c, 0.45, self.panel), c)
        } else {
            (over(c, 0.12, self.canvas), over(c, 0.45, self.canvas), c)
        }
    }

    /// egui's visuals for this palette. Every widget state has the same 1 px stroke width
    /// and no expansion, so nothing changes size when hovered, pressed or opened.
    pub fn visuals(&self) -> egui::Visuals {
        let mut v = if self.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        let r = CornerRadius::same(self.radius);
        v.panel_fill = self.panel;
        v.window_fill = self.elevated;
        v.window_stroke = Stroke::new(1.0, self.border);
        v.window_corner_radius = CornerRadius::same(self.radius + 4);
        v.menu_corner_radius = CornerRadius::same(self.radius + 2);
        v.extreme_bg_color = self.canvas;
        v.text_edit_bg_color = Some(self.canvas);
        v.faint_bg_color = self.subtle;
        v.code_bg_color = self.subtle;
        v.hyperlink_color = self.accent;
        v.warn_fg_color = self.attention;
        v.error_fg_color = self.del_marker;
        v.selection.bg_fill = self.text_selection;
        v.selection.stroke = Stroke::new(1.0, self.accent);
        v.text_cursor.stroke = Stroke::new(2.0, self.accent);
        v.window_shadow = egui::epaint::Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(if self.dark { 90 } else { 40 }) };
        v.popup_shadow = egui::epaint::Shadow { offset: [0, 4], blur: 12, spread: 0, color: Color32::from_black_alpha(if self.dark { 80 } else { 30 }) };
        let w = &mut v.widgets;
        let state = |bg: Color32, stroke: Color32, fg: Color32| egui::style::WidgetVisuals {
            bg_fill: bg,
            weak_bg_fill: bg,
            bg_stroke: Stroke::new(1.0, stroke),
            fg_stroke: Stroke::new(1.0, fg),
            corner_radius: r,
            expansion: 0.0,
        };
        w.noninteractive = state(self.panel, self.muted_border, self.text);
        w.inactive = state(self.subtle, Color32::TRANSPARENT, self.text);
        w.hovered = state(self.hover, Color32::TRANSPARENT, self.text);
        w.active = state(self.active, Color32::TRANSPARENT, self.text);
        w.open = state(self.active, Color32::TRANSPARENT, self.text);
        v
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
