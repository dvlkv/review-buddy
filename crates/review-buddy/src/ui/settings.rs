//! Settings window: Accounts, Repositories and Review tabs (port of SettingsView.swift).

use std::sync::{Arc, Mutex};

use egui::{Align, FontId, Layout, RichText, Ui};
use review_buddy_core::diff::layout::DiffViewMode;
use review_buddy_core::git::process::ToolLocator;
use review_buddy_core::persistence::AppSettings;
use review_buddy_core::support::keychain::Keychain;

use super::common::{card, gh_button, pal, BtnKind};
use crate::app::AppState;

const PAT_ACCOUNT: &str = "github-pat";

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Accounts,
    Repositories,
    Review,
}

#[derive(Default)]
pub struct SettingsState {
    tab: Tab,
    /// Pass number of the last frame the window was drawn; a gap means it was reopened.
    last_pass: Option<u64>,
    /// `gh auth status` result, filled in by a worker thread.
    gh_status: Arc<Mutex<Option<String>>>,
    /// Keychain PAT, filled in by a worker thread (None until read).
    pat_loaded: Arc<Mutex<Option<String>>>,
    pat: String,
    pat_saved: Option<bool>,
    model: String,
    new_repo: String,
    /// Repo whose local clone path is being edited, and the edit buffer.
    editing_clone: Option<(String, String)>,
}

impl SettingsState {
    fn on_open(&mut self, ctx: &egui::Context, settings: &AppSettings) {
        self.model = settings.claude_model.clone();
        self.pat.clear();
        self.pat_saved = None;
        self.new_repo.clear();
        self.editing_clone = None;
        *self.gh_status.lock().unwrap() = None;
        *self.pat_loaded.lock().unwrap() = None;

        let status = self.gh_status.clone();
        let c = ctx.clone();
        std::thread::spawn(move || {
            *status.lock().unwrap() = Some(gh_auth_status());
            c.request_repaint();
        });
        let pat = self.pat_loaded.clone();
        let c = ctx.clone();
        std::thread::spawn(move || {
            *pat.lock().unwrap() = Some(Keychain::read(PAT_ACCOUNT).unwrap_or_default());
            c.request_repaint();
        });
    }
}

fn gh_auth_status() -> String {
    let Some(gh) = ToolLocator::find("gh") else { return "Not installed".into() };
    let Ok(out) = std::process::Command::new(gh).args(["auth", "status"]).output() else {
        return "Not installed".into();
    };
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !text.contains("Logged in") {
        return "Not logged in — run `gh auth login`".into();
    }
    text.lines()
        .find(|l| l.contains("Logged in"))
        .map(|l| l.trim().trim_start_matches(|c: char| !c.is_alphanumeric()).trim().to_string())
        .unwrap_or_else(|| "Logged in".into())
}

pub fn settings_window(ctx: &egui::Context, state: &mut AppState, s: &mut SettingsState) {
    let pass = ctx.cumulative_pass_nr();
    if s.last_pass.map(|p| pass > p + 1).unwrap_or(true) {
        s.on_open(ctx, &state.services.settings());
    }
    s.last_pass = Some(pass);
    if let Some(pat) = s.pat_loaded.lock().unwrap().take() {
        if s.pat.is_empty() {
            s.pat = pat;
        }
    }

    let before = state.services.settings();
    let mut edited = before.clone();
    let mut pinned_changed = false;

    let mut open = true;
    egui::Window::new("Settings")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(560.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.set_width(560.0);
            ui.set_min_height(420.0);
            tab_bar(ui, &mut s.tab);
            ui.add_space(8.0);
            egui::ScrollArea::vertical().max_height(520.0).auto_shrink([false, true]).show(ui, |ui| match s.tab {
                Tab::Accounts => accounts_tab(ui, state, s, &mut edited),
                Tab::Repositories => pinned_changed = repos_tab(ui, s, &mut edited),
                Tab::Review => review_tab(ui, &mut edited),
            });
        });

    if edited != before {
        let affects_layout = edited.view_mode != before.view_mode
            || edited.large_file_threshold != before.large_file_threshold
            || edited.hide_whitespace != before.hide_whitespace;
        state.services.update_settings(|x| *x = edited);
        if affects_layout {
            for pr in state.prs.values_mut() {
                pr.apply_settings();
            }
        }
        if pinned_changed {
            state.pinned_repos_changed();
        }
    }

    if !open {
        state.ui.show_settings = false;
        s.last_pass = None;
    }
}

// MARK: Chrome

fn tab_bar(ui: &mut Ui, tab: &mut Tab) {
    let p = pal(ui);
    ui.horizontal(|ui| {
        let w = 3.0 * 120.0;
        ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
        for (t, name) in [(Tab::Accounts, "Accounts"), (Tab::Repositories, "Repositories"), (Tab::Review, "Review")] {
            let selected = *tab == t;
            let text = RichText::new(name).size(14.0).color(if selected { p.text } else { p.muted_text });
            let btn = egui::Button::new(if selected { text.strong() } else { text })
                .fill(if selected { p.subtle } else { egui::Color32::TRANSPARENT })
                .stroke(egui::Stroke::new(1.0, if selected { p.border } else { egui::Color32::TRANSPARENT }))
                .corner_radius(egui::CornerRadius::same(6))
                .min_size(egui::vec2(116.0, 26.0));
            if ui.add(btn).clicked() {
                *tab = t;
            }
        }
    });
}

/// A grouped-form section: a heading, a bordered card of rows, an optional footer.
fn section(ui: &mut Ui, title: &str, footer: Option<&str>, add: impl FnOnce(&mut Ui)) {
    let p = pal(ui);
    ui.add_space(4.0);
    ui.label(RichText::new(title).size(13.0).strong().color(p.muted_text));
    ui.add_space(2.0);
    card(ui, p.subtle, p.border, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            add(ui);
        });
    });
    if let Some(f) = footer {
        ui.add_space(2.0);
        ui.label(RichText::new(f).size(12.0).color(p.muted_text));
    }
    ui.add_space(10.0);
}

fn divider(ui: &mut Ui) {
    let p = pal(ui);
    let r = ui.available_rect_before_wrap();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(r.width(), 1.0), egui::Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, egui::Stroke::new(1.0, p.muted_border));
}

/// A labelled row with its control right-aligned.
fn row(ui: &mut Ui, label: &str, control: impl FnOnce(&mut Ui)) {
    let p = pal(ui);
    ui.horizontal(|ui| {
        ui.set_min_height(24.0);
        ui.label(RichText::new(label).size(14.0).color(p.text));
        ui.with_layout(Layout::right_to_left(Align::Center), control);
    });
}

fn caption(ui: &mut Ui, text: &str) {
    let p = pal(ui);
    ui.label(RichText::new(text).size(12.0).color(p.muted_text));
}

fn toggle(ui: &mut Ui, label: &str, value: &mut bool) {
    let p = pal(ui);
    ui.horizontal(|ui| {
        ui.set_min_height(24.0);
        ui.checkbox(value, RichText::new(label).size(14.0).color(p.text));
    });
}

/// SwiftUI-style stepper: a label and −/+ buttons that move `value` by `step` within `min..=max`.
fn stepper(ui: &mut Ui, label: &str, value: &mut i64, min: i64, max: i64, step: i64) {
    row(ui, label, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if gh_button(ui, " + ", BtnKind::Default, *value < max).clicked() {
            *value = (*value + step).min(max);
        }
        if gh_button(ui, " − ", BtnKind::Default, *value > min).clicked() {
            *value = (*value - step).max(min);
        }
    });
}

// MARK: Accounts

fn accounts_tab(ui: &mut Ui, state: &AppState, s: &mut SettingsState, edited: &mut AppSettings) {
    let p = pal(ui);
    section(ui, "GitHub", None, |ui| {
        let status = s.gh_status.lock().unwrap().clone();
        row(ui, "GitHub CLI", |ui| match status {
            Some(st) => {
                ui.label(RichText::new(st).size(13.0).color(p.muted_text));
            }
            None => {
                ui.label(RichText::new("Checking…").size(13.0).color(p.muted_text));
                ui.add(egui::Spinner::new().size(12.0));
            }
        });
        divider(ui);
        let mut save = false;
        row(ui, "Personal access token (fallback)", |ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut s.pat).password(true).desired_width(220.0).hint_text("ghp_…"));
            if r.changed() {
                s.pat_saved = None;
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                save = true;
            }
        });
        caption(
            ui,
            "Review Buddy uses `gh auth token`. Add a token only if the GitHub CLI isn't installed. It's stored in your Keychain.",
        );
        ui.horizontal(|ui| {
            if gh_button(ui, "Save token", BtnKind::Default, true).clicked() {
                save = true;
            }
            match s.pat_saved {
                Some(true) => {
                    ui.label(RichText::new("Saved").size(12.0).color(p.success));
                }
                Some(false) => {
                    ui.label(RichText::new("Couldn't write to the Keychain").size(12.0).color(p.del_marker));
                }
                None => {}
            }
        });
        if save {
            s.pat_saved = Some(Keychain::write(PAT_ACCOUNT, s.pat.trim()));
        }
    });

    section(ui, "Claude", None, |ui| {
        row(ui, "claude CLI", |ui| {
            let text = match &state.services.claude_path {
                Some(path) => RichText::new(path).font(FontId::monospace(12.0)).color(p.muted_text),
                None => RichText::new("Not found — install Claude Code").size(13.0).color(p.attention),
            };
            ui.add(egui::Label::new(text).truncate());
        });
        divider(ui);
        row(ui, "Model", |ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut s.model).desired_width(220.0).hint_text("Blank = CLI default"));
            if r.changed() {
                edited.claude_model = s.model.clone();
            }
        });
        caption(ui, "Discuss runs with full permissions inside a separate worktree for each PR. It never pushes on its own.");
    });
}

// MARK: Repositories

/// Returns true when the pinned list changed.
fn repos_tab(ui: &mut Ui, s: &mut SettingsState, edited: &mut AppSettings) -> bool {
    let p = pal(ui);
    let mono = FontId::monospace(13.0);
    let mut pinned_changed = false;

    section(ui, "Pinned repositories", None, |ui| {
        let mut remove = None;
        for repo in &edited.pinned_repos {
            ui.horizontal(|ui| {
                ui.set_min_height(22.0);
                ui.label(RichText::new(repo).font(mono.clone()).color(p.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if gh_button(ui, "Remove", BtnKind::Danger, true).on_hover_text("Unpin").clicked() {
                        remove = Some(repo.clone());
                    }
                });
            });
            divider(ui);
        }
        if let Some(r) = remove {
            edited.pinned_repos.retain(|x| *x != r);
            pinned_changed = true;
        }
        let mut add = false;
        ui.horizontal(|ui| {
            let can_add = s.new_repo.contains('/');
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if gh_button(ui, "Add", BtnKind::Default, can_add).clicked() {
                    add = true;
                }
                let r = ui.add(
                    egui::TextEdit::singleline(&mut s.new_repo).hint_text("owner/repo").font(mono.clone()).desired_width(f32::INFINITY),
                );
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    add = true;
                }
            });
        });
        if add {
            let r = s.new_repo.trim().to_string();
            let parts: Vec<&str> = r.split('/').collect();
            if parts.len() == 2 && parts.iter().all(|x| !x.is_empty()) && !edited.pinned_repos.contains(&r) {
                edited.pinned_repos.push(r);
                s.new_repo.clear();
                pinned_changed = true;
            }
        }
    });

    let mut repos: Vec<String> = edited.pinned_repos.iter().chain(edited.repo_clones.keys()).cloned().collect();
    repos.sort();
    repos.dedup();
    section(
        ui,
        "Local clones",
        Some("Map a repository to your own clone to reuse its objects and build caches. Review Buddy always works in a separate worktree, so your checkout is never touched."),
        |ui| {
            if repos.is_empty() {
                caption(ui, "Pin a repository to map it to a local clone.");
            }
            for (i, repo) in repos.iter().enumerate() {
                if i > 0 {
                    divider(ui);
                }
                let editing = s.editing_clone.as_ref().map(|(r, _)| r == repo).unwrap_or(false);
                ui.horizontal(|ui| {
                    ui.set_min_height(22.0);
                    ui.label(RichText::new(repo).font(mono.clone()).color(p.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if edited.repo_clones.contains_key(repo)
                            && gh_button(ui, "×", BtnKind::Invisible, true).on_hover_text("Use a managed clone").clicked()
                        {
                            edited.repo_clones.remove(repo);
                            if editing {
                                s.editing_clone = None;
                            }
                        }
                        if !editing && gh_button(ui, "Choose…", BtnKind::Default, true).clicked() {
                            let cur = edited.repo_clones.get(repo).cloned().unwrap_or_default();
                            s.editing_clone = Some((repo.clone(), cur));
                        }
                        if !editing {
                            let (text, color) = match edited.repo_clones.get(repo) {
                                Some(path) => (head_truncate(path, 44), p.muted_text),
                                None => ("Managed clone".to_string(), p.muted_text),
                            };
                            ui.add(egui::Label::new(RichText::new(text).size(13.0).color(color)).truncate());
                        }
                    });
                });
                if editing {
                    let mut commit = false;
                    let mut cancel = false;
                    let mut error = None;
                    if let Some((_, buf)) = s.editing_clone.as_mut() {
                        ui.horizontal(|ui| {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if gh_button(ui, "Cancel", BtnKind::Default, true).clicked() {
                                    cancel = true;
                                }
                                if gh_button(ui, "Save", BtnKind::Primary, !buf.trim().is_empty()).clicked() {
                                    commit = true;
                                }
                                let r = ui.add(
                                    egui::TextEdit::singleline(buf)
                                        .hint_text(format!("Path to your local clone of {repo}"))
                                        .font(FontId::monospace(12.0))
                                        .desired_width(f32::INFINITY),
                                );
                                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                    commit = true;
                                }
                            });
                        });
                        if commit {
                            let path = expand_tilde(buf.trim());
                            if std::path::Path::new(&path).is_dir() {
                                edited.repo_clones.insert(repo.clone(), path);
                            } else {
                                error = Some("That folder doesn't exist.");
                                commit = false;
                            }
                        }
                    }
                    if let Some(e) = error {
                        ui.label(RichText::new(e).size(12.0).color(p.del_marker));
                    }
                    if commit || cancel {
                        s.editing_clone = None;
                    }
                }
            }
        },
    );
    pinned_changed
}

fn expand_tilde(path: &str) -> String {
    match (path.strip_prefix('~'), std::env::var("HOME")) {
        (Some(rest), Ok(home)) if rest.is_empty() || rest.starts_with('/') => format!("{home}{rest}"),
        _ => path.to_string(),
    }
}

/// Keeps the tail of a long path, like SwiftUI's `.truncationMode(.head)`.
fn head_truncate(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max { s.to_string() } else { format!("…{}", s.chars().skip(n - max + 1).collect::<String>()) }
}

// MARK: Review

fn review_tab(ui: &mut Ui, s: &mut AppSettings) {
    let p = pal(ui);
    section(ui, "AI", None, |ui| {
        toggle(ui, "Summarize pull requests automatically when opened", &mut s.auto_summarize);
        divider(ui);
        let label = if s.auto_pre_review_max_lines == 0 {
            "Auto pre-review: off".to_string()
        } else {
            format!("Auto pre-review PRs up to {} changed lines", s.auto_pre_review_max_lines)
        };
        stepper(ui, &label, &mut s.auto_pre_review_max_lines, 0, 5000, 100);
    });

    section(ui, "Diff", None, |ui| {
        row(ui, "Default view", |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (mode, name) in [(DiffViewMode::Split, "Split"), (DiffViewMode::Unified, "Unified")] {
                let selected = s.view_mode == mode;
                let btn = egui::Button::new(RichText::new(name).size(13.0).color(if selected { p.text } else { p.muted_text }))
                    .fill(if selected { p.canvas } else { p.subtle })
                    .stroke(egui::Stroke::new(1.0, p.border))
                    .min_size(egui::vec2(72.0, 24.0))
                    .corner_radius(if mode == DiffViewMode::Split {
                        egui::CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 }
                    } else {
                        egui::CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }
                    });
                if ui.add(btn).clicked() {
                    s.view_mode = mode;
                }
            }
        });
        divider(ui);
        let label = format!("Collapse files with more than {} changed lines", s.large_file_threshold);
        stepper(ui, &label, &mut s.large_file_threshold, 200, 20000, 200);
        divider(ui);
        let mut size = s.font_size.round() as i64;
        stepper(ui, &format!("Code font size: {size} pt"), &mut size, 10, 18, 1);
        if size != s.font_size.round() as i64 {
            s.font_size = size as f64;
        }
        divider(ui);
    });

    section(ui, "Notifications", None, |ui| {
        toggle(ui, "Notify me about new review requests", &mut s.notify_on_review_requests);
    });
}
