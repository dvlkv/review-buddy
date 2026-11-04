// TODO: port the rest of Transcript.swift (TranscriptReducer).
use crate::models::{new_id, Date};
use chrono::Utc;
use serde::{Deserialize, Serialize};

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
}
