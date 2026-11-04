//! Discuss conversation messages and the reducer that folds Claude events into them.

use super::stream_events::{ClaudeEvent, ClaudeResult, ContentBlock};
use crate::models::{new_id, Date};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
    Error,
    System,
}

/// One entry in a Discuss conversation as shown in the side panel.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscussMessage {
    pub id: String,
    pub role: Role,
    pub text: String,
    /// For tool calls: tool name, compact input, and result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<String>,
    #[serde(default)]
    pub tool_failed: bool,
    pub created_at: Date,
    /// True while text is still streaming.
    #[serde(default)]
    pub is_streaming: bool,
}

impl DiscussMessage {
    pub fn new(role: Role, text: impl Into<String>) -> Self {
        DiscussMessage {
            id: new_id(),
            role,
            text: text.into(),
            tool_name: None,
            tool_input: None,
            tool_result: None,
            tool_failed: false,
            created_at: Utc::now(),
            is_streaming: false,
        }
    }

    /// A tool-call row as created from a `tool_use` block.
    pub fn tool(id: impl Into<String>, name: impl Into<String>, input: impl Into<String>) -> Self {
        let name = name.into();
        DiscussMessage {
            id: id.into(),
            tool_name: Some(name.clone()),
            tool_input: Some(input.into()),
            ..DiscussMessage::new(Role::Tool, name)
        }
    }

    /// "Read src/a.ts", "Bash: npm test" — the folded tool row label.
    pub fn tool_summary(&self) -> String {
        let Some(name) = &self.tool_name else { return self.text.clone() };
        let obj = match self.tool_input.as_deref().map(serde_json::from_str::<serde_json::Value>) {
            Some(Ok(serde_json::Value::Object(o))) => o,
            _ => return name.clone(),
        };
        let s = |k: &str| obj.get(k).and_then(|v| v.as_str());
        fn short(p: &str) -> String {
            let parts: Vec<&str> = p.split('/').filter(|c| !c.is_empty()).collect();
            if parts.len() > 3 {
                format!("…/{}", parts[parts.len() - 3..].join("/"))
            } else {
                p.to_string()
            }
        }
        match name.as_str() {
            "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
                format!("{} {}", name, short(s("file_path").or(s("notebook_path")).unwrap_or("")))
            }
            "Bash" => {
                let cmd = s("command").unwrap_or("").split('\n').find(|l| !l.is_empty()).unwrap_or("");
                format!("Bash: {}", cmd.chars().take(80).collect::<String>())
            }
            "Grep" => format!("Grep \"{}\"", s("pattern").unwrap_or("")),
            "Glob" => format!("Glob {}", s("pattern").unwrap_or("")),
            "WebFetch" => format!("Fetch {}", s("url").unwrap_or("")),
            "Task" | "Agent" => format!("Agent: {}", s("description").unwrap_or("")),
            _ => name.clone(),
        }
    }
}

/// Folds `ClaudeEvent`s into a list of `DiscussMessage`s. Pure, so it is unit-tested
/// against recorded CLI output.
#[derive(Clone, Debug, Default)]
pub struct TranscriptReducer {
    messages: Vec<DiscussMessage>,
    session_id: Option<String>,
    is_running: bool,
    last_result: Option<ClaudeResult>,
    /// Index of the assistant message receiving streamed text.
    streaming_index: Option<usize>,
    tool_index_by_id: HashMap<String, usize>,
}

impl TranscriptReducer {
    pub fn new(messages: Vec<DiscussMessage>, session_id: Option<String>) -> Self {
        let tool_index_by_id =
            messages.iter().enumerate().filter(|(_, m)| m.role == Role::Tool).map(|(i, m)| (m.id.clone(), i)).collect();
        TranscriptReducer { messages, session_id, tool_index_by_id, ..Default::default() }
    }

    pub fn messages(&self) -> &[DiscussMessage] {
        &self.messages
    }
    pub fn into_messages(self) -> Vec<DiscussMessage> {
        self.messages
    }
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
    pub fn is_running(&self) -> bool {
        self.is_running
    }
    pub fn last_result(&self) -> Option<&ClaudeResult> {
        self.last_result.as_ref()
    }

    /// Records a user turn. `display` replaces `text` in the transcript (e.g. to hide
    /// the `<review_context>` preamble).
    pub fn user_sent(&mut self, text: &str, display: Option<&str>) {
        self.messages.push(DiscussMessage::new(Role::User, display.unwrap_or(text)));
        self.is_running = true;
        self.streaming_index = None;
    }

    pub fn apply(&mut self, event: &ClaudeEvent) {
        match event {
            ClaudeEvent::Initialized { session_id, .. } => self.session_id = Some(session_id.clone()),
            ClaudeEvent::TextDelta { text, .. } => {
                if let Some(i) = self.streaming_index {
                    self.messages[i].text.push_str(text);
                } else {
                    let mut m = DiscussMessage::new(Role::Assistant, text.clone());
                    m.is_streaming = true;
                    self.messages.push(m);
                    self.streaming_index = Some(self.messages.len() - 1);
                }
            }
            ClaudeEvent::Assistant { blocks, .. } => {
                for b in blocks {
                    match b {
                        ContentBlock::Text(t) => {
                            if let Some(i) = self.streaming_index.take() {
                                // Replace streamed text with the authoritative full text.
                                self.messages[i].text = t.clone();
                                self.messages[i].is_streaming = false;
                            } else if !t.is_empty() {
                                self.messages.push(DiscussMessage::new(Role::Assistant, t.clone()));
                            }
                        }
                        ContentBlock::Thinking(_) => {}
                        ContentBlock::ToolUse { id, name, input } => {
                            self.finish_streaming();
                            self.messages.push(DiscussMessage::tool(id.clone(), name.clone(), input.clone()));
                            self.tool_index_by_id.insert(id.clone(), self.messages.len() - 1);
                        }
                    }
                }
            }
            ClaudeEvent::ToolResults(results) => {
                for r in results {
                    if let Some(&i) = self.tool_index_by_id.get(&r.tool_use_id) {
                        self.messages[i].tool_result = Some(r.content.chars().take(20_000).collect());
                        self.messages[i].tool_failed = r.is_error;
                    }
                }
            }
            ClaudeEvent::Result(res) => {
                self.finish_streaming();
                self.is_running = false;
                self.last_result = Some(res.clone());
                if let Some(sid) = &res.session_id {
                    self.session_id = Some(sid.clone());
                }
                if res.is_error {
                    let text = res.text.clone().unwrap_or_else(|| format!("Claude stopped ({}).", res.subtype));
                    self.messages.push(DiscussMessage::new(Role::Error, text));
                }
            }
            ClaudeEvent::Other(_) => {}
        }
    }

    pub fn process_exited(&mut self, status: i32, stderr: &str) {
        self.finish_streaming();
        if self.is_running {
            self.is_running = false;
            let msg = stderr.trim();
            let text = if msg.is_empty() { format!("Claude exited ({status}).") } else { msg.to_string() };
            self.messages.push(DiscussMessage::new(Role::Error, text));
        }
    }

    pub fn interrupted(&mut self) {
        self.finish_streaming();
        self.is_running = false;
        self.messages.push(DiscussMessage::new(Role::System, "Stopped."));
    }

    fn finish_streaming(&mut self) {
        if let Some(i) = self.streaming_index.take() {
            self.messages[i].is_streaming = false;
        }
    }
}
