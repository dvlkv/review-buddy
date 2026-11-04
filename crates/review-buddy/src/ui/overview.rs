//! The PR's Conversation and Commits (as on GitHub), plus the AI pre-review. Shown in
//! place of the diff when `store.show_overview` is set (the "Conversation" row).

use std::sync::{Arc, Mutex};

use egui::{Color32, CornerRadius, FontId, Margin, RichText, Sense, Stroke, Ui, Vec2};
use review_buddy_core::models::{
    CheckState, ConversationComment, ConversationReview, MergeCheck, MergeMethod, PRDetail, PRMergeStatus, ReviewThread,
    Severity,
};
use review_buddy_core::Date;

use super::common::{avatar, card, gh_button, pal, pill, relative, spinner, BtnKind};
use super::markdown::markdown;
use crate::pr_store::{PRStore, TaskState};
use crate::theme::Palette;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Conversation,
    Commits,
}

/// Results of async store calls; callbacks can only reach the store, so they write here.
#[derive(Default)]
struct Shared {
    confirming: bool,
    merging: bool,
    merge_error: Option<String>,
    posting: bool,
    closing: bool,
    comment_error: Option<String>,
    /// Set when a comment was posted; the text box is cleared on the next frame.
    clear_comment: bool,
}

#[derive(Default)]
pub struct OverviewState {
    tab: Tab,
    /// The PR the transient state below belongs to; reset when it changes.
    pr_key: String,
    hide_checks: bool,
    /// The merge method picked from the split button's menu (session-wide, like Swift's @AppStorage).
    method: Option<MergeMethod>,
    bypass: bool,
    comment: String,
    shared: Arc<Mutex<Shared>>,
}

enum Action {
    Back,
    Refresh,
    Scroll(String),
    StartPreReview,
    CancelPreReview,
    Merge(MergeMethod, bool),
    Comment,
    SetClosed(bool),
}

pub fn overview(ui: &mut Ui, store: &mut PRStore, state: &mut OverviewState) {
    let key = store.pr_ref.to_string();
    if state.pr_key != key {
        let method = state.method;
        *state = OverviewState { pr_key: key, method, ..Default::default() };
    }
    if std::mem::take(&mut state.shared.lock().unwrap().clear_comment) {
        state.comment.clear();
    }
    let p = pal(ui);
    let mut actions = vec![];
    egui::ScrollArea::vertical().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible).auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(Margin::symmetric(4, 12)).show(ui, |ui| {
            ui.set_max_width(900.0_f32.min(ui.available_width()));
            ui.spacing_mut().item_spacing.y = 16.0;
            header(ui, store, state, &mut actions);
            outdated_bar(ui, store, &p, &mut actions);
            pre_review_card(ui, store, &p, &mut actions);
            match state.tab {
                Tab::Conversation => {
                    timeline(ui, store, &p, &mut actions);
                    // GitHub's order: timeline, merge box, then the comment box.
                    merge_box(ui, store, state, &p, &mut actions);
                    comment_composer(ui, store, state, &p, &mut actions);
                }
                Tab::Commits => commit_list(ui, store, &p),
            }
        });
    });
    for a in actions {
        apply(a, store, state);
    }
}

fn apply(a: Action, store: &mut PRStore, state: &mut OverviewState) {
    match a {
        Action::Back => store.show_overview = false,
        Action::Refresh => store.refresh(),
        Action::Scroll(id) => {
            let in_layout = store.layout.as_ref().is_some_and(|l| l.blocks.iter().any(|b| b.id == id));
            if in_layout {
                store.scroll_to_block(&id);
            } else if let Some(anchor) = anchor_for(store, &id) {
                // Collapsed files have no block rows; this expands the file first.
                store.scroll_to_anchor(&anchor);
            } else {
                store.show_overview = false;
            }
        }
        Action::StartPreReview => store.start_pre_review(),
        Action::CancelPreReview => store.cancel_pre_review(),
        Action::Merge(method, bypass) => {
            {
                let mut sh = state.shared.lock().unwrap();
                sh.merging = true;
                sh.merge_error = None;
            }
            let shared = state.shared.clone();
            store.merge_pull_request(method, bypass, move |_, err| {
                let mut sh = shared.lock().unwrap();
                sh.merging = false;
                sh.confirming = false;
                sh.merge_error = err;
            });
        }
        Action::Comment => {
            {
                let mut sh = state.shared.lock().unwrap();
                sh.posting = true;
                sh.comment_error = None;
            }
            let shared = state.shared.clone();
            store.add_conversation_comment(&state.comment, move |_, err| {
                let mut sh = shared.lock().unwrap();
                sh.posting = false;
                sh.clear_comment = err.is_none();
                sh.comment_error = err;
            });
        }
        Action::SetClosed(closed) => {
            {
                let mut sh = state.shared.lock().unwrap();
                sh.closing = true;
                sh.comment_error = None;
            }
            let shared = state.shared.clone();
            let finish = move |s: &mut PRStore| {
                let shared2 = shared.clone();
                s.set_closed(closed, move |_, err| {
                    let mut sh = shared2.lock().unwrap();
                    sh.closing = false;
                    sh.comment_error = err;
                });
            };
            // Like GitHub, a typed comment is posted before closing.
            if closed && !state.comment.trim().is_empty() {
                let shared = state.shared.clone();
                store.add_conversation_comment(&state.comment, move |s, err| {
                    match err {
                        Some(e) => {
                            let mut sh = shared.lock().unwrap();
                            sh.closing = false;
                            sh.comment_error = Some(e);
                        }
                        None => {
                            shared.lock().unwrap().clear_comment = true;
                            finish(s);
                        }
                    }
                });
            } else {
                finish(store);
            }
        }
    }
}

fn anchor_for(store: &PRStore, id: &str) -> Option<review_buddy_core::models::CodeAnchor> {
    if let Some(t) = store.threads.get(id) {
        let end = t.anchor_line();
        return Some(review_buddy_core::models::CodeAnchor::new(t.path.clone(), t.side, t.start_line.unwrap_or(end), end));
    }
    store.findings.get(id).map(|f| f.anchor())
}

// MARK: - Header

fn header(ui: &mut Ui, store: &PRStore, state: &mut OverviewState, actions: &mut Vec<Action>) {
    let commits = store.detail.as_ref().and_then(|d| d.conversation.as_ref()).map_or(0, |c| c.commits.len())
        + store.local_commits.len();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if gh_button(ui, "← Files", BtnKind::Default, true).on_hover_text("Back to the diff").clicked() {
            actions.push(Action::Back);
        }
        segmented(ui, &mut state.tab, &[(Tab::Conversation, "Conversation".into()), (Tab::Commits, format!("Commits ({commits})"))]);
        if let Some(d) = &store.detail {
            ui.add_space(4.0);
            let p = pal(ui);
            state_badge(ui, d, &p);
            ui.add(egui::Label::new(RichText::new(&d.title).size(15.0).strong().color(p.text)).truncate());
        }
    });
}

/// A macOS-like segmented picker.
fn segmented(ui: &mut Ui, value: &mut Tab, options: &[(Tab, String)]) {
    let p = pal(ui);
    egui::Frame::new()
        .fill(p.subtle)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (tab, label) in options {
                let selected = *value == *tab;
                let btn = egui::Button::new(RichText::new(label).size(13.0).color(p.text))
                    .fill(if selected { p.canvas } else { Color32::TRANSPARENT })
                    .stroke(Stroke::new(1.0, if selected { p.border } else { Color32::TRANSPARENT }))
                    .corner_radius(CornerRadius::same(5))
                    .min_size(Vec2::new(0.0, 20.0));
                if ui.add(btn).clicked() {
                    *value = *tab;
                }
            }
        });
}

fn state_badge(ui: &mut Ui, d: &PRDetail, p: &Palette) {
    let (text, color) = match d.state.as_str() {
        "MERGED" => ("Merged", p.ai_accent),
        "CLOSED" => ("Closed", p.del_marker),
        _ if d.is_draft => ("Draft", p.muted_text),
        _ => ("Open", p.success),
    };
    pill(ui, text, color, true);
}

/// "New commits were pushed" with a Refresh button.
fn outdated_bar(ui: &mut Ui, store: &PRStore, p: &Palette, actions: &mut Vec<Action>) {
    if !store.is_outdated() {
        return;
    }
    egui::Frame::new()
        .fill(p.accent.gamma_multiply(0.1))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                super::icons::icon(ui, super::icons::Icon::Refresh, 13.0, p.accent);
                ui.label(RichText::new("This pull request has new commits on GitHub.").size(13.0).color(p.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if gh_button(ui, "Refresh", BtnKind::Primary, true)
                        .on_hover_text("Reload the diff and update the local checkout (⌘R)")
                        .clicked()
                    {
                        actions.push(Action::Refresh);
                    }
                });
            });
        });
}

// MARK: - AI pre-review

fn pre_review_card(ui: &mut Ui, store: &PRStore, p: &Palette, actions: &mut Vec<Action>) {
    let open = store.open_findings();
    let running = store.pre_review_state.is_running();
    egui::Frame::new()
        .fill(p.ai_bg)
        .stroke(Stroke::new(1.0, p.ai_accent.gamma_multiply(0.3)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(RichText::new("✨").color(p.ai_accent));
                ui.label(RichText::new("AI pre-review").size(14.0).strong().color(p.ai_accent));
                match &store.pre_review_state {
                    TaskState::Running(m) => {
                        spinner(ui);
                        ui.label(RichText::new(m).size(12.0).color(p.muted_text));
                    }
                    _ if !open.is_empty() => {
                        ui.label(RichText::new(format!("{} open", open.len())).size(12.0).color(p.muted_text));
                    }
                    _ => {}
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if running {
                        if gh_button(ui, "◼ Stop", BtnKind::Default, true).clicked() {
                            actions.push(Action::CancelPreReview);
                        }
                    } else {
                        let title = if store.findings.is_empty() { "✨ Run pre-review" } else { "✨ Run again" };
                        if gh_button(ui, title, BtnKind::Ai, true)
                            .on_hover_text("Ask Claude to review the whole PR and leave private suggested comments")
                            .clicked()
                        {
                            actions.push(Action::StartPreReview);
                        }
                    }
                });
            });
            if let TaskState::Failed(e) = &store.pre_review_state {
                ui.label(RichText::new(e).size(13.0).color(p.del_marker));
            }
            if !open.is_empty() {
                ui.spacing_mut().item_spacing.y = 4.0;
                for f in &open {
                    let resp = ui
                        .horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            let sev = match f.severity {
                                Severity::High => "High",
                                Severity::Medium => "Medium",
                                Severity::Low => "Low",
                                Severity::Nit => "Nit",
                            };
                            pill(ui, sev, if f.severity == Severity::High { p.del_marker } else { p.muted_text }, false);
                            let file = f.path.rsplit('/').next().unwrap_or(&f.path);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format!("{file}:{}", f.start_line))
                                        .font(FontId::monospace(12.0))
                                        .color(p.accent),
                                );
                                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                    ui.add(
                                        egui::Label::new(RichText::new(&f.title).size(13.0).color(p.text))
                                            .truncate()
                                            .selectable(false),
                                    );
                                });
                            });
                        })
                        .response;
                    let resp = resp.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.clicked() {
                        actions.push(Action::Scroll(f.id.clone()));
                    }
                }
            } else if !running && !store.findings.is_empty() {
                ui.label(RichText::new("All findings handled.").size(13.0).color(p.muted_text));
            }
        });
}

// MARK: - Timeline

enum Item<'a> {
    Comment(&'a ConversationComment),
    Review(&'a ConversationReview),
    Thread(&'a ReviewThread),
}

impl Item<'_> {
    fn date(&self) -> Option<Date> {
        match self {
            Item::Comment(c) => Some(c.created_at),
            Item::Review(r) => Some(r.submitted_at),
            Item::Thread(t) => t.comments.first().map(|c| c.created_at),
        }
    }
    fn id(&self) -> &str {
        match self {
            Item::Comment(c) => &c.id,
            Item::Review(r) => &r.id,
            Item::Thread(t) => &t.id,
        }
    }
}

fn timeline(ui: &mut Ui, store: &PRStore, p: &Palette, actions: &mut Vec<Action>) {
    let conv = store.detail.as_ref().and_then(|d| d.conversation.as_ref());
    let mut items: Vec<Item> = vec![];
    if let Some(c) = conv {
        items.extend(c.comments.iter().map(Item::Comment));
        items.extend(c.reviews.iter().map(Item::Review));
    }
    items.extend(store.threads.values().filter(|t| !t.comments.is_empty()).map(Item::Thread));
    items.sort_by(|a, b| a.date().cmp(&b.date()).then_with(|| a.id().cmp(b.id())));

    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 12.0;
        if let Some(d) = &store.detail {
            entry(ui, p, &d.author, "opened this pull request", |ui| {
                if d.body.trim().is_empty() {
                    ui.label(RichText::new("No description provided.").italics().color(p.muted_text));
                } else {
                    markdown(ui, &d.body, &[]);
                }
            });
        }
        for item in &items {
            match item {
                Item::Comment(c) => {
                    entry(ui, p, &c.author, &format!("commented {}", relative(&c.created_at)), |ui| markdown(ui, &c.body, &[]));
                }
                Item::Review(r) => review_entry(ui, p, r),
                Item::Thread(t) => thread_entry(ui, p, t, actions),
            }
        }
        if conv.is_none() {
            ui.label(
                RichText::new("Comments, reviews and commits load with the pull request. Refresh (⌘R) to fetch them.")
                    .size(13.0)
                    .color(p.muted_text),
            );
        }
    });
}

/// One timeline card: avatar, then a bordered box with an author/caption header and a body.
fn entry(ui: &mut Ui, p: &Palette, author: &str, caption: &str, content: impl FnOnce(&mut Ui)) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        avatar(ui, author, 28.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            card(ui, p.canvas, p.border, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::Frame::new()
                    .fill(p.subtle)
                    .corner_radius(CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 })
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.label(RichText::new(author).size(13.0).strong().color(p.text));
                            ui.label(RichText::new(caption).size(13.0).color(p.muted_text));
                        });
                    });
                hline(ui, p.border);
                egui::Frame::new().inner_margin(Margin::same(12)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 8.0;
                    content(ui);
                });
            });
        });
    });
}

fn hline(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, color));
}

fn review_entry(ui: &mut Ui, p: &Palette, r: &ConversationReview) {
    let verb = match r.state.as_str() {
        "APPROVED" => "approved these changes",
        "CHANGES_REQUESTED" => "requested changes",
        "DISMISSED" => "left a dismissed review",
        _ => "reviewed",
    };
    let caption = format!("{verb} {}", relative(&r.submitted_at));
    if r.body.trim().is_empty() {
        // GitHub shows bodiless reviews as a single event line.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let (rect, _) = ui.allocate_exact_size(Vec2::new(28.0, 18.0), Sense::hover());
            review_icon(ui, rect.center(), &r.state, p);
            avatar(ui, &r.author, 18.0);
            ui.label(RichText::new(&r.author).size(13.0).strong().color(p.text));
            ui.label(RichText::new(caption).size(13.0).color(p.muted_text));
        });
    } else {
        entry(ui, p, &r.author, &caption, |ui| {
            if matches!(r.state.as_str(), "APPROVED" | "CHANGES_REQUESTED") {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                    review_icon(ui, rect.center(), &r.state, p);
                    let (t, c) = if r.state == "APPROVED" { ("Approved", p.success) } else { ("Changes requested", p.del_marker) };
                    pill(ui, t, c, false);
                });
            }
            markdown(ui, &r.body, &[]);
        });
    }
}

fn review_icon(ui: &Ui, c: egui::Pos2, state: &str, p: &Palette) {
    match state {
        "APPROVED" => icon(ui, c, Icon::CheckCircle, p.success, 16.0),
        "CHANGES_REQUESTED" => icon(ui, c, Icon::BangCircle, p.del_marker, 16.0),
        _ => icon(ui, c, Icon::Ring, p.muted_text, 16.0),
    }
}

/// An inline review thread: location, first comment, reply count. Click to open it in the diff.
fn thread_entry(ui: &mut Ui, p: &Palette, t: &ReviewThread, actions: &mut Vec<Action>) {
    let first = &t.comments[0];
    entry(ui, p, &first.author, &format!("commented on code {}", relative(&first.created_at)), |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let link = ui
                .add(
                    egui::Label::new(
                        RichText::new(format!("{}:{}", t.path, t.anchor_line())).font(FontId::monospace(13.0)).color(p.accent),
                    )
                    .sense(Sense::click())
                    .selectable(false),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Show in the diff");
            if link.clicked() {
                actions.push(Action::Scroll(t.id.clone()));
            }
            if t.is_resolved {
                pill(ui, "Resolved", p.muted_text, false);
            }
            if t.is_outdated {
                pill(ui, "Outdated", p.attention, false);
            }
            if first.is_pending {
                pill(ui, "Pending", p.attention, false);
            }
        });
        markdown(ui, &first.body, &[]);
        if t.comments.len() > 1 {
            let n = t.comments.len() - 1;
            let text = if n == 1 { "1 reply".to_string() } else { format!("{n} replies") };
            ui.label(RichText::new(text).size(13.0).color(p.muted_text));
        }
    });
}

// MARK: - Commits

fn commit_list(ui: &mut Ui, store: &PRStore, p: &Palette) {
    let commits = store.detail.as_ref().and_then(|d| d.conversation.as_ref()).map(|c| c.commits.as_slice()).unwrap_or(&[]);
    card(ui, p.canvas, p.border, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut first = true;
        let mut sep = |ui: &mut Ui| {
            if !std::mem::take(&mut first) {
                hline(ui, p.border);
            }
        };
        for c in commits {
            sep(ui);
            commit_row(ui, p, &c.oid, &c.headline, &c.author, Some(&c.date), c.checks, false);
        }
        for c in &store.local_commits {
            sep(ui);
            commit_row(ui, p, &c.sha, &c.subject, &c.author, c.date.as_ref(), CheckState::None, true);
        }
        if commits.is_empty() && store.local_commits.is_empty() {
            egui::Frame::new().inner_margin(Margin::same(12)).show(ui, |ui| {
                ui.label(RichText::new("No commits loaded.").size(13.0).color(p.muted_text));
            });
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn commit_row(ui: &mut Ui, p: &Palette, sha: &str, headline: &str, author: &str, date: Option<&Date>, checks: CheckState, local: bool) {
    egui::Frame::new().inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            avatar(ui, author, 18.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let short: String = sha.chars().take(7).collect();
                ui.add(egui::Label::new(RichText::new(short).font(FontId::monospace(12.0)).color(p.muted_text)))
                    .on_hover_text(sha);
                let (rect, resp) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                match checks {
                    CheckState::Success => {
                        icon(ui, rect.center(), Icon::Check, p.success, 14.0);
                        resp.on_hover_text("Checks passed");
                    }
                    CheckState::Failure | CheckState::Error => {
                        icon(ui, rect.center(), Icon::Cross, p.del_marker, 14.0);
                        resp.on_hover_text("Checks failing");
                    }
                    CheckState::Pending | CheckState::Expected => {
                        icon(ui, rect.center(), Icon::Dot, p.attention, 14.0);
                        resp.on_hover_text("Checks running");
                    }
                    CheckState::None => {}
                }
                if local {
                    pill(ui, "Local", p.attention, false);
                }
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(egui::Label::new(RichText::new(headline).size(14.0).color(p.text).strong()).truncate());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(RichText::new(author).size(12.0).strong().color(p.muted_text));
                        if let Some(d) = date {
                            ui.label(RichText::new(format!("committed {}", relative(d))).size(12.0).color(p.muted_text));
                        }
                    });
                });
            });
        });
    });
}

// MARK: - Merge box

fn merge_box(ui: &mut Ui, store: &PRStore, state: &mut OverviewState, p: &Palette, actions: &mut Vec<Action>) {
    let Some(d) = &store.detail else { return };
    let Some(st) = &d.status else { return };
    card(ui, p.canvas, p.border, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        review_row(ui, st, p);
        if !st.checks.is_empty() {
            hline(ui, p.border);
            checks_section(ui, &st.checks, &mut state.hide_checks, p);
        }
        hline(ui, p.border);
        merge_row(ui, st, d, p);
        if d.state == "OPEN" && d.viewer_can_push && st.merge_state_status != "DRAFT" {
            egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 0, bottom: 12 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                merge_button(ui, store, st, state, p, actions);
            });
        }
    });
}

fn status_row(ui: &mut Ui, p: &Palette, ic: Icon, color: Color32, title: &str, detail: Option<&str>) -> egui::Response {
    egui::Frame::new()
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (rect, _) = ui.allocate_exact_size(Vec2::new(20.0, 18.0), Sense::hover());
                icon(ui, rect.center(), ic, color, 18.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(title).size(14.0).strong().color(p.text));
                    if let Some(detail) = detail {
                        ui.label(RichText::new(detail).size(13.0).color(p.muted_text));
                    }
                });
            });
        })
        .response
}

fn review_row(ui: &mut Ui, st: &PRMergeStatus, p: &Palette) {
    match st.review_decision.as_deref() {
        Some("APPROVED") => status_row(ui, p, Icon::CheckCircle, p.success, "Changes approved", None),
        Some("CHANGES_REQUESTED") => status_row(
            ui,
            p,
            Icon::CrossCircle,
            p.del_marker,
            "Changes requested",
            Some("A reviewer asked for changes before merging."),
        ),
        Some("REVIEW_REQUIRED") => status_row(
            ui,
            p,
            Icon::BangCircle,
            p.attention,
            "Review required",
            Some("At least one approving review is required."),
        ),
        _ => status_row(ui, p, Icon::Ring, p.muted_text, "No review required", None),
    };
}

fn check_rank(c: &MergeCheck) -> u8 {
    if c.is_failure() {
        0
    } else if c.is_pending() {
        1
    } else {
        2
    }
}

fn checks_section(ui: &mut Ui, checks: &[MergeCheck], hide: &mut bool, p: &Palette) {
    let failed = checks.iter().filter(|c| c.is_failure()).count();
    let pending = checks.iter().filter(|c| c.is_pending()).count();
    let ok = checks.len() - failed - pending;
    let (ic, color, title) = if failed > 0 {
        (Icon::CrossCircle, p.del_marker, "Some checks were not successful")
    } else if pending > 0 {
        (Icon::DotCircle, p.attention, "Some checks haven't completed yet")
    } else {
        (Icon::CheckCircle, p.success, "All checks have passed")
    };
    let mut parts = vec![];
    if failed > 0 {
        parts.push(format!("{failed} failing"));
    }
    if pending > 0 {
        parts.push(format!("{pending} in progress"));
    }
    if ok > 0 {
        parts.push(format!("{ok} successful"));
    }
    let detail = format!("{}{}", parts.join(", "), if checks.len() == 1 { " check" } else { " checks" });
    let resp = status_row(ui, p, ic, color, title, Some(&detail));
    let chevron = if *hide { "⏷" } else { "⏶" };
    ui.painter().text(
        egui::pos2(resp.rect.right() - 16.0, resp.rect.center().y),
        egui::Align2::CENTER_CENTER,
        chevron,
        FontId::proportional(13.0),
        p.muted_text,
    );
    let resp = resp.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        *hide = !*hide;
    }
    if *hide {
        return;
    }
    let mut sorted: Vec<&MergeCheck> = checks.iter().collect();
    sorted.sort_by(|a, b| (check_rank(a), &a.name).cmp(&(check_rank(b), &b.name)));
    egui::Frame::new()
        .fill(p.subtle.gamma_multiply(if p.dark { 1.0 } else { 0.5 }))
        .inner_margin(Margin { left: 0, right: 0, top: 0, bottom: 8 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            for c in sorted {
                egui::Frame::new().inner_margin(Margin::symmetric(12, 5)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(20.0, 14.0), Sense::hover());
                        if c.is_success() {
                            icon(ui, rect.center(), Icon::Check, p.success, 14.0);
                        } else if c.is_pending() {
                            icon(ui, rect.center(), Icon::Dot, p.attention, 14.0);
                        } else {
                            icon(ui, rect.center(), Icon::Cross, p.del_marker, 14.0);
                        }
                        ui.label(RichText::new(&c.name).size(13.0).strong().color(p.text));
                        if let Some(app) = &c.app {
                            ui.label(RichText::new(app).size(13.0).color(p.muted_text));
                        }
                        if let Some(url) = &c.url {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let r = ui
                                    .add(
                                        egui::Label::new(RichText::new("Details").size(13.0).color(p.accent))
                                            .sense(Sense::click())
                                            .selectable(false),
                                    )
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .on_hover_text(url);
                                if r.clicked() {
                                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                                }
                            });
                        }
                    });
                });
            }
        });
}

fn merge_row(ui: &mut Ui, st: &PRMergeStatus, d: &PRDetail, p: &Palette) {
    if d.state == "MERGED" {
        status_row(ui, p, Icon::CheckCircle, p.ai_accent, "Pull request merged", None);
    } else if d.state == "CLOSED" {
        status_row(ui, p, Icon::CrossCircle, p.del_marker, "Pull request closed", None);
    } else if st.mergeable == "CONFLICTING" || st.merge_state_status == "DIRTY" {
        status_row(ui, p, Icon::BangCircle, p.del_marker, "This branch has conflicts that must be resolved", None);
    } else {
        match st.merge_state_status.as_str() {
            "CLEAN" | "HAS_HOOKS" => {
                status_row(ui, p, Icon::CheckCircle, p.success, "Ready to merge", Some("No conflicts with the base branch."))
            }
            "UNSTABLE" => status_row(ui, p, Icon::BangCircle, p.attention, "Can merge, but some checks are failing", None),
            "BEHIND" => status_row(ui, p, Icon::BangCircle, p.attention, "This branch is out-of-date with the base branch", None),
            "DRAFT" => status_row(ui, p, Icon::Ring, p.muted_text, "This pull request is still a draft", None),
            "BLOCKED" => status_row(
                ui,
                p,
                Icon::CrossCircle,
                p.del_marker,
                "Merging is blocked",
                Some("Required reviews or checks haven't passed yet."),
            ),
            _ => status_row(
                ui,
                p,
                Icon::Ring,
                p.muted_text,
                "Checking mergeability…",
                Some("GitHub is still computing this. Refresh (⌘R) to update."),
            ),
        };
    }
}

/// GitHub's merge split button: pick a method, press, then confirm (merging can't be undone).
fn merge_button(ui: &mut Ui, store: &PRStore, st: &PRMergeStatus, state: &mut OverviewState, p: &Palette, actions: &mut Vec<Action>) {
    let methods: Vec<MergeMethod> = match &st.allowed_merge_methods {
        Some(m) if !m.is_empty() => m.clone(),
        _ => MergeMethod::ALL.to_vec(),
    };
    let method = state
        .method
        .filter(|m| methods.contains(m))
        .or(st.default_merge_method.filter(|m| methods.contains(m)))
        .unwrap_or(methods[0]);
    let can_merge = st.can_merge();
    let can_bypass = st.can_bypass();
    if !can_bypass {
        state.bypass = false;
    }
    let enabled = can_merge || (can_bypass && state.bypass);
    let tint = if can_merge { p.success } else { p.del_marker };
    let (confirming, merging, error) = {
        let sh = state.shared.lock().unwrap();
        (sh.confirming, sh.merging, sh.merge_error.clone())
    };
    ui.spacing_mut().item_spacing.y = 8.0;
    if confirming {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let title = match method {
                MergeMethod::Merge => "Confirm merge",
                MergeMethod::Squash => "Confirm squash and merge",
                MergeMethod::Rebase => "Confirm rebase and merge",
            };
            if filled_button_busy(ui, title, tint, true, CornerRadius::same(6), merging).clicked() {
                actions.push(Action::Merge(method, state.bypass && !can_merge));
            }
            if gh_button(ui, "Cancel", BtnKind::Default, !merging).clicked() {
                state.shared.lock().unwrap().confirming = false;
            }
            let base = store.detail.as_ref().map_or("base", |d| d.base_ref_name.as_str());
            ui.label(RichText::new(format!("Merges #{} into {base} on GitHub.", store.pr_ref.number)).size(13.0).color(p.muted_text));
        });
    } else {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let split = methods.len() > 1;
            let radius = if split { CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 } } else { CornerRadius::same(6) };
            if filled_button(ui, method.button_title(), tint, enabled, radius).clicked() {
                let mut sh = state.shared.lock().unwrap();
                sh.merge_error = None;
                sh.confirming = true;
            }
            // One allowed method (repo setting): a plain button, as on GitHub.
            if split {
                ui.add_space(1.0);
                let r = filled_button(ui, "▾", tint, enabled, CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 }).on_hover_text("Merge method");
                egui::Popup::menu(&r).show(|ui| {
                    for m in &methods {
                        let label = format!("{} {}", if *m == method { "✔" } else { "   " }, m.title());
                        if ui.button(label).clicked() {
                            state.method = Some(*m);
                            ui.close();
                        }
                    }
                });
            }
            ui.add_space(8.0);
            if !can_merge && !can_bypass {
                let why = if st.mergeable == "CONFLICTING" { "Resolve conflicts first." } else { "Not mergeable yet." };
                ui.label(RichText::new(why).size(13.0).color(p.muted_text));
            }
        });
        if can_bypass {
            ui.checkbox(
                &mut state.bypass,
                RichText::new("Merge without waiting for requirements to be met (bypass rules)").size(13.0),
            );
        }
    }
    if let Some(e) = error {
        ui.label(RichText::new(format!("{e} Refresh (⌘R) and try again.")).size(13.0).color(p.del_marker));
    }
}

fn filled_button(ui: &mut Ui, text: &str, color: Color32, enabled: bool, radius: CornerRadius) -> egui::Response {
    filled_button_busy(ui, text, color, enabled, radius, false)
}

/// A tinted button in `color` (merge green, blocked red).
fn filled_button_busy(ui: &mut Ui, text: &str, color: Color32, enabled: bool, radius: CornerRadius, busy: bool) -> egui::Response {
    let p = pal(ui);
    let (fill, stroke, fg) = p.tinted(color);
    let look = super::icons::Look { fill, fg, stroke, native: false, frame: true, radius };
    super::icons::button_full(ui, text, &[], egui::FontId::proportional(13.0), look, 28.0, enabled, busy)
}

// MARK: - Comment box

fn comment_composer(ui: &mut Ui, store: &PRStore, state: &mut OverviewState, p: &Palette, actions: &mut Vec<Action>) {
    let Some(d) = &store.detail else { return };
    // GitHub lets the author and anyone with write access close or reopen.
    let can_close = d.state != "MERGED" && (d.viewer_can_push || d.author == d.viewer_login);
    let (posting, closing, error) = {
        let sh = state.shared.lock().unwrap();
        (sh.posting, sh.closing, sh.comment_error.clone())
    };
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.label(RichText::new("Add a comment").size(14.0).strong().color(p.text));
        let edit = egui::TextEdit::multiline(&mut state.comment)
            .hint_text("Leave a comment (Markdown)")
            .font(FontId::proportional(14.0))
            .desired_width(f32::INFINITY)
            .desired_rows(5)
            .background_color(p.canvas)
            .frame(
                egui::Frame::new()
                    .fill(p.canvas)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::same(6)),
            );
        let resp = ui.add(edit);
        let has_text = !state.comment.trim().is_empty();
        let submit_key = resp.has_focus()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
        if let Some(e) = error {
            ui.label(RichText::new(e).size(13.0).color(p.del_marker));
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Posted to GitHub right away, outside your review.").size(12.0).color(p.muted_text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if super::common::gh_button_busy(ui, "Comment", &[], BtnKind::Primary, has_text && !closing, posting).on_hover_text("⌘↩").clicked()
                    || (submit_key && has_text && !closing)
                {
                    actions.push(Action::Comment);
                }
                if can_close {
                    let closed = d.state == "CLOSED";
                    let (title, kind) = if closed {
                        ("Reopen pull request", BtnKind::Default)
                    } else if state.comment.is_empty() {
                        ("Close pull request", BtnKind::Danger)
                    } else {
                        ("Close with comment", BtnKind::Danger)
                    };
                    let all = ["Reopen pull request", "Close pull request", "Close with comment"];
                    if super::common::gh_button_busy(ui, title, &all, kind, !posting, closing).clicked() {
                        actions.push(Action::SetClosed(!closed));
                    }
                }
            });
        });
    });
}

// MARK: - Icons

#[derive(Clone, Copy)]
enum Icon {
    Check,
    Cross,
    Dot,
    CheckCircle,
    CrossCircle,
    BangCircle,
    DotCircle,
    Ring,
}

/// Small vector status icons (the bundled fonts lack SF Symbols-like glyphs).
fn icon(ui: &Ui, c: egui::Pos2, kind: Icon, color: Color32, size: f32) {
    let painter = ui.painter();
    let r = size / 2.0;
    let fg = |w: f32| Stroke::new(w, color);
    let white = Stroke::new((size / 9.0).max(1.5), Color32::WHITE);
    let check = |s: Stroke, k: f32| {
        painter.line_segment([c + Vec2::new(-0.45, 0.0) * k, c + Vec2::new(-0.12, 0.33) * k], s);
        painter.line_segment([c + Vec2::new(-0.12, 0.33) * k, c + Vec2::new(0.45, -0.3) * k], s);
    };
    let cross = |s: Stroke, k: f32| {
        painter.line_segment([c + Vec2::new(-0.35, -0.35) * k, c + Vec2::new(0.35, 0.35) * k], s);
        painter.line_segment([c + Vec2::new(0.35, -0.35) * k, c + Vec2::new(-0.35, 0.35) * k], s);
    };
    match kind {
        Icon::Check => check(fg(2.0), r * 1.6),
        Icon::Cross => cross(fg(2.0), r * 1.4),
        Icon::Dot => {
            painter.circle_filled(c, r * 0.4, color);
        }
        Icon::CheckCircle => {
            painter.circle_filled(c, r, color);
            check(white, r * 1.1);
        }
        Icon::CrossCircle => {
            painter.circle_filled(c, r, color);
            cross(white, r * 1.0);
        }
        Icon::BangCircle => {
            painter.circle_filled(c, r, color);
            painter.line_segment([c + Vec2::new(0.0, -0.5 * r), c + Vec2::new(0.0, 0.15 * r)], white);
            painter.circle_filled(c + Vec2::new(0.0, 0.48 * r), white.width * 0.6, Color32::WHITE);
        }
        Icon::DotCircle => {
            painter.circle_stroke(c, r - 1.0, fg(2.0));
            painter.circle_filled(c, r * 0.35, color);
        }
        Icon::Ring => {
            painter.circle_stroke(c, r - 1.0, fg(1.5));
        }
    }
}
