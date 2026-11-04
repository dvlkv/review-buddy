//! Subprocess execution shared by git, gh and claude callers. Blocking: call it from a
//! worker thread, never from the UI thread.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug, Default)]
pub struct ProcessResult {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ProcessResult {
    pub fn stdout_string(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
    pub fn stderr_string(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessError {
    pub command: String,
    pub status: i32,
    pub stderr: String,
}

impl std::fmt::Display for ProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = self.stderr.trim();
        write!(f, "`{}` failed ({})", self.command, self.status)?;
        if !msg.is_empty() {
            write!(f, ": {}", msg)?;
        }
        Ok(())
    }
}

impl std::error::Error for ProcessError {}

/// Runs subprocesses. A trait so tests can script git/gh responses.
pub trait CommandRunning: Send + Sync {
    fn run(
        &self,
        executable: &str,
        args: &[String],
        cwd: Option<&Path>,
        env: Option<&HashMap<String, String>>,
        stdin: Option<&[u8]>,
    ) -> Result<ProcessResult, String>;

    /// Runs and fails on non-zero exit.
    fn check(
        &self,
        executable: &str,
        args: &[String],
        cwd: Option<&Path>,
        env: Option<&HashMap<String, String>>,
        stdin: Option<&[u8]>,
    ) -> Result<ProcessResult, String> {
        let r = self.run(executable, args, cwd, env, stdin)?;
        if r.status != 0 {
            let name = Path::new(executable)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| executable.to_string());
            let mut parts = vec![name];
            parts.extend(args.iter().cloned());
            return Err(ProcessError { command: parts.join(" "), status: r.status, stderr: r.stderr_string() }.to_string());
        }
        Ok(r)
    }
}

/// Convenience: `args!["a", x, "b"]` → `Vec<String>`.
#[macro_export]
macro_rules! args {
    ($($a:expr),* $(,)?) => { vec![$(String::from($a)),*] };
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessRunner;

impl ProcessRunner {
    pub fn new() -> Self {
        ProcessRunner
    }
}

pub fn base_command(executable: &str, args: &[String], cwd: Option<&Path>, env: Option<&HashMap<String, String>>) -> Command {
    let mut cmd = Command::new(executable);
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    cmd.env("PATH", ToolLocator::search_path());
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    if let Some(env) = env {
        for (k, v) in env {
            cmd.env(k, v);
        }
    }
    cmd
}

impl CommandRunning for ProcessRunner {
    fn run(
        &self,
        executable: &str,
        args: &[String],
        cwd: Option<&Path>,
        env: Option<&HashMap<String, String>>,
        stdin: Option<&[u8]>,
    ) -> Result<ProcessResult, String> {
        let mut cmd = base_command(executable, args, cwd, env);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
        let mut child = cmd.spawn().map_err(|e| format!("Couldn't start {}: {}", executable, e))?;
        // Drain both pipes concurrently so large outputs (e.g. a 50 MB diff) never deadlock.
        let mut out = child.stdout.take().unwrap();
        let mut err = child.stderr.take().unwrap();
        let out_t = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = out.read_to_end(&mut b);
            b
        });
        let err_t = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = err.read_to_end(&mut b);
            b
        });
        if let (Some(data), Some(mut sin)) = (stdin, child.stdin.take()) {
            let data = data.to_vec();
            std::thread::spawn(move || {
                let _ = sin.write_all(&data);
            });
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        let stdout = out_t.join().unwrap_or_default();
        let stderr = err_t.join().unwrap_or_default();
        Ok(ProcessResult { status: status.code().unwrap_or(-1), stdout, stderr })
    }
}

/// Finds CLI tools when launched from Finder (where PATH is minimal).
pub struct ToolLocator;

fn tool_cache() -> &'static Mutex<HashMap<String, String>> {
    static C: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

impl ToolLocator {
    pub fn search_dirs() -> Vec<String> {
        let home = dirs::home_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        vec![
            "/opt/homebrew/bin".into(),
            "/usr/local/bin".into(),
            format!("{home}/.local/bin"),
            format!("{home}/.claude/local"),
            format!("{home}/.bun/bin"),
            format!("{home}/.npm-global/bin"),
            format!("{home}/.cargo/bin"),
            "/usr/bin".into(),
            "/bin".into(),
            "/usr/sbin".into(),
            "/sbin".into(),
        ]
    }

    pub fn search_path() -> String {
        let existing: Vec<String> = std::env::var("PATH")
            .unwrap_or_default()
            .split(':')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        let mut seen = HashSet::new();
        existing
            .into_iter()
            .chain(Self::search_dirs())
            .filter(|d| seen.insert(d.clone()))
            .collect::<Vec<_>>()
            .join(":")
    }

    /// Absolute path to `tool`, or None. Checks PATH dirs, then the user's login shell.
    pub fn find(tool: &str) -> Option<String> {
        if let Some(c) = tool_cache().lock().unwrap().get(tool) {
            return Some(c.clone());
        }
        for dir in Self::search_path().split(':') {
            let p = PathBuf::from(dir).join(tool);
            if is_executable(&p) {
                let s = p.to_string_lossy().into_owned();
                tool_cache().lock().unwrap().insert(tool.into(), s.clone());
                return Some(s);
            }
        }
        // nvm and friends: ask the login shell.
        let out = Command::new("/bin/zsh")
            .args(["-lc", &format!("command -v {}", tool)])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if out.status.success() && s.starts_with('/') {
            tool_cache().lock().unwrap().insert(tool.into(), s.clone());
            return Some(s);
        }
        None
    }
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_and_captures() {
        let r = ProcessRunner.run("/bin/sh", &args!["-c", "echo out; echo err >&2; exit 3"], None, None, None).unwrap();
        assert_eq!(r.status, 3);
        assert_eq!(r.stdout_string(), "out\n");
        assert_eq!(r.stderr_string(), "err\n");
        let e = ProcessRunner.check("/bin/sh", &args!["-c", "exit 2"], None, None, None).unwrap_err();
        assert!(e.contains("failed (2)"));
    }

    #[test]
    fn large_output_and_stdin() {
        let input = vec![b'x'; 3_000_000];
        let r = ProcessRunner.run("/bin/cat", &[], None, None, Some(&input)).unwrap();
        assert_eq!(r.stdout.len(), 3_000_000);
    }
}
