//! The model — agent loop, session snapshots, worker thread, and the
//! `Clown` conversation state machine.
//!
//! Port of `src/model.zig` + `tk.ai.agent` (tokamak). In the Zig
//! original the agent loop runs in a forked child whose messages arrive
//! over a pipe; here the same flow is a worker thread + mpsc, and the
//! parent's `agent` is updated exclusively via snapshots — exactly as
//! in the Zig code (`tick()` → `loadSnapshot()`).

use std::io;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::llm::{Client, Message, Request, Response, Role, ToolCall, Toolbox};
use crate::{prompt, tools};

// =============================================================== agent

/// Agent loop — port of `tk.ai.agent` (`Agent` + `AgentRuntime`).

#[derive(Debug, Clone)]
pub struct AgentOptions {
    pub model: String,
    /// Names of enabled tools (empty = no tools in the request).
    pub tools: Vec<String>,
    pub max_completion_tokens: u32,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub auto_retry: u8,
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            model: "default".into(),
            tools: Vec::new(),
            max_completion_tokens: 4096,
            temperature: None,
            top_p: None,
            auto_retry: 1,
        }
    }
}

/// Error names mirror the short `@errorName` strings surfaced in the Zig
/// TUI ("NoChoice", "RetryFailed", HTTP error names, ...).
#[derive(Debug)]
pub enum AgentError {
    NoChoice,
    RetryFailed,
    Client(String),
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoChoice => write!(f, "NoChoice"),
            Self::RetryFailed => write!(f, "RetryFailed"),
            Self::Client(e) => write!(f, "{e}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Agent {
    pub options: AgentOptions,
    pub messages: Vec<Message>,
    pub total_tokens: u32,
}

impl Agent {
    pub fn new(options: AgentOptions) -> Self {
        Self {
            options,
            messages: Vec::new(),
            total_tokens: 0,
        }
    }

    pub fn add_message(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    /// Make one LLM call. Returns the assistant message's tool calls (if
    /// any). Retries on empty choices; appends the assistant message to
    /// the conversation. Mirrors `Agent.next()`.
    pub fn next(&mut self, runtime: &AgentRuntime) -> Result<Option<Vec<ToolCall>>, AgentError> {
        let mut retries = self.options.auto_retry + 1;
        while retries > 0 {
            retries -= 1;
            let res = runtime.create_completion(self)?;
            // Zig: agent.total_tokens = res.usage.total_tokens
            self.total_tokens = res.total_tokens();

            let Some(choice) = res.single_choice() else {
                return Err(AgentError::NoChoice);
            };
            let is_empty = choice.text().is_none_or(str::is_empty);

            if choice.message.tool_calls.is_none() && is_empty {
                tracing::debug!("empty choice, retrying");
                continue;
            }

            self.add_message(choice.message.clone());
            return Ok(choice.message.tool_calls.clone());
        }
        Err(AgentError::RetryFailed)
    }

    /// Execute all tool calls and append tool result messages.
    pub fn accept_all(&mut self, runtime: &AgentRuntime, tcs: &[ToolCall]) {
        for tc in tcs {
            self.accept(runtime, tc);
        }
    }

    pub fn accept(&mut self, runtime: &AgentRuntime, tc: &ToolCall) {
        let content = runtime.exec_tool(tc);
        self.respond(tc, content);
    }

    pub fn respond(&mut self, tc: &ToolCall, content: String) {
        self.add_message(Message::tool(content, tc.id.clone()));
    }

    /// Remove the trailing assistant/tool messages and, if a user message
    /// now trails, remove and return it (to allow re-prompting).
    /// Mirrors `Agent.undo()`.
    pub fn undo(&mut self) -> Option<Message> {
        while let Some(msg) = self.messages.last() {
            if msg.role != Role::Assistant && msg.role != Role::Tool {
                break;
            }
            self.messages.pop();
        }

        if let Some(msg) = self.messages.last() {
            if msg.role == Role::User {
                return self.messages.pop();
            }
        }
        None
    }
}

#[derive(Clone)]
pub struct AgentRuntime {
    client: Arc<Client>,
    toolbox: Arc<Toolbox>,
}

impl AgentRuntime {
    pub fn new(client: Client, toolbox: Arc<Toolbox>) -> Self {
        Self {
            client: Arc::new(client),
            toolbox,
        }
    }

    pub fn toolbox(&self) -> &Toolbox {
        &self.toolbox
    }

    /// Mirrors `AgentRuntime.createCompletion()`.
    fn create_completion(&self, agent: &Agent) -> Result<Response, AgentError> {
        let tools = self.toolbox.query(&agent.options.tools);
        let request = Request {
            model: agent.options.model.clone(),
            messages: agent.messages.clone(),
            tools: (!tools.is_empty()).then_some(tools),
            response_format: None,
            reasoning_effort: None,
            max_completion_tokens: agent.options.max_completion_tokens,
            temperature: agent.options.temperature,
            top_p: agent.options.top_p,
        };

        self.client
            .create_chat_completion(&request)
            .map_err(AgentError::Client)
    }

    /// Mirrors `AgentRuntime.execTool()` — never fails, errors become the
    /// result string.
    pub fn exec_tool(&self, tool: &ToolCall) -> String {
        self.toolbox
            .exec(&tool.function.name, &tool.function.arguments)
    }
}

// ============================================================= session

/// Session save/load — port of the snapshot code in `src/model.zig`.
///
/// Files live in the current working directory and are byte-compatible
/// with the Zig original's `session-*.json` format.

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub messages: Vec<Message>,
    pub total_tokens: u32,
}

/// `session-{YYYY-MM-DD HH:MM:SS UTC}.json` (matches Zig's naming).
pub fn session_filename() -> String {
    let now = Local::now();
    format!("session-{} UTC.json", now.format("%Y-%m-%d %H:%M:%S"))
}

/// Save the snapshot to a session file (2-space indent, like Zig).
pub fn save_session(snap: &Snapshot) -> io::Result<()> {
    let json = serde_json::to_string_pretty(snap).expect("snapshot is serializable");
    std::fs::write(session_filename(), json)
}

/// Load a snapshot from a session file. Unknown fields are ignored
/// (Zig's `.ignore_unknown_fields = true`).
pub fn load_session(filename: &str) -> io::Result<Snapshot> {
    let contents = std::fs::read_to_string(filename)?;
    let snap: Snapshot = serde_json::from_str(&contents)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(snap)
}

/// `/continue` — load the most recent session file, if any.
/// Same shell glob as the Zig original (`ls -1t session-*.json | head -1`).
pub fn continue_latest() -> Option<Snapshot> {
    let entry = tools::run_command_entry().handler;
    let out = entry(&serde_json::json!({
        "command": "ls -1t session-*.json 2>/dev/null | head -1"
    }))
    .ok()?;
    let filename = out.trim();
    if filename.is_empty() {
        return None;
    }
    load_session(filename).ok()
}

// ============================================================== worker

// Agent-loop worker thread.
//
// Port of the fork-based worker in `src/model.zig`. Zig forks a child
// process and pipes line-delimited JSON back so the TUI can `SIGKILL`
// an in-flight LLM call. The Rust equivalent:
//
// - the agent loop runs on a dedicated OS thread (blocking reqwest),
// - messages arrive over an `mpsc` channel as `WorkerMsg` (same
//   externally-tagged JSON shape as the Zig pipe protocol),
// - stopping the worker drops the receiver; the thread's next
//   `send()` fails and the loop exits. A thread blocked in an HTTP
//   call finishes it and then bails — the server may keep generating,
//   but we ignore the result, which matches SIGKILL for our purposes.

/// Port of `WorkerMsg` — a Zig `union(enum)` with the same JSON shape
/// (externally tagged: `{"snapshot": ...}` / `{"err": ...}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerMsg {
    Snapshot(Snapshot),
    Err(String),
}

impl WorkerMsg {
    pub fn snapshot(agent: &Agent) -> Self {
        Self::Snapshot(Snapshot {
            messages: agent.messages.clone(),
            total_tokens: agent.total_tokens,
        })
    }
}

/// Port of `Worker`.
pub struct Worker {
    pub started_at: Instant,
    handle: JoinHandle<()>,
}

impl Worker {
    /// Port of the `waitpid(WNOHANG)` check in `Clown.tick()`.
    pub fn finished(&self) -> bool {
        self.handle.is_finished()
    }
}

/// Spawn the agent loop. Port of `Clown.start()` (the fork half).
///
/// The returned receiver *is* the kill switch: dropping it disconnects
/// the channel and ends the loop on the next message.
pub fn spawn_worker(agent: Agent, runtime: AgentRuntime) -> (Worker, Receiver<WorkerMsg>) {
    let (tx, rx) = channel();
    let handle = std::thread::Builder::new()
        .name("clown-worker".into())
        .spawn(move || {
            // Never let a panic in the agent loop kill the process.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                worker_inner(agent, runtime, &tx);
            }));
        })
        .expect("failed to spawn worker thread");

    (
        Worker {
            started_at: Instant::now(),
            handle,
        },
        rx,
    )
}

/// Port of `Clown.workerInner()` — the loop the forked child ran.
fn worker_inner(mut agent: Agent, runtime: AgentRuntime, tx: &Sender<WorkerMsg>) {
    loop {
        let tcs = match agent.next(&runtime) {
            Ok(tcs) => tcs,
            Err(e) => {
                let _ = tx.send(WorkerMsg::Err(e.to_string()));
                return;
            }
        };
        match tcs {
            Some(tcs) => agent.accept_all(&runtime, &tcs),
            // `None` = final answer, no tool calls. The Zig `while`
            // exits here; one last snapshot goes out after the loop.
            None => break,
        }
        if tx.send(WorkerMsg::snapshot(&agent)).is_err() {
            // Receiver dropped — the worker was stopped.
            return;
        }
    }
    let _ = tx.send(WorkerMsg::snapshot(&agent));
}

/// An agent + runtime pointed at a closed localhost port, for tests.
pub fn dummy_agent_runtime() -> (Agent, AgentRuntime) {
    let config = Config {
        base_url: "http://127.0.0.1:9".into(),
        ..Default::default()
    };
    let toolbox = Arc::new({
        let mut tb = Toolbox::default();
        tools::register_all_tools(&mut tb);
        tb
    });
    let agent = Agent::new(Default::default());
    (agent, AgentRuntime::new(Client::new(&config), toolbox))
}

// =============================================================== clown

// The `Clown` — conversation state machine.
//
// Direct port of `Clown` in `src/model.zig` (see the worker section
// above for the fork→thread translation).

const COMPACT_PROMPT: &str = "Please provide a concise summary of the conversation so far.
Include:
- Current state of any ongoing tasks
- Key decisions and their rationale
- Important file paths and code snippets
- Anything that is absolutely neccessary in order to continue the work
Keep it under 2000 characters. After providing the summary, stop.";

pub struct Clown {
    runtime: AgentRuntime,
    /// Parent-side conversation state — updated only via worker snapshots.
    pub agent: Agent,
    worker: Option<Worker>,
    receiver: Option<Receiver<WorkerMsg>>,
    compacting: bool,
    /// Last worker error, surfaced in the TUI.
    pub err: Option<String>,
}

impl Clown {
    /// Port of `Clown.init()`: agent with all tools enabled, 32k max
    /// completion tokens, system prompt = PREFIX + AGENTS.md + date/cwd.
    pub fn new(config: &Config) -> io::Result<Self> {
        let mut toolbox = Toolbox::default();
        tools::register_all_tools(&mut toolbox);

        let runtime = AgentRuntime::new(Client::new(config), toolbox.into());

        let options = AgentOptions {
            model: "default".into(),
            max_completion_tokens: 32 * 1024,
            tools: runtime.toolbox().all_names(),
            ..Default::default()
        };

        let mut agent = Agent::new(options);
        agent.add_message(Message::system(
            prompt::load_system_prompt().map_err(io::Error::other)?,
        ));

        Ok(Self {
            runtime,
            agent,
            worker: None,
            receiver: None,
            compacting: false,
            err: None,
        })
    }

    // ------------------------------------------------------------- worker

    /// Port of `Clown.start()` — kill any running worker and spawn a new
    /// one with a clone of the current agent state.
    fn start(&mut self) {
        self.stop();
        self.err = None;
        let (worker, receiver) = spawn_worker(self.agent.clone(), self.runtime.clone());
        self.worker = Some(worker);
        self.receiver = Some(receiver);
    }

    /// Port of `Clown.stop()`. Dropping the receiver is the kill switch:
    /// the worker's next send fails and the loop exits.
    pub fn stop(&mut self) {
        self.worker = None;
        self.receiver = None;
    }

    pub fn busy(&self) -> bool {
        self.worker.is_some()
    }

    /// Seconds since the current worker started (0 when idle).
    pub fn elapsed(&self) -> u64 {
        self.worker
            .as_ref()
            .map(|w| w.started_at.elapsed().as_secs())
            .unwrap_or(0)
    }

    /// Port of `Clown.tick()` — call from the TUI on idle.
    ///
    /// Returns `true` when observable state changed (a worker message was
    /// applied or the worker finished), so the caller knows it must
    /// redraw. The tokamak original only emits `.render` on change; this
    /// flag is what lets the TUI mirror that instead of redrawing every
    /// poll tick.
    pub fn tick(&mut self) -> bool {
        // Drain all pending worker messages (into a local vec so we can
        // mutate self while the receiver is still borrowed).
        let pending: Vec<WorkerMsg> = self
            .receiver
            .as_ref()
            .map(|rx| {
                let mut v = Vec::new();
                while let Ok(m) = rx.try_recv() {
                    v.push(m);
                }
                v
            })
            .unwrap_or_default();
        let had_msgs = !pending.is_empty();
        for msg in pending {
            match msg {
                WorkerMsg::Snapshot(s) => self.load_snapshot(s),
                WorkerMsg::Err(e) => self.err = Some(e),
            }
        }

        // Worker finished.
        let mut finished = false;
        if let Some(worker) = &self.worker {
            if worker.finished() {
                self.worker = None;
                self.receiver = None;
                finished = true;

                if self.compacting {
                    let _ = self.finish_compact();
                }
            }
        }

        had_msgs || finished
    }

    // ---------------------------------------------------------- messages

    /// Port of `Clown.send()` — append the user message and run.
    pub fn send(&mut self, msg: impl Into<String>) {
        self.agent.add_message(Message::user(msg));
        self.start();
    }

    /// Port of `Clown.clear()` — keep only the system message.
    pub fn clear(&mut self) {
        self.stop();
        self.agent.messages.truncate(1);
    }

    /// Port of `Clown.clearTools()` — remove all tool messages, keep the rest.
    pub fn clear_tools(&mut self) {
        self.stop();
        self.agent.messages.retain(|m| m.role != Role::Tool);
    }

    /// Port of `Clown.compact()` — ask the model to summarize; the actual
    /// replacement happens in `finish_compact()` when the worker exits.
    pub fn compact(&mut self) {
        self.compacting = true;
        self.send(COMPACT_PROMPT);
    }

    /// Port of `Clown.finishCompact()`.
    fn finish_compact(&mut self) -> io::Result<()> {
        self.compacting = false;

        let last = match self.agent.messages.last() {
            Some(m) if m.role == Role::Assistant => m,
            _ => return Ok(()),
        };
        let summary = match last.content.as_ref().and_then(|c| c.text()) {
            Some(t) => t.to_string(),
            None => return Ok(()),
        };

        self.clear();
        self.send(format!(
            "The conversation history has been compacted to save context space. Acknowledge this and ask user what you want to do next.

<compacted summary>
{summary}
</compacted summary>"
        ));
        Ok(())
    }

    /// Port of `Clown.retry()` — drop the assistant's response (and tool
    /// results) and re-run with the same user message.
    pub fn retry(&mut self) {
        self.pop_trailing_assistant_and_tools();
        self.start();
    }

    /// Port of `Clown.undo()` + `Agent.undo()` — remove trailing
    /// assistant/tool messages and the preceding user message, copying it
    /// back into the input buffer (if it fits), to allow re-prompting.
    /// Returns the new input length (0 when there was nothing to undo).
    pub fn undo(&mut self, buf: &mut [u8]) -> usize {
        self.pop_trailing_assistant_and_tools();
        let Some(msg) = self.agent.messages.pop() else {
            return 0;
        };
        if let (true, Some(text)) = (
            msg.role == Role::User,
            msg.content.as_ref().and_then(|c| c.text()),
        ) {
            if text.len() < buf.len() {
                buf[..text.len()].copy_from_slice(text.as_bytes());
                return text.len();
            }
        }
        0
    }

    fn pop_trailing_assistant_and_tools(&mut self) {
        while let Some(msg) = self.agent.messages.last() {
            if msg.role != Role::Assistant && msg.role != Role::Tool {
                break;
            }
            self.agent.messages.pop();
        }
    }

    // ----------------------------------------------------------- sessions

    fn make_snapshot(&self) -> Snapshot {
        Snapshot {
            messages: self.agent.messages.clone(),
            total_tokens: self.agent.total_tokens,
        }
    }

    fn load_snapshot(&mut self, snap: Snapshot) {
        self.agent.messages = snap.messages;
        self.agent.total_tokens = snap.total_tokens;
    }

    /// Port of `Clown.save()`.
    pub fn save(&self) -> io::Result<()> {
        save_session(&self.make_snapshot())
    }

    /// Port of `Clown.load()`.
    pub fn load(&mut self, filename: &str) -> io::Result<()> {
        self.stop();
        let snap = load_session(filename)?;
        self.load_snapshot(snap);
        Ok(())
    }

    /// Port of `Clown.continue()` — load the most recent session, if any.
    pub fn continue_latest(&mut self) -> io::Result<()> {
        if let Some(snap) = continue_latest() {
            self.load_snapshot(snap);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clown_with_history() -> Clown {
        let mut c = Clown::new(&Config::default()).expect("clown init");
        // Replace the system prompt with a marker, add history.
        c.agent.messages = vec![
            Message::system("SYS"),
            Message::user("do the thing"),
            Message::assistant("on it"),
            Message::tool("File written successfully", "call_1"),
            Message::assistant("done!"),
        ];
        c
    }

    #[test]
    fn clear_keeps_only_system() {
        let mut c = clown_with_history();
        c.clear();
        assert_eq!(c.agent.messages.len(), 1);
        assert_eq!(c.agent.messages[0].role, Role::System);
    }

    #[test]
    fn clear_tools_removes_only_tool_messages() {
        let mut c = clown_with_history();
        c.clear_tools();
        assert_eq!(c.agent.messages.len(), 4);
        assert!(!c.agent.messages.iter().any(|m| m.role == Role::Tool));
    }

    #[test]
    fn undo_returns_last_user_message_to_buffer() {
        let mut c = clown_with_history();
        let mut buf = [0u8; 4096];
        let len = c.undo(&mut buf);
        assert_eq!(std::str::from_utf8(&buf[..len]).unwrap(), "do the thing");
        // history now ends at the system message
        assert_eq!(c.agent.messages.len(), 1);
    }

    #[test]
    fn retry_drops_trailing_assistant_and_tools() {
        let mut c = clown_with_history();
        // Don't actually start a worker in the unit test — just check the pop logic.
        c.pop_trailing_assistant_and_tools();
        assert_eq!(c.agent.messages.len(), 2);
        assert_eq!(c.agent.messages[1].role, Role::User);
    }

    /// Full-loop test against a closed port: send → worker starts →
    /// connection error comes back over the channel → tick surfaces it.
    #[test]
    fn worker_error_surfaces_in_tick() {
        let mut c = Clown::new(&Config {
            base_url: "http://127.0.0.1:9".into(),
            ..Default::default()
        })
        .expect("clown init");
        c.send("hello");
        assert!(c.busy());

        // Pump until the error arrives (worker thread hits ECONNREFUSED).
        for _ in 0..1000 {
            c.tick();
            if c.err.is_some() && !c.busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!c.busy());
        assert!(c.err.is_some(), "expected a worker error");
        println!("surfaced error: {}", c.err.unwrap());
    }

    // ------------------------------------------------- worker unit tests

    #[test]
    fn msg_json_shape_matches_zig() {
        let err: String = serde_json::to_string(&WorkerMsg::Err("Timeout".into())).unwrap();
        assert_eq!(err, r#"{"err":"Timeout"}"#);

        let snap = WorkerMsg::Snapshot(Snapshot {
            messages: vec![],
            total_tokens: 7,
        });
        assert_eq!(
            serde_json::to_string(&snap).unwrap(),
            r#"{"snapshot":{"messages":[],"total_tokens":7}}"#
        );
    }

    #[test]
    fn dropping_receiver_stops_worker() {
        let (agent, runtime) = dummy_agent_runtime();
        // Use a closed port so next() fails quickly.
        let (worker, rx) = spawn_worker(agent, runtime);
        drop(rx); // kill switch
                  // The thread exits once its in-flight request fails and the next
                  // send sees the disconnected channel.
        for _ in 0..1000 {
            if worker.finished() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            worker.finished(),
            "worker should exit once the receiver is dropped"
        );
    }
}
