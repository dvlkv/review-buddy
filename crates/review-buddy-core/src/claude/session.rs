//! The `claude` CLI as a subprocess: a long-lived stream-json session for Discuss,
//! and blocking one-shot runs for summaries, pre-reviews and comment drafts.
//!
//! No async runtime: a reader thread per process delivers [`SessionOutput`] through a
//! callback (the UI wraps it to push into a channel and call `ctx.request_repaint()`).

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::stream_events::{ClaudeEvent, ClaudeResult, StreamJson};
use crate::git::process::{base_command, CommandRunning, ProcessRunner, ToolLocator};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeConfig {
    pub executable: String,
    pub cwd: PathBuf,
    pub resume_session_id: Option<String>,
    pub fork_session: bool,
    pub append_system_prompt: Option<String>,
    pub model: Option<String>,
    pub permission_mode: String,
    pub json_schema: Option<String>,
    pub extra_args: Vec<String>,
}

impl ClaudeConfig {
    pub fn new(executable: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        ClaudeConfig {
            executable: executable.into(),
            cwd: cwd.into(),
            resume_session_id: None,
            fork_session: false,
            append_system_prompt: None,
            model: None,
            permission_mode: "bypassPermissions".into(),
            json_schema: None,
            extra_args: vec![],
        }
    }

    /// Arguments for a long-lived streaming session.
    pub fn streaming_args(&self) -> Vec<String> {
        let mut a: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-mode",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        a.push(self.permission_mode.clone());
        a.extend(self.common_args());
        a
    }

    /// Arguments for a one-shot run that prints a single JSON result.
    pub fn one_shot_args(&self, prompt: &str) -> Vec<String> {
        let mut a: Vec<String> =
            ["-p", "--output-format", "json", "--permission-mode"].iter().map(|s| s.to_string()).collect();
        a.push(self.permission_mode.clone());
        a.extend(self.common_args());
        if let Some(schema) = &self.json_schema {
            a.push("--json-schema".into());
            a.push(schema.clone());
        }
        a.push(prompt.to_string());
        a
    }

    fn common_args(&self) -> Vec<String> {
        let mut a = Vec::new();
        if let Some(id) = &self.resume_session_id {
            a.push("--resume".into());
            a.push(id.clone());
        }
        if self.fork_session {
            a.push("--fork-session".into());
        }
        if let Some(p) = self.append_system_prompt.as_ref().filter(|p| !p.is_empty()) {
            a.push("--append-system-prompt".into());
            a.push(p.clone());
        }
        if let Some(m) = self.model.as_ref().filter(|m| !m.is_empty()) {
            a.push("--model".into());
            a.push(m.clone());
        }
        a.extend(self.extra_args.iter().cloned());
        a
    }
}

/// Absolute path of the `claude` CLI.
pub fn find_claude() -> Result<String, ClaudeError> {
    ToolLocator::find("claude").ok_or(ClaudeError::NotInstalled)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaudeError {
    NotInstalled,
    Failed(String),
    BadOutput,
}

impl std::fmt::Display for ClaudeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaudeError::NotInstalled => {
                f.write_str("The `claude` CLI was not found. Install Claude Code and sign in with `claude`.")
            }
            ClaudeError::Failed(s) => f.write_str(s),
            ClaudeError::BadOutput => f.write_str("Claude returned output that could not be parsed."),
        }
    }
}

impl std::error::Error for ClaudeError {}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionOutput {
    Event(ClaudeEvent),
    /// The process exited; delivered after all of its stdout events.
    Exited { status: i32, stderr: String },
}

pub type OutputCallback = Arc<dyn Fn(SessionOutput) + Send + Sync + 'static>;

/// What the UI needs from a Discuss session; implemented by [`ClaudeSession`] and by test fakes.
pub trait ClaudeSessionApi: Send {
    fn send(&self, text: &str) -> Result<(), ClaudeError>;
    fn interrupt(&self);
    fn terminate(&self);
    fn set_turn_active(&self, active: bool);
}

/// The currently running process.
struct Proc {
    pid: u32,
    stdin: Option<ChildStdin>,
    alive: Arc<AtomicBool>,
}

struct Shared {
    config: ClaudeConfig,
    proc: Mutex<Option<Proc>>,
    session_id: Mutex<Option<String>>,
    turn_active: AtomicBool,
    finished: AtomicBool,
    on_output: OutputCallback,
}

impl Shared {
    fn emit(&self, out: SessionOutput) {
        if !self.finished.load(Ordering::SeqCst) {
            (self.on_output)(out);
        }
    }
}

/// A long-lived `claude` process speaking stream-json over stdin/stdout.
/// Multiple user turns are written to the same process; if it exits, the next
/// `send` restarts it with `--resume <session_id>`. Dropping the session terminates
/// the process.
pub struct ClaudeSession {
    shared: Arc<Shared>,
}

fn signal(pid: u32, sig: &str) {
    let _ = Command::new("/bin/kill")
        .args([sig, &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

impl ClaudeSession {
    /// `on_output` is called from a background thread, in order, for every event and exit.
    pub fn new(config: ClaudeConfig, on_output: impl Fn(SessionOutput) + Send + Sync + 'static) -> Self {
        let session_id = config.resume_session_id.clone();
        ClaudeSession {
            shared: Arc::new(Shared {
                config,
                proc: Mutex::new(None),
                session_id: Mutex::new(session_id),
                turn_active: AtomicBool::new(false),
                finished: AtomicBool::new(false),
                on_output: Arc::new(on_output),
            }),
        }
    }

    /// Channel flavour: outputs go to the returned receiver and `wake` is called after
    /// each one (e.g. `move || ctx.request_repaint()`).
    pub fn with_channel(config: ClaudeConfig, wake: impl Fn() + Send + Sync + 'static) -> (Self, Receiver<SessionOutput>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let s = Self::new(config, move |out| {
            let _ = tx.lock().unwrap().send(out);
            wake();
        });
        (s, rx)
    }

    pub fn config(&self) -> &ClaudeConfig {
        &self.shared.config
    }

    pub fn is_process_running(&self) -> bool {
        self.shared.proc.lock().unwrap().as_ref().is_some_and(|p| p.alive.load(Ordering::SeqCst))
    }

    pub fn current_session_id(&self) -> Option<String> {
        self.shared.session_id.lock().unwrap().clone()
    }

    pub fn send(&self, text: &str) -> Result<(), ClaudeError> {
        let mut guard = self.shared.proc.lock().unwrap();
        self.ensure_process(&mut guard)?;
        let stdin = guard.as_mut().and_then(|p| p.stdin.as_mut()).ok_or(ClaudeError::Failed("Claude is not running.".into()))?;
        stdin
            .write_all(&StreamJson::user_message(text))
            .and_then(|_| stdin.flush())
            .map_err(|e| ClaudeError::Failed(format!("Couldn't write to claude: {e}")))
    }

    /// Asks Claude to stop the current turn; falls back to SIGINT after 2 s if the
    /// turn is still active.
    pub fn interrupt(&self) {
        let (pid, alive) = {
            let mut guard = self.shared.proc.lock().unwrap();
            let Some(p) = guard.as_mut().filter(|p| p.alive.load(Ordering::SeqCst)) else { return };
            if let Some(stdin) = p.stdin.as_mut() {
                let _ = stdin.write_all(&StreamJson::interrupt_request()).and_then(|_| stdin.flush());
            }
            (p.pid, p.alive.clone())
        };
        let shared = Arc::downgrade(&self.shared);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            let Some(shared) = shared.upgrade() else { return };
            let guard = shared.proc.lock().unwrap();
            let still_current = guard.as_ref().is_some_and(|p| p.pid == pid);
            if still_current && alive.load(Ordering::SeqCst) && shared.turn_active.load(Ordering::SeqCst) {
                signal(pid, "-INT");
            }
        });
    }

    /// Set by the owner from the transcript; decides whether the SIGINT fallback is needed.
    pub fn set_turn_active(&self, active: bool) {
        self.shared.turn_active.store(active, Ordering::SeqCst);
    }

    pub fn turn_active(&self) -> bool {
        self.shared.turn_active.load(Ordering::SeqCst)
    }

    /// Closes stdin and sends SIGTERM. The process's `Exited` output is still delivered.
    pub fn terminate(&self) {
        if let Some(mut p) = self.shared.proc.lock().unwrap().take() {
            drop(p.stdin.take());
            if p.alive.load(Ordering::SeqCst) {
                signal(p.pid, "-TERM");
            }
        }
    }

    /// Terminates and stops delivering any further output.
    pub fn finish(&self) {
        self.shared.finished.store(true, Ordering::SeqCst);
        self.terminate();
    }

    fn ensure_process(&self, slot: &mut Option<Proc>) -> Result<(), ClaudeError> {
        if slot.as_ref().is_some_and(|p| p.alive.load(Ordering::SeqCst)) {
            return Ok(());
        }
        let mut cfg = self.shared.config.clone();
        if let Some(sid) = self.shared.session_id.lock().unwrap().clone() {
            cfg.resume_session_id = Some(sid);
        }
        let mut cmd = base_command(&cfg.executable, &cfg.streaming_args(), Some(&cfg.cwd), None);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ClaudeError::NotInstalled
            } else {
                ClaudeError::Failed(format!("Couldn't start claude: {e}"))
            }
        })?;
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let alive = Arc::new(AtomicBool::new(true));
        *slot = Some(Proc { pid: child.id(), stdin: child.stdin.take(), alive: alive.clone() });

        let err_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf);
            buf
        });
        // Weak so a dropped session doesn't keep its callback alive through the reader.
        let shared = Arc::downgrade(&self.shared);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if line.last() == Some(&b'\n') {
                    line.pop();
                }
                let Some(ev) = StreamJson::decode_line(&line) else { continue };
                let Some(shared) = shared.upgrade() else { continue };
                match &ev {
                    ClaudeEvent::Initialized { session_id, .. } => {
                        *shared.session_id.lock().unwrap() = Some(session_id.clone());
                    }
                    ClaudeEvent::Result(ClaudeResult { session_id: Some(sid), .. }) => {
                        *shared.session_id.lock().unwrap() = Some(sid.clone());
                    }
                    _ => {}
                }
                shared.emit(SessionOutput::Event(ev));
            }
            // Report exit only after stdout and stderr hit EOF and the process ended,
            // so no trailing events are lost.
            let stderr = err_thread.join().unwrap_or_default();
            let status = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
            alive.store(false, Ordering::SeqCst);
            if let Some(shared) = shared.upgrade() {
                shared.emit(SessionOutput::Exited { status, stderr: String::from_utf8_lossy(&stderr).into_owned() });
            }
        });
        Ok(())
    }
}

impl Drop for ClaudeSession {
    fn drop(&mut self) {
        self.terminate();
    }
}

impl ClaudeSessionApi for ClaudeSession {
    fn send(&self, text: &str) -> Result<(), ClaudeError> {
        ClaudeSession::send(self, text)
    }
    fn interrupt(&self) {
        ClaudeSession::interrupt(self)
    }
    fn terminate(&self) {
        ClaudeSession::terminate(self)
    }
    fn set_turn_active(&self, active: bool) {
        ClaudeSession::set_turn_active(self, active)
    }
}

/// One-shot `claude -p --output-format json` runs (summary, pre-review, comment drafts).
/// Blocking: call from a worker thread.
pub trait ClaudeOneShotRunning: Send + Sync {
    fn run(&self, config: &ClaudeConfig, prompt: &str) -> Result<ClaudeResult, ClaudeError>;
}

pub struct ClaudeOneShot {
    runner: Arc<dyn CommandRunning>,
}

impl Default for ClaudeOneShot {
    fn default() -> Self {
        ClaudeOneShot { runner: Arc::new(ProcessRunner) }
    }
}

impl ClaudeOneShot {
    pub fn new(runner: Arc<dyn CommandRunning>) -> Self {
        ClaudeOneShot { runner }
    }
}

impl ClaudeOneShotRunning for ClaudeOneShot {
    fn run(&self, config: &ClaudeConfig, prompt: &str) -> Result<ClaudeResult, ClaudeError> {
        let r = self
            .runner
            .run(&config.executable, &config.one_shot_args(prompt), Some(&config.cwd), None, None)
            .map_err(ClaudeError::Failed)?;
        // The CLI prints the result object even on error exits.
        let last = r.stdout.split(|&b| b == b'\n').rfind(|l| !l.is_empty()).unwrap_or(&r.stdout);
        match StreamJson::decode_line(last) {
            Some(ClaudeEvent::Result(res)) => {
                if res.is_error {
                    Err(ClaudeError::Failed(
                        res.text.unwrap_or_else(|| format!("Claude reported an error ({}).", res.subtype)),
                    ))
                } else {
                    Ok(res)
                }
            }
            _ if r.status != 0 => {
                let err = r.stderr_string();
                Err(ClaudeError::Failed(if err.is_empty() { format!("claude exited with {}", r.status) } else { err }))
            }
            _ => Err(ClaudeError::BadOutput),
        }
    }
}
