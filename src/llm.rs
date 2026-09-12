//! The LLM layer — OpenAI-compatible chat types, HTTP client, and the
//! tool registry.
//!
//! Port of `tk.ai.chat` + `tk.ai.client` + `tk.ai.AgentToolbox`
//! (tokamak). The wire format is byte-compatible with `tk.ai.chat` so
//! that session files saved by the Zig original can be loaded and vice
//! versa. Zig's `jsonSkipNull` serializes every field except `null`
//! optionals, in declaration order — replicated here with
//! `skip_serializing_if`.

use std::time::Duration;

use reqwest::blocking::Client as HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Config;

// =========================================================== chat types

/// A message content: a plain string or an array of typed content parts.
/// Zig's `TextOrContents` union, serialized untagged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Contents(Vec<ContentPart>),
}

impl Content {
    /// Extract plain text (Zig's `Choice.text()`): the string form, or the
    /// first `text` part in the contents form.
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(t) => Some(t),
            Self::Contents(parts) => parts.iter().find_map(|p| p.text.as_deref()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    Text,
    ImageUrl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageDetail {
    Low,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
    pub detail: ImageDetail,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentPart {
    #[serde(rename = "type")]
    pub part_type: ContentType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<ImageUrl>,
}

impl ContentPart {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            part_type: ContentType::Text,
            text: Some(text.into()),
            image_url: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
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

impl Tool {
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: Value) -> Self {
        Self {
            tool_type: ToolType::Function,
            function: FunctionSpec {
                name: name.into(),
                description: Some(description.into()),
                parameters,
                strict: true,
            },
        }
    }
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
    /// Zig's `Choice.text()`.
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
    /// Zig's `Response.singleChoice()`: only the single-element case.
    pub fn single_choice(&self) -> Option<&Choice> {
        (self.choices.len() == 1).then(|| &self.choices[0])
    }

    /// Total tokens for this response (0 if the server omitted usage).
    pub fn total_tokens(&self) -> u32 {
        self.usage.map(|u| u.total_tokens).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default = "default_max_tokens")]
    pub max_completion_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
}

fn default_max_tokens() -> u32 {
    4096
}

impl Request {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            tools: None,
            response_format: None,
            reasoning_effort: None,
            max_completion_tokens: 4096,
            temperature: None,
            top_p: None,
        }
    }
}

// ============================================================== client

// Port of `tk.ai.client` — a thin JSON-over-HTTP wrapper used against
// llama.cpp's OpenAI-compatible endpoint.

/// Short, human-readable error names surfaced in the TUI (Zig parity:
/// `@errorName(err)` strings like "Timeout", "FileNotFound").
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

#[derive(Debug)]
pub struct Client {
    http: HttpClient,
    base_url: String,
    api_key: Option<String>,
}

impl Client {
    pub fn new(config: &Config) -> Self {
        let http = HttpClient::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("failed to build HTTP client");

        // Keep the base URL without a trailing slash; the endpoint is
        // appended below (same as tokamak's `http.Client.request`).
        let base_url = config.base_url.trim_end_matches('/').to_string();

        Self {
            http,
            base_url,
            api_key: config.api_key.clone(),
        }
    }

    pub fn create_chat_completion(&self, params: &Request) -> Result<Response, String> {
        let mut req = self
            .http
            .post(format!("{}/chat/completions", self.base_url));

        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

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
// Port of `tk.ai.AgentToolbox` + `AgentTool`. Schemas are hand-written
// (5 static tools) instead of generated from arg structs. Errors are
// returned as plain "ErrorName" strings which become the tool result
// message — exactly what Zig's `execTool` does with `@errorName(e)`.

pub type ToolHandler = fn(&Value) -> Result<String, String>;

pub struct ToolEntry {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
    pub handler: ToolHandler,
}

impl ToolEntry {
    pub fn new(
        name: &'static str,
        description: &'static str,
        parameters: Value,
        handler: ToolHandler,
    ) -> Self {
        Self {
            name,
            description,
            parameters,
            handler,
        }
    }
}

#[derive(Default)]
pub struct Toolbox {
    tools: Vec<ToolEntry>,
}

impl Toolbox {
    pub fn add(&mut self, entry: ToolEntry) {
        self.tools.push(entry);
    }

    /// Names of all registered tools (used to enable all tools on an agent).
    pub fn all_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name.to_string()).collect()
    }

    /// Build the request `tools` array for the given names (Zig's `query`).
    pub fn query(&self, names: &[String]) -> Vec<Tool> {
        names
            .iter()
            .filter_map(|name| self.tools.iter().find(|t| t.name == name.as_str()))
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
    /// Never fails — unknown tools and handler errors come back as result
    /// strings (Zig's `execTool`: `error.NotFound`, `@errorName(e)`).
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
