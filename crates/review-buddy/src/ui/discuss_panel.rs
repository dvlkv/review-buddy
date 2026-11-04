//! Right-hand Discuss panel: the list of Discuss threads and the active conversation.

use std::collections::HashSet;

use egui::{Align, Color32, CornerRadius, FontId, Key, KeyboardShortcut, Layout, Margin, Modifiers, RichText, Sense, Stroke, Ui, Vec2};
use review_buddy_core::claude::transcript::{DiscussMessage, Role};
use review_buddy_core::*;

use crate::pr_store::{PRStore, TaskState};
use crate::theme::Palette;
use crate::ui::blocks::fix_badge;
use crate::ui::common::{gh_button, pal, pill, relative, spinner, BtnKind};
use crate::ui::markdown::markdown;

#[derive(Default)]
pub struct DiscussPanelState {
    pub last_active: Option<String>,
    /// Tool rows the user unfolded (by message id).
    expanded_tools: HashSet<String>,
    /// The conversation the input box was last focused for (focus on open, like Swift's onAppear).
    focused_for: Option<String>,
}

/// What the conversation view asked for this frame; applied once the borrows end.
enum Intent {
    Send(String),
    Stop,
    FixIt,
    Convert,
    Delete,
}

pub fn discuss_panel(ui: &mut Ui, store: &mut PRStore, state: &mut DiscussPanelState) {
    let p = pal(ui);
    let active = store.active_discuss_id.clone().filter(|id| store.discuss_threads.contains_key(id));
    if active.is_none() {
        state.focused_for = None;
    }
    header(ui, store, active.is_some(), &p);
    ui.add(egui::Separator::default().spacing(0.0));
    match active {
        Some(id) => conversation(ui, store, state, &id, &p),
        None => thread_list(ui, store, &p),
    }
}

fn header(ui: &mut Ui, store: &mut PRStore, has_active: bool, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 38.0), Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(12.0, 0.0))).layout(Layout::left_to_right(Align::Center)));
    let ui = &mut child;
    ui.spacing_mut().item_spacing.x = 8.0;
    if has_active {
        let back = super::icons::icon_button(ui, super::icons::Icon::ChevronLeft, 13.0, p.muted_text, true).on_hover_text("All conversations");
        if back.clicked() {
            store.active_discuss_id = None;
        }
    }
    super::icons::icon(ui, super::icons::Icon::Sparkle, 14.0, p.ai_accent);
    ui.label(RichText::new("Discuss").strong().size(14.0).color(p.text));
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let n = store.local_commits.len();
        if n > 0 {
            pill(ui, &format!("{n} local commit{}", if n == 1 { "" } else { "s" }), p.attention, false);
        }
    });
}

// MARK: Thread list

fn thread_list(ui: &mut Ui, store: &mut PRStore, p: &Palette) {
    if store.discuss_threads.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.set_max_width((ui.available_width() - 48.0).max(100.0));
            ui.spacing_mut().item_spacing.y = 10.0;
            super::icons::icon(ui, super::icons::Icon::Sparkle, 28.0, p.ai_accent.gamma_multiply(0.6));
            ui.label(RichText::new("Select lines in the diff and press Discuss (or D) to ask Claude about them.").size(13.0).color(p.muted_text));
            ui.label(
                RichText::new("Claude works in a private checkout of this PR. It can read the whole repo, run commands, and fix what you discuss.")
                    .size(12.0)
                    .color(p.muted_text),
            );
        });
        return;
    }
    let threads = store.sorted_discuss_threads();
    let mut open: Option<String> = None;
    let mut delete: Option<String> = None;
    egui::ScrollArea::vertical().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible).auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for t in &threads {
            let inner = egui::Frame::new().inner_margin(Margin::symmetric(12, 7)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(RichText::new(t.anchor.label()).font(FontId::monospace(12.0)).color(p.accent));
                    if t.status == DiscussStatus::Outdated {
                        pill(ui, "Outdated", p.attention, false);
                    }
                    if let Some(fix) = store.fix_status(&t.id) {
                        fix_badge(ui, fix);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(relative(&t.created_at)).size(11.0).color(p.muted_text));
                    });
                });
                let title = if t.messages.is_empty() { "New conversation".to_string() } else { t.title() };
                ui.add(egui::Label::new(RichText::new(two_lines(&title)).size(13.0).color(p.text)).wrap().selectable(false));
            });
            let resp = ui.interact(inner.response.rect, ui.id().with(("discuss-row", &t.id)), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(inner.response.rect, CornerRadius::ZERO, p.subtle.gamma_multiply(0.6));
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.clicked() {
                open = Some(t.id.clone());
            }
            resp.context_menu(|ui| {
                if ui.button(RichText::new("Delete conversation").color(p.del_marker)).clicked() {
                    delete = Some(t.id.clone());
                    ui.close();
                }
            });
            let r = inner.response.rect;
            ui.painter().hline(r.x_range(), r.bottom(), Stroke::new(1.0, p.muted_border));
        }
    });
    if let Some(id) = open {
        store.active_discuss_id = Some(id.clone());
        store.scroll_to_block(&id);
    }
    if let Some(id) = delete {
        store.delete_discuss(&id);
    }
}

/// Keeps at most two lines of a title (Swift's `lineLimit(2)`).
fn two_lines(s: &str) -> String {
    let t: String = s.chars().take(160).collect();
    if t.len() < s.len() {
        format!("{t}…")
    } else {
        t
    }
}

// MARK: Conversation

fn conversation(ui: &mut Ui, store: &mut PRStore, state: &mut DiscussPanelState, id: &str, p: &Palette) {
    let Some(thread) = store.discuss_threads.get(id).cloned() else { return };
    store.controller(id);
    let mut intent: Option<Intent> = None;

    // Anchor header.
    egui::Panel::top(egui::Id::new("discuss-anchor"))
        .frame(egui::Frame::new().fill(p.canvas))
        .resizable(false)
        .show_separator_line(true)
        .show(ui, |ui| anchor_header(ui, store, &thread, &mut intent, p));

    // Actions, fix commits and input box.
    egui::Panel::bottom(egui::Id::new("discuss-input"))
        .frame(egui::Frame::new().fill(p.canvas))
        .resizable(false)
        .show_separator_line(true)
        .show(ui, |ui| bottom_area(ui, store, state, &thread, &mut intent, p));

    // Transcript.
    egui::CentralPanel::default().frame(egui::Frame::new().fill(p.canvas)).show(ui, |ui| {
        let c = &store.controllers[id];
        egui::ScrollArea::vertical().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin::same(12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 10.0;
                if c.messages().is_empty() {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Ask Claude anything about the selected lines — why they're written this way, whether there's a bug, how to test them…")
                            .size(13.0)
                            .color(p.muted_text),
                    );
                }
                for m in c.messages() {
                    message(ui, m, &mut state.expanded_tools, p);
                }
                if let Some(status) = &c.status {
                    ui.horizontal(|ui| {
                        spinner(ui);
                        ui.label(RichText::new(status).size(13.0).color(p.muted_text));
                    });
                } else if c.reducer.is_running() {
                    typing_indicator(ui, p);
                }
            });
        });
    });

    match intent {
        Some(Intent::Send(text)) => {
            store.with_controller(id, |c, s| c.send(s, &text));
        }
        Some(Intent::Stop) => {
            store.with_controller(id, |c, s| c.stop(s));
        }
        Some(Intent::FixIt) => {
            store.with_controller(id, |c, s| c.fix_it(s));
        }
        Some(Intent::Convert) => {
            store.with_controller(id, |c, s| c.convert_to_comment(s));
        }
        Some(Intent::Delete) => store.delete_discuss(id),
        None => {}
    }
}

fn anchor_header(ui: &mut Ui, store: &mut PRStore, t: &DiscussThread, intent: &mut Option<Intent>, p: &Palette) {
    let lines = store.lines_for(&t.anchor);
    let inner = egui::Frame::new().inner_margin(Margin::same(10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new("≡").color(p.muted_text).size(13.0));
            ui.label(RichText::new(t.anchor.label()).font(FontId::monospace(13.0)).color(p.accent));
            if t.status == DiscussStatus::Outdated {
                pill(ui, "Outdated", p.attention, false);
            }
            if let Some(fix) = store.fix_status(&t.id) {
                fix_badge(ui, fix);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let del = ui
                    .add(egui::Button::new(RichText::new("Delete").size(12.0).color(p.muted_text)).frame(false))
                    .on_hover_text("Delete this conversation");
                if del.clicked() {
                    *intent = Some(Intent::Delete);
                }
            });
        });
        if !lines.is_empty() {
            let mut shown: Vec<&str> = lines.iter().take(6).map(String::as_str).collect();
            if lines.len() > 6 {
                shown.push("…");
            }
            egui::Frame::new().fill(p.subtle).corner_radius(CornerRadius::same(5)).inner_margin(Margin::same(6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 1.0;
                // One truncated label per line (a multi-line label would truncate to a single row).
                for l in shown {
                    ui.add(egui::Label::new(RichText::new(l).font(FontId::monospace(12.0)).color(p.muted_text)).truncate().selectable(false));
                }
            });
        }
    });
    let resp = ui.interact(inner.response.rect, ui.id().with("discuss-anchor-click"), Sense::click()).on_hover_text("Show in diff");
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp.clicked() && intent.is_none() {
        store.scroll_to_anchor(&t.anchor);
    }
}

fn bottom_area(ui: &mut Ui, store: &mut PRStore, state: &mut DiscussPanelState, t: &DiscussThread, intent: &mut Option<Intent>, p: &Palette) {
    let id = t.id.as_str();
    let local_count = store.local_commits.len();
    let fix_commits: Vec<(String, Option<String>)> = t
        .fix_commits
        .iter()
        .map(|sha| (sha.clone(), store.local_commits.iter().find(|c| &c.sha == sha).map(|c| c.subject.clone())))
        .collect();
    let c = store.controller(id);
    let has_answer = c.messages().iter().any(|m| m.role == Role::Assistant);
    let running = c.is_running();
    let reducer_running = c.reducer.is_running();
    let drafting = c.draft_state.is_running();

    egui::Frame::new().inner_margin(Margin { left: 10, right: 10, top: 8, bottom: 10 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 6.0;

        // Commits made by "Fix it" from this conversation.
        if !fix_commits.is_empty() {
            ui.label(RichText::new("Commits from this conversation").size(12.0).strong().color(p.muted_text));
            for (sha, subject) in &fix_commits {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let short: String = sha.chars().take(7).collect();
                    ui.label(RichText::new(short).font(FontId::monospace(12.0)).color(p.accent));
                    match subject {
                        Some(s) => {
                            ui.add(egui::Label::new(RichText::new(s).size(12.5).color(p.text)).truncate());
                        }
                        None => {
                            ui.label(RichText::new("pushed").size(12.0).color(p.muted_text));
                        }
                    }
                });
            }
        }
        if local_count > 0 {
            ui.label(
                RichText::new(format!(
                    "{local_count} local commit{} not pushed yet. Push them with “{local_count} local” in the toolbar.",
                    if local_count == 1 { "" } else { "s" }
                ))
                .size(12.0)
                .color(p.attention),
            );
        }

        // Actions.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let label = if drafting { "Drafting…" } else { "→ Comment" };
            if super::common::gh_button_ex(ui, label, &["Drafting…", "→ Comment"], BtnKind::Default, has_answer && !running && !drafting)
                .on_hover_text("Draft a review comment from this conversation")
                .clicked()
            {
                *intent = Some(Intent::Convert);
            }
            let fix_label = if c.is_fixing() { "Fixing…" } else { "Fix it" };
            if super::common::gh_button_ex(ui, fix_label, &["Fixing…", "Fix it"], BtnKind::Ai, has_answer && !running)
                .on_hover_text("Ask Claude to implement the fix and commit it to the PR branch (not pushed)")
                .clicked()
            {
                *intent = Some(Intent::FixIt);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let TaskState::Failed(e) = &c.draft_state {
                    ui.label(RichText::new("⚠").color(p.del_marker)).on_hover_text(e);
                }
            });
        });

        // Input box.
        let mut focused = false;
        let frame = egui::Frame::new()
            .fill(p.subtle.gamma_multiply(0.5))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(5, 5));
        let edit_id = ui.id().with(("discuss-input", id));
        let has_focus = ui.memory(|m| m.has_focus(edit_id));
        // Focus changes the border color only: a thicker border would shift the text.
        let frame = frame.stroke(Stroke::new(1.0, if has_focus { p.ai_accent } else { p.border }));
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible).max_height(160.0).auto_shrink([false, true]).show(ui, |ui| {
                let hint = if c.messages().is_empty() { "Ask about these lines…" } else { "Reply to Claude…" };
                let resp = ui.add(
                    egui::TextEdit::multiline(&mut c.input)
                        .id(edit_id)
                        .hint_text(RichText::new(hint).color(p.muted_text))
                        .font(FontId::proportional(14.0))
                        .text_color(p.text)
                        .frame(egui::Frame::NONE)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter)),
                );
                if c.wants_focus || state.focused_for.as_deref() != Some(id) {
                    resp.request_focus();
                    c.wants_focus = false;
                    state.focused_for = Some(id.to_string());
                }
                focused = resp.has_focus();
            });
        });
        let can_send = !c.input.trim().is_empty() && !running;
        let enter = focused && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift);

        ui.horizontal(|ui| {
            ui.label(RichText::new("↩ send · ⇧↩ newline").size(11.5).color(p.muted_text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let send = gh_button(ui, "↑ Send", BtnKind::Ai, can_send).clicked();
                if (send || enter) && can_send {
                    *intent = Some(Intent::Send(std::mem::take(&mut c.input)));
                }
                if reducer_running {
                    let stop = gh_button(ui, "■ Stop", BtnKind::Danger, true).on_hover_text("Stop (⌘.)").clicked();
                    if stop || ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Period)) {
                        *intent = Some(Intent::Stop);
                    }
                }
            });
        });
    });
}

fn message(ui: &mut Ui, m: &DiscussMessage, expanded: &mut HashSet<String>, p: &Palette) {
    match m.role {
        Role::User => {
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                let max_w = (ui.available_width() - 30.0).max(80.0);
                egui::Frame::new()
                    .fill(blend(p.accent, p.canvas, 0.12))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(10, 7))
                    .show(ui, |ui| {
                        ui.set_max_width(max_w - 20.0);
                        ui.with_layout(Layout::top_down(Align::Min), |ui| {
                            ui.add(egui::Label::new(RichText::new(&m.text).size(14.0).color(p.text)).wrap());
                        });
                    });
            });
        }
        Role::Assistant => {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.add_space(0.0);
                super::icons::icon(ui, super::icons::Icon::Sparkle, 13.0, p.ai_accent);
                let w = ui.available_width();
                ui.allocate_ui_with_layout(Vec2::new(w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_max_width(w);
                    if m.is_streaming {
                        ui.multiply_opacity(0.92);
                    }
                    markdown(ui, &m.text, &[]);
                });
            });
        }
        Role::Tool => tool_row(ui, m, expanded, p),
        Role::Error => {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new("⚠").size(13.0).color(p.del_marker));
                ui.add(egui::Label::new(RichText::new(&m.text).size(13.0).color(p.del_marker)).wrap());
            });
        }
        Role::System => {
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(&m.text).size(12.0).color(p.muted_text));
            });
        }
    }
}

fn tool_row(ui: &mut Ui, m: &DiscussMessage, expanded: &mut HashSet<String>, p: &Palette) {
    let open = expanded.contains(&m.id);
    let mono = FontId::monospace(12.5);
    ui.horizontal(|ui| {
        ui.add_space(20.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            let row = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    super::icons::icon(ui, if open { super::icons::Icon::ChevronDown } else { super::icons::Icon::ChevronRight }, 11.0, p.muted_text);
                    ui.label(RichText::new(tool_icon(m.tool_name.as_deref())).font(mono.clone()).color(p.muted_text));
                    let reserve = if m.tool_result.is_none() || m.tool_failed { 22.0 } else { 0.0 };
                    let w = (ui.available_width() - reserve).max(40.0);
                    ui.allocate_ui_with_layout(Vec2::new(w, 16.0), Layout::left_to_right(Align::Center), |ui| {
                        ui.set_max_width(w);
                        ui.add(egui::Label::new(RichText::new(m.tool_summary()).font(mono.clone()).color(p.muted_text)).truncate().selectable(false));
                    });
                    if m.tool_result.is_none() {
                        spinner(ui);
                    }
                    if m.tool_failed {
                        super::icons::icon(ui, super::icons::Icon::Close, 12.0, p.del_marker);
                    }
                    ui.allocate_space(Vec2::new(ui.available_width(), 0.0));
                })
                .response;
            let resp = ui.interact(row.rect, ui.id().with(("tool", &m.id)), Sense::click());
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.clicked() {
                if open {
                    expanded.remove(&m.id);
                } else {
                    expanded.insert(m.id.clone());
                }
            }
            if open {
                let text = format!("{}\n\n{}", m.tool_input.as_deref().unwrap_or(""), m.tool_result.as_deref().unwrap_or("…"));
                egui::Frame::new().fill(p.subtle).corner_radius(CornerRadius::same(6)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::both().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible).id_salt(("tool-body", &m.id)).max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                        egui::Frame::new().inner_margin(Margin::same(8)).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(text).font(FontId::monospace(12.0)).color(p.text)).extend());
                        });
                    });
                });
            }
        });
    });
}

fn tool_icon(name: Option<&str>) -> &'static str {
    match name {
        Some("Read") => "≡",
        Some("Edit" | "Write" | "MultiEdit") => "✎",
        Some("Bash") => "$",
        Some("Grep" | "Glob") => "⌕",
        _ => "⚙",
    }
}

/// Three pulsing dots while Claude is thinking.
fn typing_indicator(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(60.0, 10.0), Sense::hover());
    let t = ui.input(|i| i.time) as f32;
    let phase = (t / 1.2).fract() * std::f32::consts::TAU;
    for i in 0..3 {
        let a = 0.3 + 0.7 * (phase + i as f32 * 0.8).sin().max(0.0);
        let center = egui::pos2(rect.left() + 22.0 + i as f32 * 9.0, rect.center().y);
        ui.painter().circle_filled(center, 2.5, p.ai_accent.gamma_multiply(a));
    }
    ui.ctx().request_repaint();
}

fn blend(fg: Color32, bg: Color32, a: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 * a + y as f32 * (1.0 - a)).round() as u8;
    Color32::from_rgb(mix(fg.r(), bg.r()), mix(fg.g(), bg.g()), mix(fg.b(), bg.b()))
}
