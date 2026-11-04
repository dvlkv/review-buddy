//! Inline blocks drawn between diff rows: review threads, comment composers,
//! Discuss markers and AI pre-review findings.

use egui::{Color32, Id, Key, Modifiers, RichText, Stroke, Ui};
use review_buddy_core::claude::transcript::Role;
use review_buddy_core::diff::layout::{InlineBlockKind, InlineBlockRef};
use review_buddy_core::review::markdown::MarkdownParser;
use review_buddy_core::*;

use super::common::{avatar, card, gh_button, pal, pill, relative, spinner, suggestion_preview, BtnKind};
use super::markdown::markdown;
use crate::pr_store::{Composer, ComposerKind, PRStore};

pub enum BlockAction {
    OpenDiscuss,
}

pub fn block(ui: &mut Ui, store: &mut PRStore, b: &InlineBlockRef) -> Option<BlockAction> {
    match b.kind {
        InlineBlockKind::ReviewThread => {
            thread_block(ui, store, &b.id);
            None
        }
        InlineBlockKind::Composer => {
            composer_block(ui, store, &b.id);
            None
        }
        InlineBlockKind::Discuss => discuss_marker(ui, store, &b.id),
        InlineBlockKind::Finding => {
            finding_block(ui, store, &b.id);
            None
        }
    }
}

fn thread_block(ui: &mut Ui, store: &mut PRStore, id: &str) {
    let Some(t) = store.threads.get(id).cloned() else { return };
    let p = pal(ui);
    let exp_id = Id::new(("resolved_expanded", id));
    let expanded: bool = ui.data(|d| d.get_temp(exp_id).unwrap_or(false));
    let collapsed = t.is_resolved && !expanded;
    card(ui, p.canvas, p.border, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        if t.is_resolved || t.is_outdated {
            egui::Frame::new().fill(p.subtle).inner_margin(egui::Margin::symmetric(12, 7)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if t.is_resolved {
                        super::icons::label(ui, "✓ Resolved", 13.0, ui.visuals().text_color());
                    }
                    if t.is_outdated {
                        pill(ui, "Outdated", p.attention, false);
                    }
                    if collapsed {
                        if let Some(first) = t.comments.first() {
                            let w = (ui.available_width() - 60.0).max(40.0);
                            let text = RichText::new(format!("@{}: {}", first.author, first.body.replace('\n', " "))).size(13.0).color(p.muted_text);
                            ui.allocate_ui_with_layout(egui::vec2(w, 18.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.add(egui::Label::new(text).truncate());
                            });
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if t.is_resolved && super::common::gh_button_ex(ui, if collapsed { "Show" } else { "Hide" }, &["Show", "Hide"], BtnKind::Invisible, true).clicked() {
                            ui.data_mut(|d| d.insert_temp(exp_id, !expanded));
                        }
                    });
                });
            });
        }
        if collapsed {
            return;
        }
        for c in &t.comments {
            comment_view(ui, store, &t, c);
            ui.separator();
        }
        let reply = store.composer_for_reply(id).cloned();
        match reply {
            Some(c) => {
                egui::Frame::new().inner_margin(egui::Margin::same(10)).show(ui, |ui| inline_composer(ui, store, &c, "Reply…"));
            }
            None => {
                egui::Frame::new().fill(p.subtle).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let w = (ui.available_width() - 190.0).max(80.0);
                        let r = ui.add_sized(
                            [w, 26.0],
                            egui::Button::new(RichText::new("Reply…").color(p.muted_text)).fill(p.canvas).stroke(Stroke::new(1.0, p.border)),
                        );
                        if r.clicked() {
                            store.start_reply(id);
                        }
                        let title = if t.is_resolved { "Unresolve conversation" } else { "Resolve conversation" };
                        if gh_button(ui, title, BtnKind::Default, true).clicked() {
                            store.set_resolved(id, !t.is_resolved);
                        }
                    });
                });
            }
        }
    });
}

fn comment_view(ui: &mut Ui, store: &mut PRStore, t: &ReviewThread, c: &ReviewComment) {
    let p = pal(ui);
    egui::Frame::new().inner_margin(egui::Margin::same(12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.horizontal(|ui| {
            avatar(ui, &c.author, 20.0);
            ui.label(RichText::new(&c.author).strong().size(13.0));
            ui.label(RichText::new(relative(&c.created_at)).size(13.0).color(p.muted_text));
            if c.is_pending {
                pill(ui, "Pending", p.attention, false);
            }
            if c.viewer_did_author {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.menu_button("⋯", |ui| {
                        if ui.button("Edit").clicked() {
                            store.start_edit(&t.id, c);
                            ui.close();
                        }
                        if ui.button(RichText::new("Delete").color(p.del_marker)).clicked() {
                            store.delete_comment(&t.id, &c.id);
                            ui.close();
                        }
                        if let Some(url) = &c.url {
                            if ui.button("Copy link").clicked() {
                                ui.ctx().copy_text(url.clone());
                                ui.close();
                            }
                        }
                    });
                });
            }
        });
        match store.composer_for_edit(&c.id).cloned() {
            Some(editor) => inline_composer(ui, store, &editor, "Edit comment"),
            None => {
                let ctx = suggestion_context(store, t);
                markdown(ui, &c.body, &ctx);
            }
        }
    });
}

fn suggestion_context(store: &PRStore, t: &ReviewThread) -> Vec<String> {
    let Some(line) = t.line else { return vec![] };
    store.lines_for(&CodeAnchor::new(t.path.clone(), t.side, t.start_line.unwrap_or(line), line))
}

fn composer_block(ui: &mut Ui, store: &mut PRStore, id: &str) {
    let Some(c) = store.composers.get(id).cloned() else { return };
    let p = pal(ui);
    card(ui, p.canvas, p.border, |ui| {
        ui.set_width(ui.available_width());
        if let Some(a) = c.anchor() {
            egui::Frame::new().inner_margin(egui::Margin { left: 12, right: 12, top: 10, bottom: 0 }).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let s = if a.side == DiffSide::Left { "L" } else { "R" };
                    let text = if a.is_multiline() {
                        format!("Commenting on lines {s}{} to {s}{}", a.start_line, a.end_line)
                    } else {
                        format!("Commenting on line {s}{}", a.start_line)
                    };
                    ui.label(RichText::new(text).size(13.0).color(p.muted_text));
                    if let Some(o) = &c.origin {
                        pill(ui, o, p.ai_accent, false);
                    }
                });
            });
        }
        egui::Frame::new().inner_margin(egui::Margin::same(10)).show(ui, |ui| inline_composer(ui, store, &c, "Leave a comment"));
    });
}

/// Write/Preview editor with suggestion insertion; ⌘↩ submits, ⇧⌘↩ single, Esc cancels.
pub fn inline_composer(ui: &mut Ui, store: &mut PRStore, c: &Composer, placeholder: &str) {
    let p = pal(ui);
    let preview_id = Id::new(("composer_preview", &c.id));
    let preview: bool = ui.data(|d| d.get_temp(preview_id).unwrap_or(false));
    let offers_single = match c.kind {
        ComposerKind::NewThread(_) | ComposerKind::Reply { .. } => store.pending_review_id.is_none(),
        ComposerKind::Edit { .. } => false,
    };
    let submit_title = match c.kind {
        ComposerKind::Edit { .. } => "Update comment",
        _ if store.pending_review_id.is_none() => "Start a review",
        _ => "Add review comment",
    };
    let mut text = c.text.clone();
    let mut submit: Option<bool> = None;
    let mut cancel = false;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.horizontal(|ui| {
            let mut tab = preview;
            if super::icons::segmented(ui, &mut tab, &[(false, "Write"), (true, "Preview")], 12.0) {
                ui.data_mut(|d| d.insert_temp(preview_id, tab));
            }
            if c.anchor().is_some() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("±").on_hover_text("Insert a suggestion").clicked() {
                        if !text.is_empty() && !text.ends_with('\n') {
                            text.push('\n');
                        }
                        text.push_str(&MarkdownParser::suggestion_template(&c.anchor_lines));
                    }
                });
            }
        });
        if preview {
            egui::Frame::new().inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.set_min_height(80.0);
                let t = if text.is_empty() { "_Nothing to preview_" } else { &text };
                markdown(ui, t, &c.anchor_lines);
            });
        } else {
            let lines = text.matches('\n').count() + 1;
            let rows = lines.clamp(4, 16);
            let edit_id = Id::new(("composer_edit", &c.id));
            let r = ui.add(
                egui::TextEdit::multiline(&mut text)
                    .id(edit_id)
                    .hint_text(placeholder)
                    .desired_rows(rows)
                    .desired_width(f32::INFINITY)
                    .font(egui::FontId::proportional(14.0)),
            );
            if c.wants_focus {
                r.request_focus();
            }
            if r.has_focus() {
                let (cmd_enter, shift, esc) = ui.input_mut(|i| {
                    let shift = i.modifiers.shift;
                    let ce = i.consume_key(Modifiers::COMMAND, Key::Enter) || i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Enter);
                    (ce, shift, i.consume_key(Modifiers::NONE, Key::Escape))
                });
                if cmd_enter {
                    submit = Some(offers_single && shift);
                }
                if esc {
                    cancel = true;
                }
            }
        }
        if let Some(e) = &c.error {
            ui.label(RichText::new(e).size(13.0).color(p.del_marker));
        }
        ui.horizontal(|ui| {
            let hint = if offers_single { "Markdown · ⌘↩ review · ⇧⌘↩ single" } else { "Markdown supported · ⌘↩ to submit" };
            ui.label(RichText::new(hint).size(12.0).color(p.muted_text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ok = !text.trim().is_empty() && !c.is_submitting;
                if super::common::gh_button_busy(ui, submit_title, &[], BtnKind::Primary, ok, c.is_submitting).clicked() {
                    submit = Some(false);
                }
                if offers_single && gh_button(ui, "Add single comment", BtnKind::Default, ok).on_hover_text("Publish now, outside a review (⇧⌘↩)").clicked() {
                    submit = Some(true);
                }
                if gh_button(ui, "Cancel", BtnKind::Default, true).clicked() {
                    cancel = true;
                }
            });
        });
    });
    if let Some(cm) = store.composers.get_mut(&c.id) {
        cm.wants_focus = false;
        if cm.text != text {
            cm.text = text;
        }
    }
    if cancel {
        store.cancel_composer(&c.id);
    } else if let Some(single) = submit {
        store.submit_composer(&c.id, single);
    }
}

fn discuss_marker(ui: &mut Ui, store: &mut PRStore, id: &str) -> Option<BlockAction> {
    let t = store.discuss_threads.get(id)?.clone();
    let p = pal(ui);
    let active = store.active_discuss_id.as_deref() == Some(id);
    let running = store.controllers.get(id).map(|c| c.is_running()).unwrap_or(false);
    let fix = store.fix_status(id).cloned();
    let stroke = Stroke::new(if active { 1.5 } else { 1.0 }, p.ai_accent.gamma_multiply(if active { 0.9 } else { 0.35 }));
    let r = egui::Frame::new()
        .fill(p.ai_bg)
        .stroke(stroke)
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                super::icons::label(ui, "✦ Discuss", 13.0, p.ai_accent);
                let title = if t.messages.is_empty() { format!("New conversation — ask Claude about {}", t.anchor.label()) } else { t.title() };
                ui.add(egui::Label::new(RichText::new(title).size(13.0).color(p.text)).truncate());
                if t.status == DiscussStatus::Outdated {
                    pill(ui, "Outdated", p.attention, false);
                }
                if let Some(f) = &fix {
                    fix_badge(ui, f);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let n = t.messages.iter().filter(|m| m.role == Role::User || m.role == Role::Assistant).count();
                    if n > 0 {
                        ui.label(RichText::new(format!("{n} messages")).size(13.0).color(p.muted_text));
                    }
                    if running {
                        spinner(ui);
                    }
                });
            });
        });
    let resp = ui.interact(r.response.rect, Id::new(("discuss_marker", id)), egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp.clicked() {
        store.active_discuss_id = Some(id.to_string());
        return Some(BlockAction::OpenDiscuss);
    }
    None
}

pub fn fix_badge(ui: &mut Ui, fix: &FixRequest) {
    let p = pal(ui);
    match fix.status {
        FixStatus::Queued => pill(ui, "Fix queued", p.muted_text, false),
        FixStatus::Running => pill(ui, "Fixing…", p.ai_accent, false),
        FixStatus::Committed => {
            let sha: String = fix.commits.first().map(|s| s.chars().take(7).collect()).unwrap_or_default();
            pill(ui, &format!("Fixed in {sha}"), p.add_marker, false)
        }
        FixStatus::Failed => pill(ui, "Fix failed", p.del_marker, false).on_hover_text(fix.error.clone().unwrap_or_default()),
    };
}

pub fn severity_pill(ui: &mut Ui, s: Severity) {
    let p = pal(ui);
    match s {
        Severity::High => pill(ui, "High", p.del_marker, true),
        Severity::Medium => pill(ui, "Medium", p.attention, true),
        Severity::Low => pill(ui, "Low", p.accent, false),
        Severity::Nit => pill(ui, "Nit", p.muted_text, false),
    };
}

fn finding_block(ui: &mut Ui, store: &mut PRStore, id: &str) {
    let Some(f) = store.findings.get(id).cloned() else { return };
    let p = pal(ui);
    let outdated = store.findings_are_outdated();
    let r = egui::Frame::new().fill(p.ai_bg).corner_radius(6).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.horizontal(|ui| {
            super::icons::label(ui, "✦ AI pre-review", 12.0, p.ai_accent);
            severity_pill(ui, f.severity);
            if outdated {
                pill(ui, "Outdated", p.attention, false).on_hover_text("The PR changed since this pre-review ran");
            }
        });
        ui.label(RichText::new(&f.title).strong().size(14.0).color(p.text));
        markdown(ui, &f.body, &[]);
        if let Some(s) = &f.suggestion {
            let old = store.lines_for(&f.anchor());
            let new: Vec<String> = s.split('\n').map(str::to_string).collect();
            suggestion_preview(ui, &old, &new);
        }
        let compact = ui.available_width() < 520.0;
        ui.horizontal(|ui| {
            if gh_button(ui, if compact { "Accept" } else { "Accept as comment" }, BtnKind::Primary, true).clicked() {
                store.accept_finding(id);
            }
            if gh_button(ui, "✦ Discuss", BtnKind::Default, true).clicked() {
                store.discuss_finding(id);
            }
            if gh_button(ui, "Dismiss", BtnKind::Invisible, true).clicked() {
                store.dismiss_finding(id);
            }
            if !compact {
                ui.label(RichText::new("Private — only you can see this").size(12.0).color(p.muted_text));
            }
        });
    });
    dashed_border(ui, r.response.rect, p.ai_accent.gamma_multiply(0.45));
}

fn dashed_border(ui: &Ui, r: egui::Rect, c: Color32) {
    let painter = ui.painter();
    let s = Stroke::new(1.0, c);
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    painter.extend(egui::Shape::dashed_line(&pts, s, 4.0, 3.0));
}
