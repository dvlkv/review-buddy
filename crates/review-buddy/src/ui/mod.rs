//! egui views: window layout, toolbar, keyboard, sheets.

pub mod avatars;
pub mod blocks;
pub mod common;
pub mod diff_canvas;
pub mod discuss_panel;
pub mod file_tree_view;
pub mod header;
pub mod icons;
#[cfg(test)]
mod layout_tests;
pub mod markdown;
pub mod overview;
pub mod script;
pub mod settings;
pub mod sidebar;

use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use egui::{Key, RichText};
use review_buddy_core::diff::layout::DiffViewMode;
use review_buddy_core::persistence::{AppSettings, LocalStore};
use review_buddy_core::review::keybindings::{KeyBindings, KeyStroke, ReviewAction};
use review_buddy_core::support::synthetic::Shape;
use review_buddy_core::*;

use crate::app::AppState;
use crate::demo::DemoGitHub;
use crate::pr_store::{LoadState, TaskState};
use crate::rt::{Job, Spawner};
use crate::services::Services;
use crate::theme::{DiffMetrics, Palette};
use common::{gh_button, pal, BtnKind};

/// View state that isn't part of any store.
#[derive(Default)]
pub struct ViewState {
    pub canvases: HashMap<PRRef, diff_canvas::CanvasState>,
    pub trees: HashMap<PRRef, file_tree_view::FileTreeState>,
    pub open_url_text: String,
    pub open_url_error: Option<String>,
    pub palette_query: String,
    pub palette_selected: usize,
    pub review_event: Option<ReviewEvent>,
    pub settings: settings::SettingsState,
    pub discuss: discuss_panel::DiscussPanelState,
    pub overview: overview::OverviewState,
}

pub struct ReviewBuddyApp {
    pub state: AppState,
    pub view: ViewState,
    jobs: Receiver<Job>,
    metrics: Option<DiffMetrics>,
    snapshot: Option<(String, u32)>,
    script: Option<script::ScriptRunner>,
    was_focused: bool,
}

impl ReviewBuddyApp {
    pub fn new(cc: &eframe::CreationContext<'_>, args: Vec<String>) -> Self {
        let ctx = cc.egui_ctx.clone();
        install_fonts(&ctx);
        install_style(&ctx);
        let (spawner, jobs) = Spawner::new(Some(ctx));
        let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
        let synthetic = args.iter().any(|a| a == "--synthetic");
        let services = if synthetic {
            let shape = match arg("--synthetic").as_deref() {
                Some("huge") => Shape::HUGE,
                Some("small") => Shape::SMALL,
                _ => Shape::LARGE,
            };
            let root = std::env::temp_dir().join(format!("rb-synthetic-{}", new_id()));
            Services::new(Arc::new(DemoGitHub::synthetic(shape, 25)), Arc::new(LocalStore::new(root)), AppSettings::default(), None, None)
        } else {
            Services::live()
        };
        let mut state = AppState::new(Arc::new(services), spawner);
        if synthetic {
            state.refresh_inbox();
            state.open(PRRef::new("demo", "synthetic", 1));
        } else {
            // Load the inbox alongside any PR opened at launch, not after it.
            state.refresh_inbox();
            if let Some(r) = arg("--open").and_then(|u| PRRef::parse(&u)) {
                state.open(r);
            }
        }
        // `--snapshot <path>`: save a PNG of the window after it settles, then quit (for checks).
        let snapshot = arg("--snapshot").map(|p| (p, 0));
        let script = if let Some(out) = arg("--perf-run") {
            Some(script::ScriptRunner::perf(out.into(), arg("--perf-only").as_deref()))
        } else {
            arg("--ui-script").map(|s| script::ScriptRunner::script(&s, arg("--snap-dir").map(Into::into).unwrap_or_else(std::env::temp_dir)))
        };
        if script.is_some() {
            cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1600.0, 1000.0)));
        }
        ReviewBuddyApp { state, view: ViewState::default(), jobs, metrics: None, snapshot, script, was_focused: true }
    }

    fn drain_jobs(&mut self) {
        while let Ok(j) = self.jobs.try_recv() {
            j(&mut self.state);
        }
    }
}

/// Keeps scroll bars clear of a resizable panel edge: the edge's grab zone reaches a few
/// points into the neighbors, and a press there must resize, not jump a scroll bar.
const EDGE_GAP: f32 = 6.0;

/// A left panel's frame, with `EDGE_GAP` before its resizable right edge.
fn edge_frame(p: &Palette) -> egui::Frame {
    egui::Frame::new().fill(p.panel).inner_margin(egui::Margin { right: EDGE_GAP as i8, ..Default::default() })
}

/// Zed-like visuals and spacing, for both the light and the dark style.
fn install_style(ctx: &egui::Context) {
    // Several things re-lay a frame out before it's shown (a new inline block, a table
    // learning its columns, a panel whose content grew). They can chain within one frame,
    // so allow a few passes; idle frames still take one (the layout sweep checks).
    ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::new(4).unwrap());
    ctx.all_styles_mut(|s| {
        s.visuals = Palette::new(s.visuals.dark_mode).visuals();
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
        s.spacing.menu_margin = egui::Margin::same(4);
        s.spacing.window_margin = egui::Margin::same(12);
        s.spacing.interact_size.y = 22.0;
        // Thin scroll bars like Zed's, shown on hover. They get their own strip instead of
        // overlaying content: a press on an overlaid track (the last row of a table, the
        // right edge of a list) would jump the scroll position.
        s.spacing.scroll = egui::style::ScrollStyle::floating();
        s.spacing.scroll.bar_width = 8.0;
        s.spacing.scroll.floating_allocated_width = 10.0;
        s.interaction.resize_grab_radius_side = 4.0;
    });
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // The system UI font and SF Mono, so text matches other macOS apps. Fallbacks come next
    // (Apple's symbol and Unicode fonts) and egui's bundled fonts last, so a missing glyph
    // never switches the whole run to a font that looks different.
    let load = |path: &str| std::fs::read(path).ok().map(|b| Arc::new(egui::FontData::from_owned(b)));
    let mut prop: Vec<String> = Vec::new();
    let mut mono: Vec<String> = Vec::new();
    if let Some(d) = load("/System/Library/Fonts/SFNS.ttf").or_else(|| load("/System/Library/Fonts/Helvetica.ttc")) {
        fonts.font_data.insert("ui".into(), d);
        prop.push("ui".into());
    }
    if let Some(d) = load("/System/Library/Fonts/SFNSMono.ttf").or_else(|| load("/System/Library/Fonts/Menlo.ttc")) {
        fonts.font_data.insert("mono".into(), d);
        mono.push("mono".into());
    }
    for (name, path) in [("symbols", "/System/Library/Fonts/Apple Symbols.ttf"), ("unicode", "/Library/Fonts/Arial Unicode.ttf")] {
        if let Some(d) = load(path) {
            fonts.font_data.insert(name.into(), d);
            prop.push(name.into());
            mono.push(name.into());
        }
    }
    for (fam, mut first) in [(egui::FontFamily::Proportional, prop), (egui::FontFamily::Monospace, mono)] {
        let list = fonts.families.entry(fam).or_default();
        first.append(list);
        *list = first;
    }
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(|s| {
        use egui::{FontFamily::*, FontId, TextStyle};
        s.text_styles = [
            (TextStyle::Small, FontId::new(11.0, Proportional)),
            (TextStyle::Body, FontId::new(13.0, Proportional)),
            (TextStyle::Button, FontId::new(13.0, Proportional)),
            (TextStyle::Heading, FontId::new(18.0, Proportional)),
            (TextStyle::Monospace, FontId::new(12.0, Monospace)),
        ]
        .into();
    });
}

impl eframe::App for ReviewBuddyApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn on_exit(&mut self) {
        self.shutdown();
    }
}

impl ReviewBuddyApp {
    /// An app on the synthetic demo PR, without eframe, for layout tests.
    #[cfg(test)]
    pub fn headless(ctx: &egui::Context, shape: Shape) -> Self {
        install_fonts(ctx);
        install_style(ctx);
        let (spawner, jobs) = Spawner::inline();
        let root = std::env::temp_dir().join(format!("rb-headless-{}", new_id()));
        let services = Services::new(Arc::new(DemoGitHub::synthetic(shape, 25)), Arc::new(LocalStore::new(root)), AppSettings::default(), None, None);
        let mut state = AppState::new(Arc::new(services), spawner);
        state.refresh_inbox();
        state.open(PRRef::new("demo", "synthetic", 1));
        ReviewBuddyApp { state, view: ViewState::default(), jobs, metrics: None, snapshot: None, script: None, was_focused: true }
    }

    /// One frame of the whole window.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let frame_start = std::time::Instant::now();
        self.drain_jobs();
        self.drive_script(&ui.ctx().clone());
        self.state.tick();
        let ctx = ui.ctx().clone();
        let font_size = self.state.services.settings().font_size as f32;
        if self.metrics.map(|m| m.font_size != font_size).unwrap_or(true) {
            self.metrics = Some(DiffMetrics::new(&ctx, font_size.clamp(9.0, 24.0)));
        }
        let metrics = self.metrics.unwrap();
        // App became active: refresh what may have changed elsewhere.
        let focused = ctx.input(|i| i.focused);
        if focused && !self.was_focused {
            self.state.refresh_inbox_if_stale();
            if let Some(s) = self.state.current_pr_mut() {
                s.check_for_updates(false);
            }
        }
        self.was_focused = focused;
        // Keep timers (inbox refresh, update checks) ticking while idle.
        ctx.request_repaint_after(std::time::Duration::from_secs(5));

        self.keyboard(&ctx);
        let p = Palette::current(&ctx);
        egui::Panel::top("toolbar").exact_size(38.0).frame(egui::Frame::new().fill(p.chrome).inner_margin(egui::Margin::symmetric(8, 0))).show(ui, |ui| {
            self.toolbar(ui);
        });
        egui::Panel::bottom("status").exact_size(28.0).frame(egui::Frame::new().fill(p.chrome).inner_margin(egui::Margin::symmetric(6, 0))).show(ui, |ui| {
            self.status_bar(ui);
        });
        // Side panels never squeeze the diff below CENTRAL_MIN (overlapping panels put one
        // panel's scroll bar under another's resize handle). Each panel's maximum width
        // leaves room for the panels still to come; if even their minimums don't fit, the
        // inbox steps aside until the window is wider.
        const CENTRAL_MIN: f32 = 360.0;
        const INBOX: (f32, f32) = (220.0, 420.0);
        const FILES: (f32, f32) = (180.0, 420.0);
        const DISCUSS: (f32, f32) = (300.0, 640.0);
        let current = self.state.current.clone();
        let show_files = current.as_ref().is_some_and(|r| self.state.prs.get(r).is_some_and(|s| s.diff.is_some()));
        let show_discuss = self.state.ui.show_discuss_panel;
        let later = |files: bool, discuss: bool| (if files { FILES.0 } else { 0.0 }) + (if discuss { DISCUSS.0 } else { 0.0 }) + CENTRAL_MIN;
        let max_for = |ui: &egui::Ui, (min, max): (f32, f32), after: f32| (ui.available_width() - after).clamp(min, max);
        let show_inbox = self.state.ui.show_sidebar && ui.available_width() >= INBOX.0 + later(show_files, show_discuss);
        if show_inbox {
            let max = max_for(ui, INBOX, later(show_files, show_discuss));
            egui::Panel::left("inbox").frame(edge_frame(&p)).resizable(true).default_size(280.0f32.min(max)).size_range(INBOX.0..=max).show(ui, |ui| {
                sidebar::sidebar(ui, &mut self.state);
            });
        }
        if let Some(r) = &current {
            if show_files {
                let max = max_for(ui, FILES, later(false, show_discuss));
                egui::Panel::left("files").frame(edge_frame(&p)).resizable(true).default_size(250.0f32.min(max)).size_range(FILES.0..=max).show(ui, |ui| {
                    let tree = self.view.trees.entry(r.clone()).or_default();
                    if let Some(s) = self.state.prs.get_mut(r) {
                        file_tree_view::file_tree(ui, s, tree);
                    }
                });
            }
        }
        if show_discuss {
            let max = max_for(ui, DISCUSS, CENTRAL_MIN);
            egui::Panel::right("discuss").resizable(true).default_size(380.0f32.min(max)).size_range(DISCUSS.0..=max).show(ui, |ui| {
                match current.as_ref().and_then(|r| self.state.prs.get_mut(r)) {
                    Some(s) => discuss_panel::discuss_panel(ui, s, &mut self.view.discuss),
                    None => {
                        ui.label("No pull request open.");
                    }
                }
            });
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(p.canvas).inner_margin(egui::Margin { right: EDGE_GAP as i8, ..Default::default() })).show(ui, |ui| {
            self.detail(ui, &metrics);
        });
        self.sheets(&ctx);
        self.take_snapshot(&ctx);
        if let Some(r) = &mut self.script {
            r.record_frame(frame_start.elapsed().as_secs_f64() * 1000.0);
        }
    }

    fn shutdown(&mut self) {
        self.state.services.store.flush();
        for s in self.state.prs.values() {
            for c in s.controllers.values() {
                c.shutdown();
            }
        }
    }
}

impl ReviewBuddyApp {
    fn take_snapshot(&mut self, ctx: &egui::Context) {
        let Some((path, frames)) = self.snapshot.clone() else { return };
        let loaded = self.state.current_pr().map(|s| s.load_state == LoadState::Loaded).unwrap_or(true);
        if frames < 30 || !loaded {
            self.snapshot = Some((path, frames + 1));
            ctx.request_repaint();
            return;
        }
        if frames == 30 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            self.snapshot = Some((path.clone(), 31));
        }
        let shot = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let _ = save_png(&path, &img);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            ctx.request_repaint();
        }
    }

    /// Zed-style title bar: what's open on the left, the review actions on the right.
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let title = self.state.current_pr().and_then(|s| s.detail.as_ref().map(|d| d.title.clone())).unwrap_or_else(|| "Review Buddy".into());
            let reference = self.state.current.as_ref().map(|r| r.to_string());
            // Title and reference truncate; the controls on the right always fit.
            let right_w = 330.0;
            let w = (ui.available_width() - right_w).max(80.0);
            ui.allocate_ui_with_layout(egui::vec2(w, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_max_width(w);
                if let Some(r) = reference {
                    ui.add(egui::Label::new(RichText::new(r).size(13.0).color(p.muted_text)).selectable(false));
                }
                ui.add(egui::Label::new(RichText::new(title).size(13.0).color(p.text)).truncate().selectable(false));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::tool_button(ui, "⚙").on_hover_text("Settings (⌘,)").clicked() {
                    self.state.ui.show_settings = true;
                }
                let has_diff = self.state.current_pr().map(|s| s.diff.is_some()).unwrap_or(false);
                if !has_diff {
                    return;
                }
                let s = self.state.current_pr().unwrap();
                let (pending, n, mode) = (s.pending_comment_count(), s.local_commits.len(), s.view_mode());
                let label = if pending > 0 { format!("Review ({pending}) ▾") } else { "Review ▾".into() };
                if common::gh_button_ex(ui, &label, &["Review (99) ▾"], BtnKind::Primary, true).on_hover_text("Finish your review").clicked() {
                    self.state.ui.show_submit_review = !self.state.ui.show_submit_review;
                }
                if n > 0 && icons::tool_button(ui, &format!("⬆ {n} local")).on_hover_text("Push fix commits to the PR branch").clicked() {
                    self.state.ui.show_push_sheet = true;
                }
                let mut new_mode = mode;
                if icons::segmented(ui, &mut new_mode, &[(DiffViewMode::Unified, "Unified"), (DiffViewMode::Split, "Split")], 12.0) {
                    self.state.current_pr_mut().unwrap().set_mode(new_mode);
                }
            });
        });
    }

    /// Zed-style status bar: panel toggles at the ends, review progress in between.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let on = |b: bool| if b { p.text } else { p.muted_text };
            if icons::icon_button(ui, icons::Icon::SidebarLeft, 14.0, on(self.state.ui.show_sidebar), true).on_hover_text("Toggle inbox (⌃⌘S)").clicked() {
                self.state.ui.show_sidebar = !self.state.ui.show_sidebar;
            }
            if let Some(s) = self.state.current_pr() {
                if s.diff.is_some() {
                    ui.add_space(6.0);
                    let (viewed, total) = (s.viewed_count(), s.files().len());
                    ui.add(egui::Label::new(RichText::new(format!("{viewed} / {total} files viewed")).size(12.0).color(p.muted_text)).selectable(false));
                    let threads = s.threads.values().filter(|t| !t.is_resolved).count();
                    if threads > 0 {
                        ui.add_space(8.0);
                        icons::label(ui, &format!("💬 {threads} open"), 12.0, p.muted_text);
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let open = self.state.ui.show_discuss_panel;
                if icons::icon_button(ui, icons::Icon::Sidebar, 14.0, on(open), true).on_hover_text("Toggle Discuss panel (⌘\\)").clicked() {
                    self.state.ui.show_discuss_panel = !open;
                }
                if icons::icon_button(ui, icons::Icon::Sparkle, 13.0, if open { p.ai_accent } else { p.muted_text }, true).on_hover_text("Discuss with Claude (D)").clicked() {
                    self.state.ui.show_discuss_panel = !open;
                }
                ui.add_space(6.0);
                ui.add(egui::Label::new(RichText::new("⌘K  Commands").size(12.0).color(p.placeholder)).selectable(false));
            });
        });
    }

    fn detail(&mut self, ui: &mut egui::Ui, metrics: &DiffMetrics) {
        let p = pal(ui);
        let Some(r) = self.state.current.clone() else {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() / 3.0);
                    ui.label(RichText::new("No pull request selected").size(19.0).strong());
                    ui.label(RichText::new("Pick a PR from the inbox, or press ⌘L to open one by URL.").color(p.muted_text));
                    if ui.button("Open by URL…").clicked() {
                        self.state.ui.show_open_url = true;
                    }
                });
            });
            return;
        };
        let Some(s) = self.state.prs.get_mut(&r) else { return };
        match s.load_state.clone() {
            LoadState::Failed(e) if s.diff.is_none() => {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() / 3.0);
                    ui.label(RichText::new(format!("⚠ Couldn't load {r}")).size(17.0).strong());
                    ui.label(RichText::new(e).color(p.muted_text));
                    if ui.button("Retry").clicked() {
                        s.load();
                    }
                });
                return;
            }
            LoadState::Loading(msg) | LoadState::Failed(msg) if s.diff.is_none() => {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() / 3.0);
                    ui.spinner();
                    ui.label(RichText::new(msg).color(p.muted_text));
                });
                return;
            }
            _ => {}
        }
        egui::Frame::new().inner_margin(egui::Margin { left: 12, right: 12, top: 0, bottom: 0 }).show(ui, |ui| {
            if s.show_overview {
                overview::overview(ui, s, &mut self.view.overview);
                return;
            }
            let open_discuss = header::pr_header(ui, s);
            ui.separator();
            let cs = self.view.canvases.entry(r.clone()).or_default();
            let action = diff_canvas::diff_canvas(ui, s, cs, metrics);
            if open_discuss || matches!(action, diff_canvas::CanvasAction::OpenDiscussPanel) {
                self.state.ui.show_discuss_panel = true;
            }
        });
        // A new discussion opens the panel.
        if self.state.current_pr().and_then(|s| s.active_discuss_id.clone()).is_some() && self.view.discuss.last_active != self.state.current_pr().and_then(|s| s.active_discuss_id.clone()) {
            self.view.discuss.last_active = self.state.current_pr().and_then(|s| s.active_discuss_id.clone());
            self.state.ui.show_discuss_panel = true;
        }
    }

    // MARK: Keyboard

    fn keyboard(&mut self, ctx: &egui::Context) {
        let text_focused = ctx.memory(|m| m.focused().is_some()) && ctx.egui_wants_keyboard_input();
        let modal = self.state.ui.show_open_url || self.state.ui.show_palette || self.state.ui.show_push_sheet || self.state.ui.show_settings;
        let strokes: Vec<KeyStroke> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => {
                        let name = key_name(*key)?;
                        Some(KeyStroke::with_modifiers(&name, modifiers.mac_cmd || modifiers.command, modifiers.shift, modifiers.alt, modifiers.ctrl))
                    }
                    _ => None,
                })
                .collect()
        });
        for k in strokes {
            let Some(a) = KeyBindings::action(&k, text_focused) else { continue };
            if modal && a != ReviewAction::Cancel {
                continue;
            }
            self.perform(a);
        }
    }

    pub fn perform(&mut self, a: ReviewAction) {
        let st = &mut self.state;
        match a {
            ReviewAction::OpenByUrl => {
                st.ui.show_open_url = true;
                self.view.open_url_text.clear();
                self.view.open_url_error = None;
            }
            ReviewAction::CommandPalette => {
                st.ui.show_palette = true;
                self.view.palette_query.clear();
                self.view.palette_selected = 0;
            }
            ReviewAction::ToggleDiscussPanel => st.ui.show_discuss_panel = !st.ui.show_discuss_panel,
            ReviewAction::ToggleSidebar => st.ui.show_sidebar = !st.ui.show_sidebar,
            ReviewAction::Submit => st.ui.show_submit_review = true,
            ReviewAction::Refresh => st.reload_current(),
            ReviewAction::PushFixes => st.ui.show_push_sheet = true,
            ReviewAction::Cancel => {
                if st.ui.show_palette || st.ui.show_open_url || st.ui.show_push_sheet || st.ui.show_submit_review || st.ui.show_settings {
                    st.ui.show_palette = false;
                    st.ui.show_open_url = false;
                    st.ui.show_push_sheet = false;
                    st.ui.show_submit_review = false;
                    st.ui.show_settings = false;
                } else if let Some(s) = st.current_pr_mut() {
                    s.clear_selection();
                }
            }
            _ => {
                let Some(s) = st.current_pr_mut() else { return };
                match a {
                    ReviewAction::Summarize => s.start_summarize(),
                    ReviewAction::PreReview => s.start_pre_review(),
                    ReviewAction::ToggleSplit => s.toggle_mode(),
                    ReviewAction::ToggleViewed => {
                        let f = s.current_file;
                        s.toggle_viewed(f)
                    }
                    ReviewAction::CollapseFile => {
                        let f = s.current_file;
                        s.toggle_collapsed(f)
                    }
                    ReviewAction::Comment => s.start_comment(None, "", None),
                    ReviewAction::Discuss => {
                        if s.start_discuss(None, None).is_some() {
                            st.ui.show_discuss_panel = true;
                        }
                    }
                    ReviewAction::Reply => s.reply_to_focused_thread(),
                    _ => s.navigate(a),
                }
            }
        }
    }

    // MARK: Sheets

    fn sheets(&mut self, ctx: &egui::Context) {
        if self.state.ui.show_open_url {
            self.open_url_sheet(ctx);
        }
        if self.state.ui.show_palette {
            self.palette(ctx);
        }
        if self.state.ui.show_push_sheet {
            self.push_sheet(ctx);
        }
        if self.state.ui.show_submit_review {
            self.submit_review(ctx);
        }
        if self.state.ui.show_settings {
            settings::settings_window(ctx, &mut self.state, &mut self.view.settings);
        }
    }

    fn open_url_sheet(&mut self, ctx: &egui::Context) {
        if self.view.open_url_text.is_empty() {
            // Prefill from the clipboard isn't available in egui; the field starts empty.
        }
        let mut open = false;
        let mut close = false;
        let m = egui::Modal::new(egui::Id::new("open_url")).show(ctx, |ui| {
            let p = pal(ui);
            ui.set_width(480.0);
            ui.label(RichText::new("Open pull request").size(16.0).strong());
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.view.open_url_text)
                    .hint_text("https://github.com/owner/repo/pull/123  or  owner/repo#123")
                    .desired_width(f32::INFINITY),
            );
            r.request_focus();
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                open = true;
            }
            if let Some(e) = &self.view.open_url_error {
                ui.label(RichText::new(e).size(13.0).color(p.del_marker));
            }
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if gh_button(ui, "Open", BtnKind::Primary, !self.view.open_url_text.is_empty()).clicked() {
                        open = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        });
        if m.should_close() || close {
            self.state.ui.show_open_url = false;
        }
        if open {
            match PRRef::parse(&self.view.open_url_text) {
                Some(r) => {
                    self.state.ui.show_open_url = false;
                    self.state.open(r);
                }
                None => self.view.open_url_error = Some("That doesn't look like a pull request URL.".into()),
            }
        }
    }

    fn palette(&mut self, ctx: &egui::Context) {
        enum Item {
            Action(ReviewAction),
            File(usize),
            Pr(PRRef),
        }
        let mut items: Vec<(String, Option<String>, Option<&'static str>, Item)> = vec![];
        for a in ReviewAction::ALL {
            if a != ReviewAction::CommandPalette && a != ReviewAction::Cancel {
                items.push((a.title().into(), None, a.shortcut(), Item::Action(a)));
            }
        }
        if let Some(s) = self.state.current_pr() {
            for (i, f) in s.files().iter().enumerate() {
                let name = f.path().rsplit('/').next().unwrap_or(f.path()).to_string();
                items.push((name, Some(f.path().to_string()), None, Item::File(i)));
            }
        }
        for sec in &self.state.inbox.sections {
            for pr in &sec.items {
                items.push((pr.title.clone(), Some(pr.pr_ref.to_string()), None, Item::Pr(pr.pr_ref.clone())));
            }
        }
        let q = self.view.palette_query.clone();
        let mut list: Vec<(i64, usize)> = if q.is_empty() {
            (0..items.len().min(60)).map(|i| (0, i)).collect()
        } else {
            let mut v: Vec<(i64, usize)> = items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| {
                    let s1 = KeyBindings::fuzzy_score(&q, &it.0);
                    let s2 = it.1.as_ref().and_then(|s| KeyBindings::fuzzy_score(&q, s)).map(|s| s - 50);
                    s1.into_iter().chain(s2).max().map(|s| (s, i))
                })
                .collect();
            v.sort_by(|a, b| b.0.cmp(&a.0));
            v.truncate(60);
            v
        };
        list.truncate(60);
        let mut run: Option<usize> = None;
        let mut close = false;
        let m = egui::Modal::new(egui::Id::new("palette")).show(ctx, |ui| {
            let p = pal(ui);
            ui.set_width(560.0);
            let r = ui.add(egui::TextEdit::singleline(&mut self.view.palette_query).hint_text("Type a command, file or PR…").desired_width(f32::INFINITY).font(egui::FontId::proportional(16.0)));
            r.request_focus();
            if r.changed() {
                self.view.palette_selected = 0;
            }
            let (down, up, enter) = ui.input(|i| (i.key_pressed(Key::ArrowDown), i.key_pressed(Key::ArrowUp), i.key_pressed(Key::Enter)));
            if down {
                self.view.palette_selected = (self.view.palette_selected + 1).min(list.len().saturating_sub(1));
            }
            if up {
                self.view.palette_selected = self.view.palette_selected.saturating_sub(1);
            }
            if enter {
                run = Some(self.view.palette_selected);
            }
            ui.separator();
            egui::ScrollArea::vertical().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible).max_height(340.0).show(ui, |ui| {
                for (n, (_, i)) in list.iter().enumerate() {
                    let it = &items[*i];
                    let selected = n == self.view.palette_selected;
                    let resp = egui::Frame::new()
                        .fill(if selected { p.accent.gamma_multiply(0.15) } else { egui::Color32::TRANSPARENT })
                        .inner_margin(egui::Margin::symmetric(10, 5))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(&it.0);
                                    if let Some(s) = &it.1 {
                                        ui.label(RichText::new(s).size(12.0).color(p.muted_text));
                                    }
                                });
                                if let Some(k) = it.2 {
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.label(RichText::new(k).monospace().size(12.0).color(p.muted_text));
                                    });
                                }
                            });
                        })
                        .response
                        .interact(egui::Sense::click());
                    if selected && (down || up) {
                        resp.scroll_to_me(None);
                    }
                    if resp.clicked() {
                        run = Some(n);
                    }
                }
            });
        });
        if m.should_close() {
            close = true;
        }
        if let Some(n) = run {
            self.state.ui.show_palette = false;
            if let Some((_, i)) = list.get(n) {
                match &items[*i].3 {
                    Item::Action(a) => self.perform(*a),
                    Item::File(f) => {
                        if let Some(s) = self.state.current_pr_mut() {
                            s.scroll_to_file(*f);
                        }
                    }
                    Item::Pr(r) => self.state.open(r.clone()),
                }
            }
        }
        if close {
            self.state.ui.show_palette = false;
        }
    }

    fn push_sheet(&mut self, ctx: &egui::Context) {
        let Some(s) = self.state.current_pr_mut() else {
            self.state.ui.show_push_sheet = false;
            return;
        };
        let mut close = false;
        let m = egui::Modal::new(egui::Id::new("push")).show(ctx, |ui| {
            let p = pal(ui);
            ui.set_width(520.0);
            ui.label(RichText::new("Push fix commits").size(16.0).strong());
            if let Some(d) = &s.detail {
                ui.label(
                    RichText::new(format!(
                        "These commits will be pushed to {} on {}.",
                        d.head_ref_name,
                        d.head_repo_name_with_owner.clone().unwrap_or_else(|| d.pr_ref.name_with_owner())
                    ))
                    .size(13.0)
                    .color(p.muted_text),
                );
            }
            common::card(ui, p.canvas, p.border, |ui| {
                ui.set_width(ui.available_width());
                for c in &s.local_commits {
                    egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(c.short_sha()).monospace().size(13.0).color(p.accent));
                            ui.label(RichText::new(&c.subject).size(13.0));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(&c.author).size(13.0).color(p.muted_text));
                            });
                        });
                    });
                }
            });
            if let Some(r) = s.push_disabled_reason() {
                ui.label(RichText::new(format!("🔒 {r}")).size(13.0).color(p.del_marker));
            }
            if let TaskState::Failed(e) = &s.push_state {
                ui.label(RichText::new(e).size(13.0).color(p.del_marker));
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Never force-pushes.").size(12.0).color(p.muted_text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let n = s.local_commits.len();
                    let ok = s.can_push() && n > 0 && !s.push_state.is_running();
                    if s.push_state.is_running() {
                        ui.spinner();
                    } else if gh_button(ui, &format!("Push {n} commit{}", if n == 1 { "" } else { "s" }), BtnKind::Primary, ok).clicked() {
                        let r = s.pr_ref.clone();
                        s.push(move |_, ok| {
                            if ok {
                                let _ = r;
                            }
                        });
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        });
        if m.should_close() || close {
            self.state.ui.show_push_sheet = false;
        }
    }

    fn submit_review(&mut self, ctx: &egui::Context) {
        let Some(s) = self.state.current_pr_mut() else {
            self.state.ui.show_submit_review = false;
            return;
        };
        let event = self.view.review_event.get_or_insert(ReviewEvent::Comment);
        let mut close = false;
        let m = egui::Modal::new(egui::Id::new("submit_review")).show(ctx, |ui| {
            let p = pal(ui);
            ui.set_width(420.0);
            ui.label(RichText::new("Finish your review").size(15.0).strong());
            ui.add(egui::TextEdit::multiline(&mut s.review_body).desired_rows(5).desired_width(f32::INFINITY).hint_text("Leave a comment"));
            let own = s.detail.as_ref().map(|d| d.author == d.viewer_login).unwrap_or(false);
            for (e, detail) in [
                (ReviewEvent::Comment, "Submit general feedback without explicit approval."),
                (ReviewEvent::Approve, "Give your approval to merge these changes."),
                (ReviewEvent::RequestChanges, "Submit feedback that must be addressed before merging."),
            ] {
                let enabled = e == ReviewEvent::Comment || !own;
                ui.add_enabled_ui(enabled, |ui| {
                    ui.radio_value(event, e, RichText::new(e.title()).strong());
                    ui.label(RichText::new(detail).size(12.5).color(p.muted_text));
                });
            }
            if let TaskState::Failed(e) = &s.submit_state {
                ui.label(RichText::new(e).size(13.0).color(p.del_marker));
            }
            ui.horizontal(|ui| {
                let n = s.pending_comment_count();
                ui.label(RichText::new(format!("{n} pending comment{}", if n == 1 { "" } else { "s" })).size(13.0).color(p.muted_text));
                if s.pending_review_id.is_some() && gh_button(ui, "Discard", BtnKind::Danger, true).clicked() {
                    s.discard_pending_review();
                    close = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ok = !s.submit_state.is_running() && (*event != ReviewEvent::Comment || !s.review_body.is_empty() || n > 0);
                    let cmd_enter = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Enter));
                    if s.submit_state.is_running() {
                        ui.spinner();
                    } else if gh_button(ui, "Submit review", BtnKind::Primary, ok).clicked() || (cmd_enter && ok) {
                        let e = *event;
                        s.submit_review(e, |_, _| {});
                        close = true;
                    }
                });
            });
        });
        if m.should_close() || close {
            self.state.ui.show_submit_review = false;
        }
    }
}

fn key_name(k: Key) -> Option<String> {
    Some(match k {
        Key::Escape => "escape".into(),
        Key::Enter => "return".into(),
        Key::Backslash => "\\".into(),
        Key::OpenBracket => "[".into(),
        Key::CloseBracket => "]".into(),
        Key::Comma => ",".into(),
        Key::Slash => "/".into(),
        k => {
            let n = k.name();
            if n.chars().count() == 1 { n.to_lowercase() } else { return None }
        }
    })
}

pub(crate) fn save_png(path: &str, img: &egui::ColorImage) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.size[0] as u32, img.size[1] as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(std::io::Error::other)?;
    let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
    w.write_image_data(&bytes).map_err(std::io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod style_tests {
    /// A button with its own stroke keeps its size when hovered.
    #[test]
    fn buttons_do_not_shrink_on_hover() {
        let ctx = egui::Context::default();
        super::install_style(&ctx);
        let frame = |pos: Option<egui::Pos2>| {
            let mut input = egui::RawInput::default();
            if let Some(p) = pos {
                input.events.push(egui::Event::PointerMoved(p));
            }
            let mut rect = egui::Rect::NOTHING;
            let _ = ctx.run_ui(input, |ui| {
                let b = egui::Button::new("Confirm squash and merge").fill(egui::Color32::GREEN).stroke(egui::Stroke::NONE);
                rect = ui.add(b).rect;
            });
            rect
        };
        let idle = frame(None);
        let _ = frame(Some(idle.center()));
        let hovered = frame(Some(idle.center()));
        assert_eq!(idle.size(), hovered.size());
    }
}
