//! Root application state (no egui types here, so stores are testable headless).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use review_buddy_core::github::client::InboxSection;
use review_buddy_core::*;

use crate::pr_store::PRStore;
use crate::rt::Spawner;
use crate::services::Services;

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub id: String,
    pub title: String,
    pub query: String,
    pub items: Vec<PRSummary>,
    pub error: Option<String>,
    pub is_loading: bool,
}

pub struct InboxStore {
    pub sections: Vec<Section>,
    pub last_refresh: Option<Instant>,
    known_review_requests: Option<HashSet<String>>,
}

impl InboxStore {
    pub fn new(services: &Services) -> Self {
        let mut s = InboxStore { sections: vec![], last_refresh: None, known_review_requests: None };
        s.rebuild_sections(services);
        s
    }

    pub fn rebuild_sections(&mut self, services: &Services) {
        let mut s: Vec<Section> = InboxSection::ALL
            .iter()
            .map(|sec| Section {
                id: serde_json::to_value(sec).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
                title: sec.title().into(),
                query: sec.query().into(),
                items: vec![],
                error: None,
                is_loading: false,
            })
            .collect();
        for repo in services.settings().pinned_repos {
            s.push(Section {
                id: format!("repo:{repo}"),
                title: repo.clone(),
                query: format!("is:pr is:open repo:{repo} sort:updated-desc"),
                items: vec![],
                error: None,
                is_loading: false,
            });
        }
        // Keep already-loaded items when pinned repos change.
        let old: HashMap<String, Section> = self.sections.drain(..).map(|x| (x.id.clone(), x)).collect();
        self.sections = s
            .into_iter()
            .map(|mut n| {
                if let Some(o) = old.get(&n.id) {
                    n.items = o.items.clone();
                }
                n
            })
            .collect();
    }
}

#[derive(Default)]
pub struct UiFlags {
    pub show_open_url: bool,
    pub show_palette: bool,
    pub show_sidebar: bool,
    pub show_discuss_panel: bool,
    pub show_submit_review: bool,
    pub show_push_sheet: bool,
    pub show_settings: bool,
}

pub struct AppState {
    pub services: Arc<Services>,
    pub spawner: Spawner,
    pub inbox: InboxStore,
    pub prs: HashMap<PRRef, PRStore>,
    cache_order: Vec<PRRef>,
    pub current: Option<PRRef>,
    pub ui: UiFlags,
    last_inbox_timer: Instant,
    last_update_timer: Instant,
}

impl AppState {
    pub fn new(services: Arc<Services>, spawner: Spawner) -> Self {
        let inbox = InboxStore::new(&services);
        AppState {
            services,
            spawner,
            inbox,
            prs: HashMap::new(),
            cache_order: vec![],
            current: None,
            ui: UiFlags { show_sidebar: true, ..Default::default() },
            last_inbox_timer: Instant::now(),
            last_update_timer: Instant::now(),
        }
    }

    pub fn current_pr(&self) -> Option<&PRStore> {
        self.current.as_ref().and_then(|r| self.prs.get(r))
    }

    pub fn current_pr_mut(&mut self) -> Option<&mut PRStore> {
        let r = self.current.clone()?;
        self.prs.get_mut(&r)
    }

    #[allow(dead_code)]
    pub fn pr_mut(&mut self, r: &PRRef) -> Option<&mut PRStore> {
        self.prs.get_mut(r)
    }

    pub fn open(&mut self, r: PRRef) {
        if let Some(s) = self.prs.get_mut(&r) {
            self.current = Some(r);
            s.check_for_updates(false);
            return;
        }
        let store = PRStore::new(r.clone(), self.services.clone(), self.spawner.clone());
        self.prs.insert(r.clone(), store);
        self.cache_order.push(r.clone());
        // Keep a handful of PRs warm; drop the oldest.
        if self.cache_order.len() > 6 {
            let old = self.cache_order.remove(0);
            if Some(&old) != self.current.as_ref() && old != r {
                if let Some(s) = self.prs.remove(&old) {
                    for c in s.controllers.values() {
                        c.shutdown();
                    }
                }
            }
        }
        self.current = Some(r.clone());
        self.prs.get_mut(&r).unwrap().load();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn open_url(&mut self, s: &str) -> bool {
        match PRRef::parse(s) {
            Some(r) => {
                self.open(r);
                true
            }
            None => false,
        }
    }

    pub fn reload_current(&mut self) {
        if let Some(s) = self.current_pr_mut() {
            s.refresh();
        }
        self.refresh_inbox();
    }

    pub fn pinned_repos_changed(&mut self) {
        self.inbox.rebuild_sections(&self.services);
        self.refresh_inbox();
    }

    pub fn refresh_inbox(&mut self) {
        self.inbox.rebuild_sections(&self.services);
        self.inbox.last_refresh = Some(Instant::now());
        let total = self.inbox.sections.len();
        let remaining = Arc::new(std::sync::atomic::AtomicUsize::new(total));
        for section in &mut self.inbox.sections {
            section.is_loading = true;
            let api = self.services.github.clone();
            let (id, q) = (section.id.clone(), section.query.clone());
            let remaining = remaining.clone();
            self.spawner.spawn(move || {
                let r = api.search(&q);
                Box::new(move |app: &mut AppState| {
                    if let Some(s) = app.inbox.sections.iter_mut().find(|s| s.id == id) {
                        s.is_loading = false;
                        match r {
                            Ok(items) => {
                                s.items = items;
                                s.error = None;
                            }
                            Err(e) => s.error = Some(e.to_string()),
                        }
                    }
                    if remaining.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                        app.notify_new_review_requests();
                    }
                })
            });
        }
    }

    /// Refresh when the app becomes active, at most once a minute.
    pub fn refresh_inbox_if_stale(&mut self) {
        if self.inbox.last_refresh.map(|t| t.elapsed() < Duration::from_secs(60)).unwrap_or(false) {
            return;
        }
        self.refresh_inbox();
    }

    fn notify_new_review_requests(&mut self) {
        let Some(section) = self.inbox.sections.iter().find(|s| s.id == "reviewRequested") else { return };
        let ids: HashSet<String> = section.items.iter().map(|i| i.id()).collect();
        let known = self.inbox.known_review_requests.replace(ids);
        let Some(known) = known else { return };
        if !self.services.settings().notify_on_review_requests || cfg!(test) {
            return;
        }
        for pr in section.items.iter().filter(|i| !known.contains(&i.id())) {
            let title = format!("Review requested: {}", pr.pr_ref);
            let body = format!("{} — @{}", pr.title, pr.author);
            std::thread::spawn(move || {
                let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
                let script = format!("display notification \"{}\" with title \"{}\"", esc(&body), esc(&title));
                let _ = std::process::Command::new("/usr/bin/osascript").args(["-e", &script]).output();
            });
        }
    }

    /// Timers: inbox every 5 minutes, PR head check every minute.
    pub fn tick(&mut self) {
        if self.last_inbox_timer.elapsed() >= Duration::from_secs(300) {
            self.last_inbox_timer = Instant::now();
            self.refresh_inbox();
        }
        if self.last_update_timer.elapsed() >= Duration::from_secs(60) {
            self.last_update_timer = Instant::now();
            if let Some(s) = self.current_pr_mut() {
                s.check_for_updates(false);
            }
        }
    }
}
