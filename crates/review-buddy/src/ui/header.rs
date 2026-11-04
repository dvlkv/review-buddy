//! PR header above the diff: state, branches, description, AI summary card, and the
//! status bars (new commits on GitHub, local fix commits, checkout errors, messages).

use std::time::{Duration, Instant};

use egui::{Color32, Id, RichText, Stroke, Ui};
use review_buddy_core::*;

use super::common::{avatar, gh_button, pal, pill, spinner, BtnKind};
use super::markdown::{inline, markdown};
use crate::pr_store::{PRStore, TaskState, WorktreeState};
use crate::theme::Palette;

#[derive(Clone, Default)]
struct HeaderMem {
    show_description: bool,
    summary_collapsed: bool,
    summary_expanded: bool,
    banner: Option<(String, f64)>,
}

/// Returns true when the Discuss panel should open.
pub fn pr_header(ui: &mut Ui, store: &mut PRStore) -> bool {
    let Some(d) = store.detail.clone() else { return false };
    let p = pal(ui);
    let mem_id = Id::new(("pr_header", store.pr_ref.to_string()));
    let mut mem: HeaderMem = ui.data(|m| m.get_temp(mem_id).unwrap_or_default());
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        state_badge(ui, &d, &p);
        avatar(ui, &d.author, 18.0);
        ui.label(RichText::new(&d.author).strong().size(12.0));
        ui.label(RichText::new("wants to merge into").size(12.0).color(p.muted_text));
        branch(ui, &d.base_ref_name, &p);
        ui.label(RichText::new("←").size(11.0).color(p.muted_text));
        branch(ui, &d.head_ref_name, &p);
        ui.label(RichText::new(format!("+{}", d.additions)).size(12.0).color(p.add_marker));
        ui.label(RichText::new(format!("−{}", d.deletions)).size(12.0).color(p.del_marker));
        let files = if d.changed_files == 1 { "1 file".to_string() } else { format!("{} files", d.changed_files) };
        ui.label(RichText::new(files).size(12.0).color(p.muted_text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Always here: shows/hides the summary, or generates one if there is none.
            let has_summary = store.summary.is_some() || store.summary_state.is_running();
            let shown = has_summary && !mem.summary_collapsed;
            let r = gh_button(ui, if shown { "▴ Hide summary" } else { "✦ AI summary" }, BtnKind::Invisible, true)
                .on_hover_text(if has_summary { "Show or hide the AI summary" } else { "Generate an AI summary and reading order" });
            if r.clicked() {
                if has_summary {
                    mem.summary_collapsed = !mem.summary_collapsed;
                } else {
                    mem.summary_collapsed = false;
                    store.start_summarize();
                }
            }
            if !d.body.is_empty() {
                let t = if mem.show_description { "▴ Hide description" } else { "☰ Description" };
                if gh_button(ui, t, BtnKind::Invisible, true).clicked() {
                    mem.show_description = !mem.show_description;
                }
            }
        });
    });
    let max_h = (ui.available_height() * 0.45).max(160.0);
    let any_body = mem.show_description || (!mem.summary_collapsed && (store.summary.is_some() || store.summary_state != TaskState::Idle));
    if any_body {
        egui::ScrollArea::vertical().id_salt("header_body").max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
            if mem.show_description {
                egui::Frame::new().stroke(Stroke::new(1.0, p.border)).corner_radius(6).inner_margin(12).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    markdown(ui, &d.body, &[]);
                });
                ui.add_space(6.0);
            }
            if !mem.summary_collapsed {
                summary_card(ui, store, &mut mem, &p);
            }
        });
    }
    git_status(ui, store, &p);
    banner(ui, store, &mut mem, &p);
    ui.add_space(6.0);
    ui.data_mut(|m| m.insert_temp(mem_id, mem));
    false
}

fn state_badge(ui: &mut Ui, d: &PRDetail, p: &Palette) {
    if d.is_draft {
        pill(ui, "Draft", p.muted_text, true);
    } else if d.state == "MERGED" {
        pill(ui, "Merged", p.ai_accent, true);
    } else if d.state == "CLOSED" {
        pill(ui, "Closed", p.del_marker, true);
    } else {
        pill(ui, "Open", Color32::from_rgb(0x1F, 0x88, 0x3D), true);
    }
}

pub fn branch(ui: &mut Ui, name: &str, p: &Palette) {
    egui::Frame::new().fill(p.accent.gamma_multiply(0.1)).corner_radius(5).inner_margin(egui::Margin::symmetric(6, 1)).show(ui, |ui| {
        let r = ui.add(egui::Label::new(RichText::new(name).monospace().size(11.5).color(p.accent)).sense(egui::Sense::click()));
        if r.on_hover_text("Click to copy").clicked() {
            ui.ctx().copy_text(name.to_string());
        }
    });
}

fn summary_card(ui: &mut Ui, store: &mut PRStore, mem: &mut HeaderMem, p: &Palette) {
    if store.summary.is_none() && store.summary_state == TaskState::Idle {
        return;
    }
    let mut go_to: Option<String> = None;
    egui::Frame::new()
        .fill(p.ai_bg)
        .stroke(Stroke::new(1.0, p.ai_accent.gamma_multiply(0.3)))
        .corner_radius(8)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.horizontal(|ui| {
                ui.label(RichText::new("✦ AI summary").strong().size(12.0).color(p.ai_accent));
                if let TaskState::Running(msg) = &store.summary_state {
                    spinner(ui);
                    ui.label(RichText::new(msg).size(11.0).color(p.muted_text));
                    if ui.link(RichText::new("Cancel").size(11.0)).clicked() {
                        store.cancel_summarize();
                    }
                }
                if let Some(s) = &store.summary {
                    if s.head_sha.as_deref() != Some(store.detail.as_ref().map(|d| d.head_oid.as_str()).unwrap_or("")) {
                        pill(ui, "Outdated", p.attention, false);
                    }
                }
                if store.summary.is_some() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let r = ui.add_enabled(!store.summary_state.is_running(), egui::Button::new("↻").frame(false)).on_hover_text("Regenerate");
                        if r.clicked() {
                            store.start_summarize();
                        }
                    });
                }
            });
            if let TaskState::Failed(e) = &store.summary_state {
                ui.label(RichText::new(e).size(12.0).color(p.del_marker));
            }
            let Some(s) = store.summary.clone() else { return };
            let text = if mem.summary_expanded || s.summary.chars().count() <= 240 {
                s.summary.clone()
            } else {
                format!("{}…", s.summary.chars().take(240).collect::<String>())
            };
            inline(ui, &text, 13.0, p.text, false);
            if s.summary.chars().count() > 240 && ui.link(if mem.summary_expanded { "Show less" } else { "Show more" }).clicked() {
                mem.summary_expanded = !mem.summary_expanded;
            }
            if !s.reading_order.is_empty() {
                ui.label(RichText::new("Suggested reading order").strong().size(11.0).color(p.muted_text));
                let n = if mem.summary_expanded { 50 } else { 5 };
                for (i, item) in s.reading_order.iter().take(n).enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_sized([18.0, 16.0], egui::Label::new(RichText::new(format!("{}.", i + 1)).size(12.0).color(p.muted_text)));
                        if ui.add(egui::Label::new(RichText::new(&item.path).monospace().size(12.0).color(p.accent)).sense(egui::Sense::click())).clicked() {
                            go_to = Some(item.path.clone());
                        }
                        ui.add(egui::Label::new(RichText::new(format!("— {}", item.reason)).size(12.0).color(p.muted_text)).truncate());
                    });
                }
            }
            if !s.risks.is_empty() {
                ui.label(RichText::new("Risky areas").strong().size(11.0).color(p.muted_text));
                let n = if mem.summary_expanded { 50 } else { 3 };
                for r in s.risks.iter().take(n) {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("⚠").size(11.0).color(p.attention));
                        match &r.path {
                            Some(path) => {
                                if ui.add(egui::Label::new(RichText::new(path).monospace().size(12.0).color(p.accent)).sense(egui::Sense::click())).clicked() {
                                    go_to = Some(path.clone());
                                }
                            }
                            None => {
                                ui.label(RichText::new("General").size(12.0).strong());
                            }
                        }
                    });
                    ui.indent(Id::new(("risk", &r.description)), |ui| inline(ui, &r.description, 12.0, p.text, false));
                }
            }
        });
    if let Some(path) = go_to {
        if let Some(f) = store.diff.as_ref().and_then(|d| d.file_index(&path)) {
            store.scroll_to_file(f);
        }
    }
}

fn bar(ui: &mut Ui, tint: Color32, add: impl FnOnce(&mut Ui)) {
    ui.add_space(4.0);
    egui::Frame::new().fill(tint.gamma_multiply(0.1)).corner_radius(6).inner_margin(8).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(add);
    });
}

/// New commits on GitHub, local fix commits vs. remote, checkout failures (§5.1, §5.4).
fn git_status(ui: &mut Ui, store: &mut PRStore, p: &Palette) {
    if store.is_outdated() {
        bar(ui, p.accent, |ui| {
            ui.label(RichText::new("⟳ This pull request has new commits on GitHub.").size(12.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if gh_button(ui, "Refresh", BtnKind::Primary, true).on_hover_text("Reload the diff and update the local checkout (⌘R)").clicked() {
                    store.refresh();
                }
            });
        });
    }
    let (ahead, local) = (store.remote_ahead, store.local_commits.len());
    if ahead > 0 && local > 0 {
        bar(ui, p.attention, |ui| {
            let s = |n: usize| if n == 1 { "" } else { "s" };
            ui.label(
                RichText::new(format!("⑂ The PR was updated on GitHub ({ahead} new commit{}) · you have {local} local fix commit{}", s(ahead), s(local)))
                    .size(12.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if gh_button(ui, "Rebase local commits", BtnKind::Default, true).clicked() {
                    store.rebase_onto_remote();
                }
            });
        });
    } else if ahead > 0 && !store.is_outdated() {
        bar(ui, p.attention, |ui| {
            ui.label(
                RichText::new(format!(
                    "⑂ The local checkout is {ahead} commit{} behind the PR: it has uncommitted changes, so it wasn't updated.",
                    if ahead == 1 { "" } else { "s" }
                ))
                .size(12.0),
            );
        });
    }
    if let WorktreeState::Failed(e) = store.worktree_state() {
        bar(ui, p.del_marker, |ui| {
            ui.label(RichText::new(format!("⚠ Checkout failed: {e}")).size(12.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if gh_button(ui, "Retry", BtnKind::Default, true).clicked() {
                    store.wt.set(WorktreeState::Idle);
                    store.ensure_worktree_then(|_, _| {});
                }
            });
        });
    }
}

/// Transient messages from store actions; they disappear after 6 seconds.
fn banner(ui: &mut Ui, store: &mut PRStore, mem: &mut HeaderMem, p: &Palette) {
    let Some(text) = store.banner.clone() else {
        mem.banner = None;
        return;
    };
    let now = ui.input(|i| i.time);
    match &mem.banner {
        Some((t, _)) if *t == text => {}
        _ => mem.banner = Some((text.clone(), now)),
    }
    let shown_at = mem.banner.as_ref().map(|b| b.1).unwrap_or(now);
    if now - shown_at > 6.0 {
        store.banner = None;
        mem.banner = None;
        return;
    }
    ui.ctx().request_repaint_after(Duration::from_millis(500));
    let _ = Instant::now();
    let mut close = false;
    bar(ui, p.accent, |ui| {
        ui.label(RichText::new(format!("ⓘ {text}")).size(12.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new("✕").frame(false)).clicked() {
                close = true;
            }
        });
    });
    if close {
        store.banner = None;
        mem.banner = None;
    }
}
