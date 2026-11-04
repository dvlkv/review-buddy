//! Generic-password storage in the login keychain, via the `security` CLI
//! (service `dev.reviewbuddy`, same as the Swift app).

use std::process::{Command, Stdio};

const SERVICE: &str = "dev.reviewbuddy";

pub struct Keychain;

impl Keychain {
    pub fn read(account: &str) -> Option<String> {
        let out = Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", SERVICE, "-a", account, "-w"])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string();
        if s.is_empty() { None } else { Some(s) }
    }

    /// Stores `value`, or deletes the item when `value` is empty.
    pub fn write(account: &str, value: &str) -> bool {
        let _ = Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", SERVICE, "-a", account])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if value.is_empty() {
            return true;
        }
        Command::new("/usr/bin/security")
            .args(["add-generic-password", "-s", SERVICE, "-a", account, "-w", value, "-U"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}
