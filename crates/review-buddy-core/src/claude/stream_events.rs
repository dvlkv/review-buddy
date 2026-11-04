//! Decoding of `claude --output-format stream-json` NDJSON lines, and encoding of the
//! lines we write to `--input-format stream-json`. Pure: no I/O.

use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};

use super::session::ClaudeError;

/// A content block of a complete assistant message.
#[derive(Clone, Debug, PartialEq)]
pub enum ContentBlock {
    Text(String),
    Thinking(String),
    /// `input` is the tool input as compact JSON with sorted keys.
    ToolUse { id: String, name: String, input: String },
}

/// A tool result reported back to the model.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn new(tool_use_id: impl Into<String>, content: impl Into<String>, is_error: bool) -> Self {
        ToolResult { tool_use_id: tool_use_id.into(), content: content.into(), is_error }
    }
}

/// The final `result` line of a turn (or of a one-shot run).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ClaudeResult {
    pub session_id: Option<String>,
    pub is_error: bool,
    pub subtype: String,
    pub text: Option<String>,
    pub cost_usd: Option<f64>,
    /// `structured_output` when `--json-schema` was used (`null` maps to `None`).
    pub structured_output: Option<Value>,
}

impl ClaudeResult {
    /// Decodes the structured output, falling back to parsing `text` as JSON
    /// (what the Swift app did for summaries and pre-reviews).
    pub fn decode_output<T: DeserializeOwned>(&self) -> Result<T, ClaudeError> {
        if let Some(v) = &self.structured_output {
            return serde_json::from_value(v.clone()).map_err(|_| ClaudeError::BadOutput);
        }
        match &self.text {
            Some(t) => serde_json::from_str(t).map_err(|_| ClaudeError::BadOutput),
            None => Err(ClaudeError::BadOutput),
        }
    }
}

/// A decoded line of `claude --output-format stream-json`.
#[derive(Clone, Debug, PartialEq)]
pub enum ClaudeEvent {
    Initialized { session_id: String, model: Option<String> },
    /// Streaming text chunk (`--include-partial-messages`).
    TextDelta { message_id: Option<String>, text: String },
    /// A complete assistant message.
    Assistant { message_id: String, blocks: Vec<ContentBlock> },
    /// Tool results reported back to the model.
    ToolResults(Vec<ToolResult>),
    Result(ClaudeResult),
    /// Anything else, e.g. `rate_limit_event`, `system/thinking_tokens`, `stream_event/message_start`.
    Other(String),
}

impl ClaudeEvent {
    pub fn other(t: impl Into<String>) -> Self {
        ClaudeEvent::Other(t.into())
    }
}

/// Stream-json codec.
pub struct StreamJson;

fn str_of<'a>(o: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    o.get(k).and_then(Value::as_str)
}

/// Re-sorts object keys recursively so output is stable regardless of serde_json features.
fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for k in keys {
                out.insert(k.clone(), sorted(&m[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

fn line(v: Value) -> Vec<u8> {
    let mut d = serde_json::to_vec(&v).unwrap_or_default();
    d.push(b'\n');
    d
}

impl StreamJson {
    /// Decodes a single NDJSON line. Returns `None` for empty lines and anything that is
    /// not a JSON object. Unknown shapes become `Other` — the CLI adds event types over
    /// time and we must never fail on them.
    pub fn decode_line(line: &[u8]) -> Option<ClaudeEvent> {
        if line.is_empty() {
            return None;
        }
        let v: Value = serde_json::from_slice(line).ok()?;
        v.as_object().map(Self::decode_object)
    }

    pub fn decode_object(obj: &Map<String, Value>) -> ClaudeEvent {
        let ty = str_of(obj, "type").unwrap_or("");
        match ty {
            "system" => {
                let subtype = str_of(obj, "subtype").unwrap_or("");
                if subtype == "init" {
                    if let Some(sid) = str_of(obj, "session_id") {
                        return ClaudeEvent::Initialized {
                            session_id: sid.to_string(),
                            model: str_of(obj, "model").map(str::to_string),
                        };
                    }
                }
                ClaudeEvent::Other(format!("system/{subtype}"))
            }
            "stream_event" => {
                let Some(ev) = obj.get("event").and_then(Value::as_object) else {
                    return ClaudeEvent::other(ty);
                };
                let ev_type = str_of(ev, "type").unwrap_or("");
                if ev_type == "content_block_delta" {
                    if let Some(delta) = ev.get("delta").and_then(Value::as_object) {
                        if str_of(delta, "type") == Some("text_delta") {
                            if let Some(text) = str_of(delta, "text") {
                                return ClaudeEvent::TextDelta { message_id: None, text: text.to_string() };
                            }
                        }
                    }
                }
                ClaudeEvent::Other(format!("stream_event/{ev_type}"))
            }
            "assistant" => {
                let Some(msg) = obj.get("message").and_then(Value::as_object) else {
                    return ClaudeEvent::other(ty);
                };
                let id = str_of(msg, "id").map(str::to_string).unwrap_or_else(crate::models::new_id);
                let blocks = msg
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|arr| arr.iter().filter_map(Value::as_object).filter_map(decode_block).collect())
                    .unwrap_or_default();
                ClaudeEvent::Assistant { message_id: id, blocks }
            }
            "user" => {
                let Some(content) =
                    obj.get("message").and_then(Value::as_object).and_then(|m| m.get("content")).and_then(Value::as_array)
                else {
                    return ClaudeEvent::other(ty);
                };
                let results: Vec<ToolResult> = content
                    .iter()
                    .filter_map(Value::as_object)
                    .filter(|b| str_of(b, "type") == Some("tool_result"))
                    .map(|b| ToolResult {
                        tool_use_id: str_of(b, "tool_use_id").unwrap_or("").to_string(),
                        content: flatten_content(b.get("content")),
                        is_error: b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                    })
                    .collect();
                if results.is_empty() {
                    ClaudeEvent::other(ty)
                } else {
                    ClaudeEvent::ToolResults(results)
                }
            }
            "result" => ClaudeEvent::Result(ClaudeResult {
                session_id: str_of(obj, "session_id").map(str::to_string),
                is_error: obj.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                subtype: str_of(obj, "subtype").unwrap_or("").to_string(),
                text: str_of(obj, "result").map(str::to_string),
                cost_usd: obj.get("total_cost_usd").and_then(Value::as_f64),
                structured_output: obj.get("structured_output").filter(|v| !v.is_null()).cloned(),
            }),
            _ => ClaudeEvent::other(ty),
        }
    }

    /// The NDJSON line (with trailing newline) for a user turn.
    pub fn user_message(text: &str) -> Vec<u8> {
        line(json!({"type": "user", "message": {"role": "user", "content": text}}))
    }

    /// A `control_request` asking the CLI to stop the current turn, with a fresh id.
    pub fn interrupt_request() -> Vec<u8> {
        Self::interrupt_request_with_id(&uuid::Uuid::new_v4().to_string().to_uppercase())
    }

    pub fn interrupt_request_with_id(id: &str) -> Vec<u8> {
        line(json!({"type": "control_request", "request_id": id, "request": {"subtype": "interrupt"}}))
    }
}

fn decode_block(b: &Map<String, Value>) -> Option<ContentBlock> {
    match str_of(b, "type")? {
        "text" => Some(ContentBlock::Text(str_of(b, "text").unwrap_or("").to_string())),
        "thinking" => Some(ContentBlock::Thinking(str_of(b, "thinking").unwrap_or("").to_string())),
        "tool_use" => {
            let input = b
                .get("input")
                .and_then(|v| serde_json::to_string(&sorted(v)).ok())
                .unwrap_or_else(|| "{}".to_string());
            Some(ContentBlock::ToolUse {
                id: str_of(b, "id").unwrap_or("").to_string(),
                name: str_of(b, "name").unwrap_or("tool").to_string(),
                input,
            })
        }
        _ => None,
    }
}

/// Tool result content is either a string or an array of `{type:"text", text}` blocks.
fn flatten_content(c: Option<&Value>) -> String {
    match c {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|b| b.as_object().and_then(|o| str_of(o, "text")))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Splits a byte stream into lines without losing partial lines between reads.
/// Empty lines are dropped.
#[derive(Clone, Debug, Default)]
pub struct LineSplitter {
    pending: Vec<u8>,
}

impl LineSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Vec<Vec<u8>> {
        self.pending.extend_from_slice(chunk);
        let mut lines = Vec::new();
        let mut start = 0;
        while let Some(off) = self.pending[start..].iter().position(|&b| b == b'\n') {
            let end = start + off;
            if end > start {
                lines.push(self.pending[start..end].to_vec());
            }
            start = end + 1;
        }
        self.pending.drain(..start);
        lines
    }

    /// Returns the unterminated tail, if any, and resets.
    pub fn flush(&mut self) -> Option<Vec<u8>> {
        let p = std::mem::take(&mut self.pending);
        if p.is_empty() {
            None
        } else {
            Some(p)
        }
    }
}
