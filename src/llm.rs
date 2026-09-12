//! The LLM layer — OpenAI-compatible chat types, HTTP client, and the
//! tool registry.
//!
//! The wire format is a stable contract: session files saved by older
//! versions must stay loadable, so field names and order must not
//! change. Only `null` optionals are omitted (`skip_serializing_if`),
//! and fields serialize in declaration order.
//!
//! The request/response structs mirror the full OpenAI-compatible
//! schema as returned by the server — keep every field, even the ones
//! we never read or write; the server may still populate them.

use std::time::Duration;

use reqwest::blocking::Client as HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Config;

// =========================================================== chat types

/// A message content: a plain string or an array of typed content
/// parts, serialized untagged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Contents(Vec<ContentPart>),
}

impl Content {
    /// Extract plain text: the string form, or the first `text` part in
    /// the contents form.
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(t) => Some(t),
            Self::Contents(parts) => parts.iter().find_map(|p| p.text.as_deref()),
        }
    }
}

// NOTE: the OpenAI wire format also supports image content parts
// (`type: "image_url"`); we intentionally don't model it — nothing in
// clown-code can produce image content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    Text,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentPart {
    #[serde(rename = "type")]
    pub part_type: ContentType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(text: impl Into<String>) -> Self {
        Self::new(Role::System, Some(text))
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::new(Role::User, Some(text))
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self::new(Role::Assistant, Some(text))
    }

    pub fn tool(text: impl Into<String>, tool_call_id: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(Content::Text(text.into())),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    fn new(role: Role, text: Option<impl Into<String>>) -> Self {
        Self {
            role,
            content: text.map(|t| Content::Text(t.into())),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolType {
    Function,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionSpec {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: Value,
    #[serde(default = "default_true")]
    pub strict: bool,
}

fn default_true() -> bool {
    true
}

/// A tool definition sent in the request's `tools` array.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    #[serde(rename = "type", default = "default_tool_type")]
    pub tool_type: ToolType,
    pub function: FunctionSpec,
}

fn default_tool_type() -> ToolType {
    ToolType::Function
}

/// A tool call requested by the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_tool_type")]
    pub tool_type: ToolType,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// JSON-encoded arguments.
    pub arguments: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    FunctionCall,
    ToolCalls,
    ContentFilter,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    #[serde(default)]
    pub logprobs: Option<Value>,
    pub finish_reason: FinishReason,
}

impl Choice {
    /// Plain text of the choice, if any.
    pub fn text(&self) -> Option<&str> {
        self.message.content.as_ref()?.text()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_fingerprint: Option<String>,
}

impl Response {
    /// The choice, only in the single-element case.
    pub fn single_choice(&self) -> Option<&Choice> {
        (self.choices.len() == 1).then(|| &self.choices[0])
    }

    /// Total tokens for this response (0 if the server omitted usage).
    pub fn total_tokens(&self) -> u32 {
        self.usage.map(|u| u.total_tokens).unwrap_or(0)
    }
}

/// Request body only (never deserialized).
#[derive(Debug, Clone, Serialize)]
pub struct Request {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    pub max_completion_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
}

// ============================================================== client

// A thin JSON-over-HTTP wrapper for llama.cpp's OpenAI-compatible endpoint.

/// Short, human-readable error names surfaced in the TUI
/// ("Timeout", "ConnectionFailed", ...).
pub fn error_name(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return "Timeout".into();
    }
    if e.is_connect() {
        return "ConnectionFailed".into();
    }
    if e.is_request() {
        // JSON parse / send errors
        return "RequestFailed".into();
    }
    "HttpError".into()
}

#[derive(Debug, Clone)]
pub struct Client {
    http: HttpClient,
    base_url: String,
}

impl Client {
    pub fn new(config: &Config) -> Self {
        let http = HttpClient::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("failed to build HTTP client");

        // Keep the base URL without a trailing slash; the endpoint is
        // appended below.
        let base_url = config.base_url.trim_end_matches('/').to_string();

        Self { http, base_url }
    }

    pub fn create_chat_completion(&self, params: &Request) -> Result<Response, String> {
        let req = self
            .http
            .post(format!("{}/chat/completions", self.base_url));

        let res = req.json(params).send().map_err(|e| error_name(&e))?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().unwrap_or_default();
            return Err(format!("HttpError: server returned {status}: {body}"));
        }

        res.json::<Response>().map_err(|e| error_name(&e))
    }
}

// =========================================================== toolbox

// Tool registry and dispatch.
//
// Schemas are hand-written (5 static tools) instead of generated from
// arg structs. Errors are returned as plain short "ErrorName" strings
// which become the tool result message.

pub type ToolHandler = fn(&Value) -> Result<String, String>;

#[derive(Clone)]
pub struct ToolEntry {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
    pub handler: ToolHandler,
}

#[derive(Default, Clone)]
pub struct Toolbox {
    tools: Vec<ToolEntry>,
}

impl Toolbox {
    pub fn add(&mut self, entry: ToolEntry) {
        self.tools.push(entry);
    }

    /// Build the request `tools` array for all registered tools.
    pub fn all_tools(&self) -> Vec<Tool> {
        self.tools
            .iter()
            .map(|t| Tool {
                tool_type: ToolType::Function,
                function: FunctionSpec {
                    name: t.name.to_string(),
                    description: Some(t.description.to_string()),
                    parameters: t.parameters.clone(),
                    strict: true,
                },
            })
            .collect()
    }

    /// Run a tool by name with JSON arguments.
    /// Never fails — unknown tools and handler errors come back as
    /// result strings ("NotFound", handler error names).
    pub fn exec(&self, name: &str, args_json: &str) -> String {
        let Some(entry) = self.tools.iter().find(|t| t.name == name) else {
            return "NotFound".into();
        };
        let Ok(args) = serde_json::from_str(args_json) else {
            return "InvalidArguments".into();
        };
        let handler = entry.handler;
        handler(&args).unwrap_or_else(|e| e)
    }
}
