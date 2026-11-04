//! Every keyboard-reachable action. The diff view maps keys to actions through
//! [`KeyBindings`], and the command palette lists the same actions.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReviewAction {
    NextFile,
    PreviousFile,
    NextHunk,
    PreviousHunk,
    NextThread,
    PreviousThread,
    ToggleViewed,
    Comment,
    Discuss,
    Reply,
    ToggleSplit,
    #[serde(rename = "openByURL")]
    OpenByUrl,
    CommandPalette,
    Submit,
    ToggleSidebar,
    ToggleDiscussPanel,
    Cancel,
    Summarize,
    PreReview,
    Refresh,
    PushFixes,
    ExpandAllThreads,
    CollapseFile,
}

impl ReviewAction {
    /// All actions, in palette order.
    pub const ALL: [ReviewAction; 23] = [
        ReviewAction::NextFile,
        ReviewAction::PreviousFile,
        ReviewAction::NextHunk,
        ReviewAction::PreviousHunk,
        ReviewAction::NextThread,
        ReviewAction::PreviousThread,
        ReviewAction::ToggleViewed,
        ReviewAction::Comment,
        ReviewAction::Discuss,
        ReviewAction::Reply,
        ReviewAction::ToggleSplit,
        ReviewAction::OpenByUrl,
        ReviewAction::CommandPalette,
        ReviewAction::Submit,
        ReviewAction::ToggleSidebar,
        ReviewAction::ToggleDiscussPanel,
        ReviewAction::Cancel,
        ReviewAction::Summarize,
        ReviewAction::PreReview,
        ReviewAction::Refresh,
        ReviewAction::PushFixes,
        ReviewAction::ExpandAllThreads,
        ReviewAction::CollapseFile,
    ];

    /// Stable identifier (the Swift raw value).
    pub fn id(self) -> &'static str {
        match self {
            ReviewAction::NextFile => "nextFile",
            ReviewAction::PreviousFile => "previousFile",
            ReviewAction::NextHunk => "nextHunk",
            ReviewAction::PreviousHunk => "previousHunk",
            ReviewAction::NextThread => "nextThread",
            ReviewAction::PreviousThread => "previousThread",
            ReviewAction::ToggleViewed => "toggleViewed",
            ReviewAction::Comment => "comment",
            ReviewAction::Discuss => "discuss",
            ReviewAction::Reply => "reply",
            ReviewAction::ToggleSplit => "toggleSplit",
            ReviewAction::OpenByUrl => "openByURL",
            ReviewAction::CommandPalette => "commandPalette",
            ReviewAction::Submit => "submit",
            ReviewAction::ToggleSidebar => "toggleSidebar",
            ReviewAction::ToggleDiscussPanel => "toggleDiscussPanel",
            ReviewAction::Cancel => "cancel",
            ReviewAction::Summarize => "summarize",
            ReviewAction::PreReview => "preReview",
            ReviewAction::Refresh => "refresh",
            ReviewAction::PushFixes => "pushFixes",
            ReviewAction::ExpandAllThreads => "expandAllThreads",
            ReviewAction::CollapseFile => "collapseFile",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            ReviewAction::NextFile => "Next file",
            ReviewAction::PreviousFile => "Previous file",
            ReviewAction::NextHunk => "Next hunk",
            ReviewAction::PreviousHunk => "Previous hunk",
            ReviewAction::NextThread => "Next thread",
            ReviewAction::PreviousThread => "Previous thread",
            ReviewAction::ToggleViewed => "Toggle viewed",
            ReviewAction::Comment => "Comment on selection",
            ReviewAction::Discuss => "Discuss selection with Claude",
            ReviewAction::Reply => "Reply to thread",
            ReviewAction::ToggleSplit => "Toggle split / unified",
            ReviewAction::OpenByUrl => "Open pull request by URL…",
            ReviewAction::CommandPalette => "Command palette",
            ReviewAction::Submit => "Submit",
            ReviewAction::ToggleSidebar => "Toggle inbox sidebar",
            ReviewAction::ToggleDiscussPanel => "Toggle Discuss panel",
            ReviewAction::Cancel => "Cancel",
            ReviewAction::Summarize => "Summarize pull request",
            ReviewAction::PreReview => "Run AI pre-review",
            ReviewAction::Refresh => "Refresh",
            ReviewAction::PushFixes => "Push fix commits…",
            ReviewAction::ExpandAllThreads => "Expand all threads",
            ReviewAction::CollapseFile => "Collapse / expand file",
        }
    }

    /// Display string for menus and the palette.
    pub fn shortcut(self) -> Option<&'static str> {
        Some(match self {
            ReviewAction::NextFile => "J",
            ReviewAction::PreviousFile => "K",
            ReviewAction::NextHunk => "N",
            ReviewAction::PreviousHunk => "P",
            ReviewAction::NextThread => "]",
            ReviewAction::PreviousThread => "[",
            ReviewAction::ToggleViewed => "V",
            ReviewAction::Comment => "C",
            ReviewAction::Discuss => "D",
            ReviewAction::Reply => "R",
            ReviewAction::ToggleSplit => "S",
            ReviewAction::OpenByUrl => "⌘L",
            ReviewAction::CommandPalette => "⌘K",
            ReviewAction::Submit => "⌘↩",
            ReviewAction::ToggleSidebar => "⌃⌘S",
            ReviewAction::ToggleDiscussPanel => "⌘\\",
            ReviewAction::Cancel => "Esc",
            ReviewAction::Refresh => "⌘R",
            ReviewAction::CollapseFile => "X",
            _ => return None,
        })
    }
}

/// A key press. `key` is stored lowercased; `"\r"` is Return, `"\u{1b}"` is Escape.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct KeyStroke {
    pub key: String,
    pub command: bool,
    pub shift: bool,
    pub option: bool,
    pub control: bool,
}

impl KeyStroke {
    pub fn new(key: &str) -> Self {
        KeyStroke { key: key.to_lowercase(), ..Default::default() }
    }
    pub fn with_modifiers(key: &str, command: bool, shift: bool, option: bool, control: bool) -> Self {
        KeyStroke { key: key.to_lowercase(), command, shift, option, control }
    }
    pub fn command(mut self) -> Self {
        self.command = true;
        self
    }
    pub fn shift(mut self) -> Self {
        self.shift = true;
        self
    }
    pub fn option(mut self) -> Self {
        self.option = true;
        self
    }
    pub fn control(mut self) -> Self {
        self.control = true;
        self
    }
}

pub struct KeyBindings;

impl KeyBindings {
    fn lookup(s: &KeyStroke) -> Option<ReviewAction> {
        use ReviewAction::*;
        // (key, command, shift, option, control)
        Some(match (s.key.as_str(), s.command, s.shift, s.option, s.control) {
            ("j", false, false, false, false) => NextFile,
            ("k", false, false, false, false) => PreviousFile,
            ("n", false, false, false, false) => NextHunk,
            ("p", false, false, false, false) => PreviousHunk,
            ("]", false, false, false, false) => NextThread,
            ("[", false, false, false, false) => PreviousThread,
            ("v", false, false, false, false) => ToggleViewed,
            ("c", false, false, false, false) => Comment,
            ("d", false, false, false, false) => Discuss,
            ("r", false, false, false, false) => Reply,
            ("s", false, false, false, false) => ToggleSplit,
            ("x", false, false, false, false) => CollapseFile,
            ("l", true, false, false, false) => OpenByUrl,
            ("k", true, false, false, false) => CommandPalette,
            ("\r", true, false, false, false) => Submit,
            ("\\", true, false, false, false) => ToggleDiscussPanel,
            ("s", true, false, false, true) => ToggleSidebar,
            ("\u{1b}", false, false, false, false) => Cancel,
            ("r", true, false, false, false) => Refresh,
            _ => return None,
        })
    }

    /// Single-key shortcuts only fire when no text field has focus.
    pub fn action(stroke: &KeyStroke, text_input_focused: bool) -> Option<ReviewAction> {
        let a = Self::lookup(stroke)?;
        let is_plain = !stroke.command && !stroke.control && !stroke.option;
        if text_input_focused && is_plain && a != ReviewAction::Cancel {
            return None;
        }
        Some(a)
    }

    /// Fuzzy subsequence score for the command palette (higher is better, `None` = no match).
    pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i64> {
        let q: Vec<char> = query.to_lowercase().chars().collect();
        let c: Vec<char> = candidate.to_lowercase().chars().collect();
        if q.is_empty() {
            return Some(0);
        }
        let (mut score, mut qi, mut last) = (0i64, 0usize, -2i64);
        let mut ci = 0usize;
        while ci < c.len() && qi < q.len() {
            if c[ci] == q[qi] {
                score += if ci as i64 == last + 1 { 5 } else { 1 };
                if ci == 0 || matches!(c[ci - 1], ' ' | '/' | '.') {
                    score += 4;
                }
                last = ci as i64;
                qi += 1;
            }
            ci += 1;
        }
        if qi != q.len() {
            return None;
        }
        Some(score * 100 - c.len() as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_keys() {
        let a = |s: KeyStroke| KeyBindings::action(&s, false);
        assert_eq!(a(KeyStroke::new("j")), Some(ReviewAction::NextFile));
        assert_eq!(a(KeyStroke::new("J")), Some(ReviewAction::NextFile));
        assert_eq!(a(KeyStroke::new("d")), Some(ReviewAction::Discuss));
        assert_eq!(a(KeyStroke::new("c")), Some(ReviewAction::Comment));
        assert_eq!(a(KeyStroke::new("k").command()), Some(ReviewAction::CommandPalette));
        assert_eq!(a(KeyStroke::new("s").command().control()), Some(ReviewAction::ToggleSidebar));
        assert_eq!(a(KeyStroke::new("q")), None);
        assert_eq!(a(KeyStroke::new("j").shift()), None);
    }

    #[test]
    fn text_focus_suppresses_plain_keys() {
        let a = |s: KeyStroke| KeyBindings::action(&s, true);
        assert_eq!(a(KeyStroke::new("j")), None);
        assert_eq!(a(KeyStroke::new("\u{1b}")), Some(ReviewAction::Cancel));
        assert_eq!(a(KeyStroke::new("\r").command()), Some(ReviewAction::Submit));
        assert_eq!(a(KeyStroke::new("l").command()), Some(ReviewAction::OpenByUrl));
    }

    #[test]
    fn every_action_has_title() {
        for a in ReviewAction::ALL {
            assert!(!a.title().is_empty());
            assert!(!a.id().is_empty());
            assert_eq!(serde_json::to_value(a).unwrap(), serde_json::json!(a.id()));
        }
        assert_eq!(ReviewAction::Discuss.shortcut(), Some("D"));
        assert_eq!(ReviewAction::Summarize.shortcut(), None);
    }

    #[test]
    fn fuzzy_score() {
        assert_eq!(KeyBindings::fuzzy_score("", "anything"), Some(0));
        assert_eq!(KeyBindings::fuzzy_score("xyz", "next file"), None);
        assert!(KeyBindings::fuzzy_score("nf", "Next file").is_some());
        let contiguous = KeyBindings::fuzzy_score("next", "Next file").unwrap();
        let scattered = KeyBindings::fuzzy_score("next", "No extra text").unwrap();
        assert!(contiguous > scattered);
        let short_name = KeyBindings::fuzzy_score("app", "src/App.swift").unwrap();
        let long_name = KeyBindings::fuzzy_score("app", "src/very/long/path/to/App.swift").unwrap();
        assert!(short_name > long_name);
        assert_eq!(KeyBindings::fuzzy_score("ba", "ab"), None, "order matters");
    }
}
