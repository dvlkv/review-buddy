//! Layout stability sweep: nothing may change size or move when the pointer hovers or
//! presses it, or when the window simply renders again.
//!
//! The whole app runs headless on the synthetic PR. For each screen (diff, split, composer,
//! threads, Discuss, Summary, merge box, settings tabs, sheets, narrow window) and each
//! theme:
//! 1. rerender: idle frames must be identical, and take one layout pass;
//! 2. first frame: content that appears after a state change must not shift a frame later;
//! 3. hover: the pointer on each interactive widget (and on a grid over the window) must
//!    not move or resize anything;
//! 4. press: holding the button down must not either;
//! 5. click: a widget must keep its own size after it's clicked (toggles, tabs, buttons
//!    that start work), unless listed in `CLICK_MAY_RESIZE`.
//!
//! "Anything" is every AccessKit node (widgets and Ui containers, with bounds) and every
//! painted text run (position and size), so custom-painted canvases are covered too.

use std::collections::HashMap;

use egui::accesskit::{Action, NodeId, Role};
use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, Vec2};
use review_buddy_core::support::synthetic::Shape as DiffShape;

use super::script::ScriptRunner;
use super::ReviewBuddyApp;

const SCREEN: Vec2 = Vec2::new(1600.0, 1000.0);
const DT: f64 = 1.0 / 60.0;

/// Quantized rect (1/100 px) so comparisons are exact without float noise.
type Q = [i32; 4];

fn q(r: Rect) -> Q {
    // Clamped: some containers report unbounded rects.
    let f = |v: f32| (v * 100.0).round().clamp(-1.0e9, 1.0e9) as i32;
    [f(r.min.x), f(r.min.y), f(r.max.x), f(r.max.y)]
}

fn size(r: &Q) -> (f32, f32) {
    ((r[2] as i64 - r[0] as i64) as f32 / 100.0, (r[3] as i64 - r[1] as i64) as f32 / 100.0)
}

#[derive(Clone, Debug)]
struct NodeInfo {
    role: Role,
    label: String,
    rect: Q,
    clickable: bool,
}

#[derive(Clone, Default)]
struct Snap {
    nodes: HashMap<NodeId, NodeInfo>,
    /// Painted text runs: text → positions/sizes.
    texts: HashMap<String, Vec<Q>>,
}

impl Snap {
    fn from_output(out: &egui::FullOutput) -> Snap {
        let mut s = Snap::default();
        if let Some(update) = &out.platform_output.accesskit_update {
            for (id, node) in &update.nodes {
                let Some(b) = node.bounds() else { continue };
                let rect = Rect::from_min_max(Pos2::new(b.x0 as f32, b.y0 as f32), Pos2::new(b.x1 as f32, b.y1 as f32));
                let label = node.label().map(str::to_string).or_else(|| node.value().map(str::to_string)).unwrap_or_default();
                let clickable = node.supports_action(Action::Click) || matches!(node.role(), Role::TextInput | Role::MultilineTextInput);
                s.nodes.insert(*id, NodeInfo { role: node.role(), label, rect: q(rect), clickable });
            }
        }
        fn walk(shape: &Shape, clip: Rect, s: &mut Snap) {
            match shape {
                Shape::Vec(v) => v.iter().for_each(|x| walk(x, clip, s)),
                Shape::Text(t) => {
                    let r = Rect::from_min_size(t.pos, t.galley.size());
                    if r.intersects(clip) && !t.galley.text().trim().is_empty() {
                        s.texts.entry(t.galley.text().to_string()).or_default().push(q(r));
                    }
                }
                _ => {}
            }
        }
        for c in &out.shapes {
            walk(&c.shape, c.clip_rect, &mut s);
        }
        for v in s.texts.values_mut() {
            v.sort();
        }
        s
    }

    fn texts_only(self) -> Snap {
        Snap { nodes: HashMap::new(), texts: self.texts }
    }

    /// Differences that mean something moved or changed size between `self` (before) and
    /// `after`. Widgets and texts that appear or disappear (hover backgrounds, tooltips)
    /// are fine; ones present in both must be identical.
    fn diff(&self, after: &Snap) -> Vec<String> {
        let mut out = vec![];
        for (id, a) in &self.nodes {
            if let Some(b) = after.nodes.get(id) {
                if a.rect != b.rect {
                    let (aw, ah) = size(&a.rect);
                    let (bw, bh) = size(&b.rect);
                    out.push(format!(
                        "node {:?} {:?} {:?}: {aw:.2}x{ah:.2} @({:.2},{:.2}) -> {bw:.2}x{bh:.2} @({:.2},{:.2})",
                        id,
                        a.role,
                        a.label,
                        a.rect[0] as f32 / 100.0,
                        a.rect[1] as f32 / 100.0,
                        b.rect[0] as f32 / 100.0,
                        b.rect[1] as f32 / 100.0
                    ));
                }
            }
        }
        for (text, a) in &self.texts {
            let Some(b) = after.texts.get(text) else { continue };
            // Same count: every run must be exactly where it was. Different count (a
            // tooltip repeating a label): every earlier run must still exist.
            let moved = if a.len() == b.len() { a != b } else { a.iter().any(|r| !b.contains(r)) };
            if moved {
                out.push(format!("text {:?}: {:?} -> {:?}", text, a.iter().map(fmt_q).collect::<Vec<_>>(), b.iter().map(fmt_q).collect::<Vec<_>>()));
            }
        }
        out.sort();
        out
    }
}

fn fmt_q(r: &Q) -> String {
    let (w, h) = size(r);
    format!("{w:.2}x{h:.2}@({:.2},{:.2})", r[0] as f32 / 100.0, r[1] as f32 / 100.0)
}

#[derive(Clone, Copy)]
enum Step {
    /// A `--ui-script` command, e.g. "selectfirst" or "demodiscuss".
    Cmd(&'static str),
    /// Click the first widget with this label.
    Click(&'static str),
    Do(fn(&mut ReviewBuddyApp)),
    /// Window size for the rest of the scenario.
    Screen(f32, f32),
}

impl Step {
    fn describe(&self) -> String {
        match self {
            Step::Cmd(c) => format!("`{c}`"),
            Step::Click(l) => format!("click {l:?}"),
            Step::Do(_) => "state change".into(),
            Step::Screen(w, h) => format!("resize to {w}x{h}"),
        }
    }
}

struct Harness {
    ctx: egui::Context,
    app: ReviewBuddyApp,
    time: f64,
    pointer: Option<Pos2>,
    screen: Vec2,
    /// What moved between the first and second frame after each step: content that
    /// shows up at one size and corrects itself a frame later (a visible flicker).
    first_frame_jumps: Vec<String>,
}

impl Harness {
    fn new(steps: &[Step], dark: bool) -> Harness {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
        // Tooltips never show during the sweep (they're overlays; they can't shift layout).
        ctx.all_styles_mut(|s| s.interaction.tooltip_delay = 1.0e6);
        // `SWEEP_PPP=2` runs the sweep at Retina scale.
        if let Some(ppp) = std::env::var("SWEEP_PPP").ok().and_then(|v| v.parse().ok()) {
            ctx.set_pixels_per_point(ppp);
        }
        let app = ReviewBuddyApp::headless(&ctx, DiffShape::SMALL);
        let mut h = Harness { ctx, app, time: 0.0, pointer: None, screen: SCREEN, first_frame_jumps: vec![] };
        h.settle();
        for step in steps {
            match *step {
                Step::Cmd(c) => {
                    let parts: Vec<String> = c.split(':').map(str::to_string).collect();
                    let mut runner = ScriptRunner::script("", std::env::temp_dir());
                    let mut next = 0.0;
                    let ctx = h.ctx.clone();
                    h.app.run_command(&ctx, &mut runner, &parts, &mut next);
                }
                Step::Click(label) => {
                    let snap = h.frame(vec![]);
                    let id = snap
                        .nodes
                        .iter()
                        .filter(|(_, n)| n.label == label && n.clickable)
                        // Deterministic: real controls first, then the lowest on screen (a
                        // sheet's tab over the title bar's button of the same name).
                        .min_by_key(|(_, n)| (matches!(n.role, Role::Label | Role::Unknown), -n.rect[1], n.rect[0]))
                        .map(|(id, _)| *id)
                        .unwrap_or_else(|| panic!("no widget labeled {label:?}"));
                    h.frame(vec![action(Action::Click, id)]);
                }
                Step::Do(f) => f(&mut h.app),
                Step::Screen(w, hh) => h.screen = Vec2::new(w, hh),
            }
            // Painted text only: windows and popups lay out in an invisible first pass
            // whose widget rects aren't shown.
            let p0 = h.ctx.cumulative_pass_nr();
            let first = h.frame(vec![]).texts_only();
            let p1 = h.ctx.cumulative_pass_nr();
            let second = h.frame(vec![]).texts_only();
            if std::env::var("SWEEP_DEBUG").is_ok() {
                eprintln!("after {}: first frame {} passes, second {}", step.describe(), p1 - p0, h.ctx.cumulative_pass_nr() - p1);
            }
            for d in first.diff(&second) {
                h.first_frame_jumps.push(format!("after {}: {d}", step.describe()));
            }
            h.settle();
        }
        h
    }

    fn frame(&mut self, events: Vec<Event>) -> Snap {
        self.time += DT;
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.screen)),
            time: Some(self.time),
            predicted_dt: DT as f32,
            focused: true,
            modifiers: Modifiers::NONE,
            events,
            ..Default::default()
        };
        if let Some(p) = self.pointer {
            if !input.events.iter().any(|e| matches!(e, Event::PointerMoved(_))) {
                input.events.insert(0, Event::PointerMoved(p));
            }
        }
        let app = &mut self.app;
        let out = self.ctx.run_ui(input, |ui| app.draw(ui));
        Snap::from_output(&out)
    }

    /// Runs frames until two in a row are identical; panics if layout never settles.
    fn settle(&mut self) -> Snap {
        let mut prev = self.frame(vec![]);
        for _ in 0..40 {
            let s = self.frame(vec![]);
            if prev.diff(&s).is_empty() && s.nodes.len() == prev.nodes.len() {
                return s;
            }
            prev = s;
        }
        panic!("layout never settled:\n{}", prev.diff(&self.frame(vec![])).join("\n"));
    }

    fn move_to(&mut self, p: Option<Pos2>) -> Snap {
        self.pointer = p;
        let ev = match p {
            Some(p) => vec![Event::PointerMoved(p)],
            None => vec![Event::PointerGone],
        };
        self.frame(ev);
        self.frame(vec![])
    }

    fn button(&mut self, p: Pos2, pressed: bool) -> Snap {
        self.frame(vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }]);
        self.frame(vec![])
    }

    fn scroll_to(&mut self, id: NodeId) -> Snap {
        self.frame(vec![action(Action::ScrollIntoView, id)]);
        self.settle()
    }
}

fn action(action: Action, target: NodeId) -> Event {
    Event::AccessKitActionRequest(egui::accesskit::ActionRequest { action, target_tree: egui::accesskit::TreeId::ROOT, target_node: target, data: None })
}

fn center(r: &Q) -> Pos2 {
    Pos2::new((r[0] as i64 + r[2] as i64) as f32 / 200.0, (r[1] as i64 + r[3] as i64) as f32 / 200.0)
}

fn on_screen(r: &Q, screen: Vec2) -> bool {
    let c = center(r);
    let (w, h) = size(r);
    w > 0.5 && h > 0.5 && c.x > 1.0 && c.y > 1.0 && c.x < screen.x - 1.0 && c.y < screen.y - 1.0
}

/// Widgets whose own size may change when clicked: (scenario prefix, label). Each turns
/// into something else by design.
const CLICK_MAY_RESIZE: &[(&str, &str)] = &[
    // The "Reply…" placeholder turns into the reply editor.
    ("", "Reply…"),
    // The merge button turns into the "Confirm …" row (as on GitHub), and Cancel back.
    ("merge-", "Squash and merge"),
    ("merge-", "Merge pull request"),
    ("merge-", "Rebase and merge"),
    ("merge-confirm", "Cancel"),
];

/// Clicks that legitimately resize other widgets: (scenario prefix, clicked label).
const SIBLINGS_MAY_RESIZE: &[(&str, &str)] = &[
    // Panel toggles change the width everything else gets.
    ("", "SidebarLeft"),
    ("", "Sidebar"),
    ("", "Sparkle"),
    // Closing or reopening changes the state badge next to the title.
    ("", "Close pull request"),
    ("", "Reopen pull request"),
];

fn may_resize(name: &str, label: &str) -> bool {
    CLICK_MAY_RESIZE.iter().any(|(prefix, l)| name.starts_with(prefix) && *l == label)
}

struct Report {
    problems: Vec<String>,
    probes: usize,
    /// Rerender and hover over widgets only (no grid, presses, clicks or scrolling).
    quick: bool,
}

fn sweep(name: &str, steps: &[Step], dark: bool, report: &mut Report) {
    let mut probed = std::collections::HashSet::new();
    let first = sweep_view(name, steps, None, dark, &mut probed, report);
    if report.quick {
        return;
    }
    // Widgets below the fold (long conversations, the merge box, settings rows): scroll
    // each unprobed one into view and sweep what's visible then.
    for _ in 0..40 {
        let next = first
            .nodes
            .iter()
            .filter(|(id, n)| n.clickable && n.role != Role::ScrollBar && !probed.contains(*id))
            .min_by_key(|(_, n)| (n.rect[1], n.rect[0]))
            .map(|(id, _)| *id);
        let Some(id) = next else { break };
        probed.insert(id);
        sweep_view(name, steps, Some(id), dark, &mut probed, report);
    }
}

/// Probes everything on screen in one state; returns that state's baseline.
fn sweep_view(name: &str, steps: &[Step], scroll: Option<NodeId>, dark: bool, probed: &mut std::collections::HashSet<NodeId>, report: &mut Report) -> Snap {
    let theme = if dark { "dark" } else { "light" };
    let tag = match scroll {
        Some(id) => format!("{name}/{theme} scrolled to {id:?}"),
        None => format!("{name}/{theme}"),
    };
    let build = || {
        let mut h = Harness::new(steps, dark);
        if let Some(id) = scroll {
            h.scroll_to(id);
        }
        h
    };
    let mut h = build();
    if scroll.is_none() {
        for d in std::mem::take(&mut h.first_frame_jumps) {
            report.problems.push(format!("[{tag}] first frame {d}"));
        }
    }
    let base = h.move_to(None);
    // 1. Rerender: identical, and done in one pass (an extra layout pass every frame
    // would mean something keeps changing size and is being corrected each frame).
    for _ in 0..3 {
        let passes = h.ctx.cumulative_pass_nr();
        let again = h.frame(vec![]);
        for d in base.diff(&again) {
            report.problems.push(format!("[{tag}] rerender: {d}"));
        }
        let n = h.ctx.cumulative_pass_nr() - passes;
        if n != 1 {
            report.problems.push(format!("[{tag}] rerender took {n} layout passes"));
        }
    }
    let mut targets: Vec<(NodeId, NodeInfo)> =
        base.nodes.iter().filter(|(i, n)| n.clickable && n.role != Role::ScrollBar && on_screen(&n.rect, h.screen) && !probed.contains(*i)).map(|(i, n)| (*i, n.clone())).collect();
    targets.sort_by_key(|(_, n)| (n.rect[1], n.rect[0]));
    probed.extend(targets.iter().map(|(i, _)| *i));
    let mut points: Vec<(String, Option<NodeId>, Pos2)> = targets.iter().map(|(i, n)| (format!("{:?} {:?} {}", n.role, n.label, fmt_q(&n.rect)), Some(*i), center(&n.rect))).collect();
    // Plus a grid over the window, for custom-painted areas (diff rows, gutters). Pressing
    // a scroll bar scrolls; that's not a layout jump.
    let bars: Vec<Rect> = base
        .nodes
        .values()
        .filter(|n| n.role == Role::ScrollBar)
        .map(|n| Rect::from_min_max(Pos2::new(n.rect[0] as f32 / 100.0, n.rect[1] as f32 / 100.0), Pos2::new(n.rect[2] as f32 / 100.0, n.rect[3] as f32 / 100.0)).expand(4.0))
        .collect();
    if scroll.is_none() && !report.quick {
        let screen = h.screen;
        let mut y = 20.0;
        while y < screen.y {
            let mut x = 20.0;
            while x < screen.x {
                if !bars.iter().any(|b| b.contains(Pos2::new(x, y))) {
                    points.push((format!("grid ({x},{y})"), None, Pos2::new(x, y)));
                }
                x += 60.0;
            }
            y += 45.0;
        }
    }
    for (i, (what, id, p)) in points.into_iter().enumerate() {
        report.probes += 1;
        // 2. Hover.
        let hovered = h.move_to(Some(p));
        for d in base.diff(&hovered) {
            report.problems.push(format!("[{tag}] hover {what}: {d}"));
        }
        // Grid points: hover everywhere, press and click a sparse subset (clicks on the
        // diff change the selection, which means rebuilding the screen afterwards).
        if report.quick || (id.is_none() && i % 5 != 0) {
            continue;
        }
        // 3. Press (and hold).
        let pressed = h.button(p, true);
        for d in base.diff(&pressed) {
            report.problems.push(format!("[{tag}] press {what}: {d}"));
        }
        // 4. Release = click.
        h.button(p, false);
        h.move_to(None);
        let after = h.frame(vec![]);
        if let Some(id) = id {
            if let (Some(a), Some(b)) = (base.nodes.get(&id), after.nodes.get(&id)) {
                if size(&a.rect) != size(&b.rect) && !may_resize(name, &a.label) {
                    report.problems.push(format!("[{tag}] click {what}: own size {} -> {}", fmt_q(&a.rect), fmt_q(&b.rect)));
                }
            }
            // Other widgets may move when a click changes the screen, but a widget that's
            // still there (same id, role and label) must keep its size.
            let clicked = base.nodes.get(&id).map(|n| n.label.as_str()).unwrap_or("");
            let siblings_may_resize = SIBLINGS_MAY_RESIZE.iter().any(|(prefix, l)| name.starts_with(prefix) && *l == clicked);
            for (oid, a) in &base.nodes {
                if siblings_may_resize || *oid == id || a.label.is_empty() || matches!(a.role, Role::Unknown | Role::GenericContainer) {
                    continue;
                }
                if let Some(b) = after.nodes.get(oid) {
                    if b.role == a.role && b.label == a.label && size(&a.rect) != size(&b.rect) {
                        report.problems.push(format!("[{tag}] click {what}: {:?} {:?} resized {} -> {}", a.role, a.label, fmt_q(&a.rect), fmt_q(&b.rect)));
                    }
                }
            }
        }
        // A click that changed the screen: start this view over.
        if !base.diff(&after).is_empty() || after.nodes.len() != base.nodes.len() {
            h = build();
            let again = h.move_to(None);
            for d in base.diff(&again) {
                report.problems.push(format!("[{tag}] rebuild differs from baseline: {d}"));
            }
        }
    }
    base
}

const PREVIEW_TABLE: &str = "settext:Edge cases\n\n| Input | Expected | Notes |\n|---|---|---|\n| `[]` | `null` | empty list |\n| `[a]` | `a` | single item, the common case |\n\nAlso see `fetchUser`.";

const BYPASS: &str = "Merge without waiting for requirements to be met (bypass rules)";

fn scenarios() -> Vec<(&'static str, Vec<Step>)> {
    use Step::*;
    vec![
        ("diff", vec![]),
        ("split", vec![Cmd("split")]),
        ("selection", vec![Cmd("selectfirst")]),
        ("composer", vec![Cmd("selectfirst"), Cmd("comment")]),
        ("pending-thread", vec![Cmd("selectfirst"), Cmd("comment"), Cmd("typecomment:Looks off by one?")]),
        ("discuss", vec![Cmd("demodiscuss")]),
        ("discuss-table", vec![Cmd("demodiscuss:table")]),
        ("ai-summary", vec![Cmd("demosummary")]),
        ("outdated", vec![Cmd("outdated")]),
        ("overview", vec![Cmd("overview")]),
        ("overview-admin", vec![Cmd("overview"), Cmd("admin")]),
        ("merge-bypass", vec![Cmd("overview"), Cmd("admin"), Click(BYPASS)]),
        ("merge-confirm", vec![Cmd("overview"), Cmd("admin"), Click(BYPASS), Click("Squash and merge")]),
        ("commits", vec![Cmd("overview"), Click("Commits (3)")]),
        ("settings", vec![Cmd("settings")]),
        ("settings-repos", vec![Cmd("settings"), Click("Repositories")]),
        ("settings-review", vec![Cmd("settings"), Click("Review")]),
        ("palette", vec![Cmd("palette")]),
        ("submit", vec![Cmd("submit")]),
        ("open-url", vec![Do(|a| a.state.ui.show_open_url = true)]),
        ("push", vec![Do(|a| a.state.ui.show_push_sheet = true)]),
        ("no-pr", vec![Do(|a| a.state.current = None)]),
        ("no-sidebar", vec![Cmd("sidebar:off")]),
        ("narrow", vec![Screen(960.0, 640.0)]),
        ("narrow-discuss", vec![Screen(960.0, 640.0), Cmd("demodiscuss")]),
        ("narrow-overview", vec![Screen(960.0, 640.0), Cmd("overview")]),
        ("split-composer", vec![Cmd("split"), Cmd("selectfirst"), Cmd("comment")]),
        ("split-discuss", vec![Cmd("split"), Cmd("demodiscuss")]),
        ("discuss-inactive", vec![Cmd("demodiscuss"), Do(|a| a.state.current_pr_mut().unwrap().active_discuss_id = None)]),
        ("submit-pending", vec![Cmd("selectfirst"), Cmd("comment"), Cmd("typecomment:Off by one?"), Cmd("submit")]),
        ("summary-expanded", vec![Cmd("fakesummary:8"), Click("Show more")]),
        // Chained re-layouts in one frame: a panel settling and a table learning its columns;
        // a composer block resizing around a previewed table.
        ("narrow-discuss-table", vec![Screen(960.0, 640.0), Cmd("demodiscuss:table")]),
        ("composer-preview-table", vec![Cmd("selectfirst"), Cmd("comment"), Cmd(PREVIEW_TABLE), Click("Preview")]),
    ]
}

/// The full sweep. Slow in debug builds; run with
/// `cargo test --release -p review-buddy layout_sweep -- --ignored --nocapture`.
#[test]
#[ignore]
fn layout_sweep() {
    let only = std::env::var("SWEEP_ONLY").ok();
    let mut report = Report { problems: vec![], probes: 0, quick: false };
    for dark in [true, false] {
        for (name, steps) in scenarios() {
            if only.as_deref().is_some_and(|o| !o.split(',').any(|x| x == name)) {
                continue;
            }
            let t = std::time::Instant::now();
            let before = report.problems.len();
            sweep(name, &steps, dark, &mut report);
            eprintln!("{name}/{}: {} problems, {:.1}s", if dark { "dark" } else { "light" }, report.problems.len() - before, t.elapsed().as_secs_f64());
        }
    }
    let mut uniq = report.problems.clone();
    uniq.dedup();
    for p in &uniq {
        eprintln!("{p}");
    }
    assert!(report.problems.is_empty(), "{} layout jumps over {} probes", report.problems.len(), report.probes);
}

/// Regression (crash in the field): marking a file viewed collapses it, which rebuilds the
/// layout while the canvas is still painting rows from the old one.
#[test]
fn collapsing_the_last_file_while_painting_does_not_crash() {
    fn to_last_file(app: &mut ReviewBuddyApp) {
        let s = app.state.current_pr_mut().unwrap();
        let l = s.layout.as_ref().unwrap();
        s.pending_scroll = l.file_start_row.last().copied();
    }
    let mut h = Harness::new(&[Step::Do(to_last_file)], true);
    let snap = h.frame(vec![]);
    let last = h.app.state.current_pr().unwrap().files().len() - 1;
    // The last file's header sits at the top of the diff: the topmost painted "Viewed".
    let r = snap.texts.get("Viewed").and_then(|v| v.iter().filter(|r| r[1] > 40 * 100).min_by_key(|r| r[1])).copied().expect("a Viewed toggle on screen");
    let at = center(&r);
    h.move_to(Some(at));
    h.button(at, true);
    h.button(at, false);
    h.settle();
    assert!(h.app.state.current_pr().unwrap().options.collapsed.contains(&last), "the click should have collapsed the last file");
}

/// A quick subset that runs with the normal test suite.
#[test]
fn layout_is_stable_on_hover_and_rerender() {
    let mut report = Report { problems: vec![], probes: 0, quick: true };
    sweep("diff", &[], true, &mut report);
    sweep("discuss", &[Step::Cmd("demodiscuss")], false, &mut report);
    for p in report.problems.iter().take(50) {
        eprintln!("{p}");
    }
    assert!(report.problems.is_empty(), "{} layout jumps", report.problems.len());
}


