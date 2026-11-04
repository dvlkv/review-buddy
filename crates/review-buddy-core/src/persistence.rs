//! JSON-file persistence under the data dir (`~/Library/Application Support/ReviewBuddy`
//! on macOS). File names, JSON keys and date format match the Swift app so existing files
//! keep loading. PR state writes are atomic, debounced and coalesced on a background thread.

use crate::diff::layout::DiffViewMode;
use crate::models::{AISummary, DiscussThread, FixRequest, PRRef, PreReviewResult};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Default delay before a PR state save hits the disk.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(400);

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub pinned_repos: Vec<String>,
    /// `owner/repo` → path of the user's local clone.
    pub repo_clones: HashMap<String, String>,
    pub view_mode: DiffViewMode,
    pub auto_summarize: bool,
    /// Run the AI pre-review automatically when a PR has at most this many changed lines (0 = never).
    pub auto_pre_review_max_lines: i64,
    pub claude_model: String,
    pub notify_on_review_requests: bool,
    pub large_file_threshold: i64,
    pub font_size: f64,
    pub hide_whitespace: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            pinned_repos: Vec::new(),
            repo_clones: HashMap::new(),
            view_mode: DiffViewMode::Unified,
            auto_summarize: false,
            auto_pre_review_max_lines: 0,
            claude_model: String::new(),
            notify_on_review_requests: true,
            large_file_threshold: 2000,
            font_size: 12.0,
            hide_whitespace: false,
        }
    }
}

impl AppSettings {
    pub fn local_clone(&self, name_with_owner: &str) -> Option<PathBuf> {
        self.repo_clones.get(name_with_owner).map(PathBuf::from)
    }

    /// Tolerant decoding so new settings never wipe old ones: each field that is missing or
    /// malformed falls back to its default on its own. Fails only when `v` isn't an object.
    pub fn from_value(v: &Value) -> Option<AppSettings> {
        let obj = v.as_object()?;
        fn field<T: DeserializeOwned>(obj: &Map<String, Value>, key: &str, default: T) -> T {
            obj.get(key).and_then(|v| T::deserialize(v).ok()).unwrap_or(default)
        }
        let d = AppSettings::default();
        Some(AppSettings {
            pinned_repos: field(obj, "pinnedRepos", d.pinned_repos),
            repo_clones: field(obj, "repoClones", d.repo_clones),
            view_mode: field(obj, "viewMode", d.view_mode),
            auto_summarize: field(obj, "autoSummarize", d.auto_summarize),
            auto_pre_review_max_lines: field(obj, "autoPreReviewMaxLines", d.auto_pre_review_max_lines),
            claude_model: field(obj, "claudeModel", d.claude_model),
            notify_on_review_requests: field(obj, "notifyOnReviewRequests", d.notify_on_review_requests),
            large_file_threshold: field(obj, "largeFileThreshold", d.large_file_threshold),
            font_size: field(obj, "fontSize", d.font_size),
            hide_whitespace: field(obj, "hideWhitespace", d.hide_whitespace),
        })
    }
}

impl<'de> Deserialize<'de> for AppSettings {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(deserializer)?;
        AppSettings::from_value(&v).ok_or_else(|| serde::de::Error::custom("settings must be a JSON object"))
    }
}

/// Everything Review Buddy remembers about one PR.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PRLocalState {
    pub discuss_threads: Vec<DiscussThread>,
    pub fix_requests: Vec<FixRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<AISummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_review: Option<PreReviewResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen_head: Option<String>,
    /// Files the user chose to review first, in order; `None` follows the AI reading order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_order: Option<Vec<String>>,
}

#[derive(Default)]
struct State {
    cache: HashMap<String, PRLocalState>,
    /// storage key → when its debounced write is due.
    pending: HashMap<String, Instant>,
    shutdown: bool,
}

struct Shared {
    root: PathBuf,
    state: Mutex<State>,
    wake: Condvar,
    /// Held while snapshotting and writing PR files, so an older snapshot can never land
    /// on disk after a newer one.
    write_lock: Mutex<()>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn pr_path(&self, key: &str) -> PathBuf {
        self.root.join("prs").join(format!("{key}.json"))
    }

    /// Writes the pending entries due by `now` (all of them when `None`).
    fn write_pending(&self, now: Option<Instant>) {
        let _w = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let batch: Vec<(String, PRLocalState)> = {
            let mut st = self.lock();
            let keys: Vec<String> =
                st.pending.iter().filter(|(_, due)| now.is_none_or(|n| **due <= n)).map(|(k, _)| k.clone()).collect();
            keys.into_iter()
                .filter_map(|k| {
                    st.pending.remove(&k);
                    st.cache.get(&k).cloned().map(|s| (k, s))
                })
                .collect()
        };
        for (key, state) in batch {
            if let Ok(data) = encode_sorted(&state) {
                let _ = write_atomic(&self.pr_path(&key), &data);
            }
        }
    }

    fn run_writer(self: Arc<Self>) {
        loop {
            {
                let mut st = self.lock();
                loop {
                    if st.shutdown {
                        return;
                    }
                    let now = Instant::now();
                    match st.pending.values().min().copied() {
                        Some(due) if due <= now => break,
                        Some(due) => st = self.wake.wait_timeout(st, due - now).unwrap_or_else(|e| e.into_inner()).0,
                        None => st = self.wake.wait(st).unwrap_or_else(|e| e.into_inner()),
                    }
                }
            }
            self.write_pending(Some(Instant::now()));
        }
    }
}

/// JSON-file persistence. Thread-safe: share it through an `Arc`. PR state is cached in
/// memory; saves are debounced and written by a background thread. Dropping the store
/// flushes pending writes.
pub struct LocalStore {
    shared: Arc<Shared>,
}

impl Default for LocalStore {
    fn default() -> Self {
        LocalStore::new(LocalStore::default_root())
    }
}

impl LocalStore {
    pub fn default_root() -> PathBuf {
        dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("ReviewBuddy")
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let _ = std::fs::create_dir_all(root.join("prs"));
        let shared = Arc::new(Shared {
            root,
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            write_lock: Mutex::new(()),
        });
        let worker = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("local-store-writer".into())
            .spawn(move || worker.run_writer())
            .expect("spawn local store writer thread");
        LocalStore { shared }
    }

    pub fn root(&self) -> &Path {
        &self.shared.root
    }

    fn settings_path(&self) -> PathBuf {
        self.shared.root.join("settings.json")
    }

    /// Settings from disk; defaults when the file is missing or unreadable.
    pub fn load_settings(&self) -> AppSettings {
        std::fs::read(self.settings_path())
            .ok()
            .and_then(|d| serde_json::from_slice::<Value>(&d).ok())
            .and_then(|v| AppSettings::from_value(&v))
            .unwrap_or_default()
    }

    /// Writes settings now (atomically).
    pub fn save_settings(&self, s: &AppSettings) -> io::Result<()> {
        write_atomic(&self.settings_path(), &encode_sorted(s)?)
    }

    /// The PR's state: cached, else from disk, else empty.
    pub fn load(&self, pr: &PRRef) -> PRLocalState {
        let key = pr.storage_key();
        if let Some(c) = self.shared.lock().cache.get(&key) {
            return c.clone();
        }
        let from_disk = std::fs::read(self.shared.pr_path(&key))
            .ok()
            .and_then(|d| serde_json::from_slice::<PRLocalState>(&d).ok())
            .unwrap_or_default();
        // A save may have landed while reading; it wins.
        self.shared.lock().cache.entry(key).or_insert(from_disk).clone()
    }

    /// Saves after [`DEFAULT_DEBOUNCE`], coalescing bursts (e.g. streaming transcript updates).
    pub fn save(&self, pr: &PRRef, state: PRLocalState) {
        self.save_with_debounce(pr, state, DEFAULT_DEBOUNCE);
    }

    pub fn save_with_debounce(&self, pr: &PRRef, state: PRLocalState, debounce: Duration) {
        let key = pr.storage_key();
        let mut st = self.shared.lock();
        st.cache.insert(key.clone(), state);
        st.pending.insert(key, Instant::now() + debounce);
        drop(st);
        self.shared.wake.notify_all();
    }

    /// Loads, mutates and saves (debounced) the PR's state.
    pub fn update(&self, pr: &PRRef, body: impl FnOnce(&mut PRLocalState)) {
        let mut s = self.load(pr);
        let key = pr.storage_key();
        let mut st = self.shared.lock();
        // Re-read under the lock so concurrent updates don't lose each other's changes.
        if let Some(c) = st.cache.get(&key) {
            s = c.clone();
        }
        body(&mut s);
        st.cache.insert(key.clone(), s);
        st.pending.insert(key, Instant::now() + DEFAULT_DEBOUNCE);
        drop(st);
        self.shared.wake.notify_all();
    }

    /// Writes every pending save now.
    pub fn flush(&self) {
        self.shared.write_pending(None);
    }
}

impl Drop for LocalStore {
    fn drop(&mut self) {
        self.flush();
        self.shared.lock().shutdown = true;
        self.shared.wake.notify_all();
    }
}

/// JSON with object keys sorted at every level, like Swift's `.sortedKeys`.
fn encode_sorted<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    let v = serde_json::to_value(value).map_err(io::Error::other)?;
    serde_json::to_vec(&sort_keys(v)).map_err(io::Error::other)
}

fn sort_keys(v: Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut entries: Vec<(String, Value)> = m.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k, sort_keys(v))).collect())
        }
        Value::Array(a) => Value::Array(a.into_iter().map(sort_keys).collect()),
        other => other,
    }
}

/// Writes to a temp file next to `path`, then renames it into place.
fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let res = std::fs::write(&tmp, data).and_then(|_| std::fs::rename(&tmp, path));
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempRoot(PathBuf);
    impl TempRoot {
        fn new() -> Self {
            TempRoot(std::env::temp_dir().join(format!("rb-store-{}", uuid::Uuid::new_v4())))
        }
    }
    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn summary(s: &str) -> AISummary {
        serde_json::from_value(serde_json::json!({ "summary": s, "readingOrder": [], "risks": [] })).unwrap()
    }

    #[test]
    fn settings_round_trip() {
        let root = TempRoot::new();
        let store = LocalStore::new(&root.0);
        assert_eq!(store.load_settings(), AppSettings::default());
        let s = AppSettings {
            pinned_repos: vec!["o/r".into()],
            repo_clones: HashMap::from([("o/r".into(), "/code/r".into())]),
            view_mode: DiffViewMode::Split,
            claude_model: "opus".into(),
            ..Default::default()
        };
        store.save_settings(&s).unwrap();
        assert_eq!(LocalStore::new(&root.0).load_settings(), s);
        assert_eq!(s.local_clone("o/r"), Some(PathBuf::from("/code/r")));
        assert_eq!(s.local_clone("x/y"), None);

        // camelCase keys, sorted.
        let text = std::fs::read_to_string(root.0.join("settings.json")).unwrap();
        assert!(
            text.starts_with(r#"{"autoPreReviewMaxLines":0,"autoSummarize":false,"claudeModel":"opus","#),
            "{text}"
        );
        assert!(text.contains(r#""viewMode":"split""#));
    }

    #[test]
    fn tolerant_settings_decoding() {
        let root = TempRoot::new();
        let store = LocalStore::new(&root.0);
        let path = root.0.join("settings.json");
        std::fs::write(&path, r#"{"pinnedRepos":["a/b"],"viewMode":"bogus","fontSize":"big","futureKey":1}"#).unwrap();
        let s = store.load_settings();
        assert_eq!(s.pinned_repos, vec!["a/b".to_string()]);
        assert_eq!(s.view_mode, DiffViewMode::Unified);
        assert_eq!(s.font_size, 12.0);
        assert_eq!(s.large_file_threshold, 2000);
        assert!(s.notify_on_review_requests);
        // Through serde too.
        let s2: AppSettings = serde_json::from_str(r#"{"hideWhitespace":true,"fontSize":14}"#).unwrap();
        assert!(s2.hide_whitespace && s2.font_size == 14.0);
        std::fs::write(&path, "garbage").unwrap();
        assert_eq!(store.load_settings(), AppSettings::default());
        std::fs::write(&path, "[1,2]").unwrap();
        assert_eq!(store.load_settings(), AppSettings::default());
    }

    #[test]
    fn pr_state_debounce_flush_and_update() {
        let root = TempRoot::new();
        let pr = PRRef::new("o", "r", 7);
        let file = root.0.join("prs/o__r__7.json");
        let store = LocalStore::new(&root.0);
        assert_eq!(store.load(&pr), PRLocalState::default());

        let st = PRLocalState { last_seen_head: Some("abc".into()), ..Default::default() };
        store.save_with_debounce(&pr, st, Duration::from_secs(30));
        assert_eq!(store.load(&pr).last_seen_head.as_deref(), Some("abc"), "cache serves unsaved state");
        assert!(!file.exists(), "write is debounced");
        store.flush();
        assert!(file.exists());
        assert_eq!(LocalStore::new(&root.0).load(&pr).last_seen_head.as_deref(), Some("abc"));
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text, r#"{"discussThreads":[],"fixRequests":[],"lastSeenHead":"abc"}"#);

        store.update(&pr, |s| s.summary = Some(summary("S")));
        assert_eq!(store.load(&pr).summary.map(|s| s.summary).as_deref(), Some("S"));
        assert_eq!(store.load(&pr).last_seen_head.as_deref(), Some("abc"));
        // Default debounce eventually writes.
        std::thread::sleep(Duration::from_millis(900));
        assert_eq!(LocalStore::new(&root.0).load(&pr).summary.map(|s| s.summary).as_deref(), Some("S"));
    }

    #[test]
    fn coalesces_bursts() {
        let root = TempRoot::new();
        let pr = PRRef::new("o", "r", 8);
        let store = LocalStore::new(&root.0);
        for i in 0..50 {
            let st = PRLocalState { last_seen_head: Some(format!("h{i}")), ..Default::default() };
            store.save_with_debounce(&pr, st, Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(LocalStore::new(&root.0).load(&pr).last_seen_head.as_deref(), Some("h49"));
    }

    #[test]
    fn drop_flushes_and_store_is_shareable() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LocalStore>();
        let root = TempRoot::new();
        let pr = PRRef::new("o", "r", 9);
        let store = Arc::new(LocalStore::new(&root.0));
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let store = Arc::clone(&store);
                let pr = pr.clone();
                std::thread::spawn(move || {
                    store.update(&pr, |s| s.review_order.get_or_insert_with(Vec::new).push(format!("f{i}")))
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        drop(store);
        let loaded = LocalStore::new(&root.0).load(&pr);
        assert_eq!(loaded.review_order.map(|o| o.len()), Some(8));
    }

    #[test]
    fn loads_swift_written_state() {
        let root = TempRoot::new();
        std::fs::create_dir_all(root.0.join("prs")).unwrap();
        std::fs::write(
            root.0.join("prs/o__r__3.json"),
            r#"{"discussThreads":[],"fixRequests":[],"lastSeenHead":"h","reviewOrder":["a.swift"],"summary":{"readingOrder":[],"risks":[],"summary":"S"}}"#,
        )
        .unwrap();
        let s = LocalStore::new(&root.0).load(&PRRef::new("o", "r", 3));
        assert_eq!(s.review_order, Some(vec!["a.swift".to_string()]));
        assert_eq!(s.summary.map(|s| s.summary).as_deref(), Some("S"));
    }
}
