//! The model — agent loop, session snapshots, worker thread, and the
//! `Clown` conversation state machine.
//!
//! The agent loop runs on a worker thread + mpsc so the TUI can
//! interrupt an in-flight LLM call; each new message is appended to the
//! parent's `agent` as it is produced.

use std::io;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Instant;

use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::llm::{Client, Message, Request, Response, Role, ToolCall, Toolbox};
use crate::{prompt, tools};

// =============================================================== agent

// Agent loop. One type holds the conversation, client, and toolbox:
// there is only ever one agent with fixed request parameters.

/// Request parameters (all fixed).
const MODEL: &str = "default";
const MAX_COMPLETION_TOKENS: u32 = 32 * 1024;
const AUTO_RETRY: u8 = 1;

#[derive(Clone)]
pub struct Agent {
    client: Client,
    toolbox: Toolbox,
    pub messages: Vec<Message>,
    pub total_tokens: u32,
}

impl Agent {
    pub fn new(client: Client, toolbox: Toolbox) -> Self {
        Self {
            client,
            toolbox,
            messages: Vec::new(),
            total_tokens: 0,
        }
    }

    pub fn add_message(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    /// Make one LLM call. Returns the assistant message's tool calls (if
    /// any). Retries on empty choices; appends the assistant message to
    /// the conversation.
    pub fn step(&mut self) -> Result<Option<Vec<ToolCall>>, String> {
        let mut retries = AUTO_RETRY + 1;
        while retries > 0 {
            retries -= 1;
            let res = self.create_completion()?;
            // Latest total-token count reported by the server.
            self.total_tokens = res.total_tokens();

            let Some(choice) = res.single_choice() else {
                return Err("NoChoice".into());
            };
            let is_empty = choice.text().is_none_or(str::is_empty);

            if choice.message.tool_calls.is_none() && is_empty {
                tracing::debug!("empty choice, retrying");
                continue;
            }

            self.add_message(choice.message.clone());
            return Ok(choice.message.tool_calls.clone());
        }
        Err("RetryFailed".into())
    }

    fn create_completion(&self) -> Result<Response, String> {
        let request = Request {
            model: MODEL.into(),
            messages: self.messages.clone(),
            tools: Some(self.toolbox.all_tools()),
            response_format: None,
            reasoning_effort: None,
            max_completion_tokens: MAX_COMPLETION_TOKENS,
            temperature: None,
            top_p: None,
        };

        self.client.create_chat_completion(&request)
    }

    /// Execute all tool calls and append tool result messages.
    /// Never fails — errors become the tool result string.
    pub fn accept_all(&mut self, tcs: &[ToolCall]) {
        for tc in tcs {
            self.add_message(Message::tool(
                self.toolbox.exec(&tc.function.name, &tc.function.arguments),
                tc.id.clone(),
            ));
        }
    }

    /// Remove trailing assistant/tool messages.
    pub fn pop_trailing_assistant_and_tools(&mut self) {
        while let Some(msg) = self.messages.last() {
            if msg.role != Role::Assistant && msg.role != Role::Tool {
                break;
            }
            self.messages.pop();
        }
    }

    /// Remove the trailing assistant/tool messages and, if a user message
    /// now trails, remove and return it (to allow re-prompting).
    pub fn undo(&mut self) -> Option<Message> {
        self.pop_trailing_assistant_and_tools();

        if let Some(msg) = self.messages.last() {
            if msg.role == Role::User {
                return self.messages.pop();
            }
        }
        None
    }
}

// ============================================================= session

/// Session save/load.
///
/// Files live in the current working directory. The JSON shape and key
/// order are a stable contract — old `session-*.json` files must stay
/// loadable.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub messages: Vec<Message>,
    pub total_tokens: u32,
}

/// `session-{YYYY-MM-DD HH:MM:SS UTC}.json`
pub fn session_filename() -> String {
    let now = Local::now();
    format!("session-{} UTC.json", now.format("%Y-%m-%d %H:%M:%S"))
}

/// Save the snapshot to a session file (2-space indent).
pub fn save_session(snap: &Snapshot) -> io::Result<()> {
    let json = serde_json::to_string_pretty(snap).expect("snapshot is serializable");
    std::fs::write(session_filename(), json)
}

/// Load a snapshot from a session file. Unknown fields are ignored.
pub fn load_session(filename: &str) -> io::Result<Snapshot> {
    let contents = std::fs::read_to_string(filename)?;
    let snap: Snapshot = serde_json::from_str(&contents)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(snap)
}

/// `/continue` — load the most recent session file, if any.
/// Uses a shell glob (`ls -1t session-*.json | head -1`).
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
// The agent loop runs on a worker thread so the TUI can interrupt an
// in-flight LLM call:
//
// - the loop runs on a dedicated OS thread (blocking reqwest),
// - each new message is sent over an `mpsc` channel as it is created,
// - stopping the worker drops the receiver; the thread's next
//   `send()` fails and the loop exits. A thread blocked in an HTTP
//   call finishes it and then bails — the server may keep generating,
//   but we ignore the result.

/// One unit of progress from the worker. `std::mpsc` is reliable and
/// ordered, so the parent can append every message exactly once.
enum WorkerMsg {
    Message(Message),
    Tokens(u32),
    Err(String),
}

/// Spawn the agent loop.
///
/// The returned receiver *is* the kill switch: dropping it disconnects
/// the channel and ends the loop on the next message.
fn spawn_worker(agent: Agent) -> (JoinHandle<()>, Receiver<WorkerMsg>) {
    let (tx, rx) = channel();
    let handle = std::thread::Builder::new()
        .name("clown-worker".into())
        .spawn(move || {
            // Never let a panic in the agent loop kill the process.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                worker_inner(agent, &tx);
            }));
        })
        .expect("failed to spawn worker thread");

    (handle, rx)
}

/// The agent loop that runs on the worker thread.
fn worker_inner(mut agent: Agent, tx: &Sender<WorkerMsg>) {
    loop {
        let before = agent.messages.len();
        let tcs = match agent.step() {
            Ok(tcs) => tcs,
            Err(e) => {
                let _ = tx.send(WorkerMsg::Err(e));
                return;
            }
        };
        // The assistant message, as it was appended.
        if !send_new(&agent, before, tx) {
            return;
        }
        let Some(tcs) = tcs else {
            // Final answer, no tool calls — done.
            return;
        };
        // The tool results, as they are appended.
        agent.accept_all(&tcs);
        if !send_new(&agent, before, tx) {
            return;
        }
    }
}

/// Send every message in `agent.messages[from..]` plus the latest
/// token count. Returns false when the receiver has been dropped.
fn send_new(agent: &Agent, from: usize, tx: &Sender<WorkerMsg>) -> bool {
    for msg in &agent.messages[from..] {
        if tx.send(WorkerMsg::Message(msg.clone())).is_err() {
            return false;
        }
    }
    tx.send(WorkerMsg::Tokens(agent.total_tokens)).is_ok()
}

/// An agent pointed at a closed localhost port, for tests.
#[cfg(test)]
pub fn dummy_agent() -> Agent {
    let config = Config {
        base_url: "http://127.0.0.1:9".into(),
        ..Default::default()
    };
    let mut toolbox = Toolbox::default();
    tools::register_all_tools(&mut toolbox);
    Agent::new(Client::new(&config), toolbox)
}

// =============================================================== clown

// The `Clown` — conversation state machine.

const COMPACT_PROMPT: &str = "Please provide a concise summary of the conversation so far.
Include:
- Current state of any ongoing tasks
- Key decisions and their rationale
- Important file paths and code snippets
- Anything that is absolutely neccessary in order to continue the work
Keep it under 2000 characters. After providing the summary, stop.";

/// A running worker: the thread handle, its start time, and the
/// receiver. Dropping the receiver is the kill switch — the worker's
/// next send fails and the loop exits.
struct Worker {
    handle: JoinHandle<()>,
    started_at: Instant,
    receiver: Receiver<WorkerMsg>,
}

pub struct Clown {
    /// Parent-side conversation state — worker messages are appended to
    /// it as they arrive.
    pub agent: Agent,
    worker: Option<Worker>,
    compacting: bool,
    /// Last worker error, surfaced in the TUI.
    pub err: Option<String>,
}

impl Clown {
    /// Agent with all tools enabled, system prompt = PREFIX + AGENTS.md
    /// + date/cwd.
    pub fn new(config: &Config) -> io::Result<Self> {
        let mut toolbox = Toolbox::default();
        tools::register_all_tools(&mut toolbox);

        let mut agent = Agent::new(Client::new(config), toolbox);
        agent.add_message(Message::system(
            prompt::load_system_prompt().map_err(io::Error::other)?,
        ));

        Ok(Self {
            agent,
            worker: None,
            compacting: false,
            err: None,
        })
    }

    // ------------------------------------------------------------- worker

    /// Kill any running worker and spawn a new one with a clone of the
    /// current agent state.
    fn start(&mut self) {
        self.stop();
        self.err = None;
        let (handle, receiver) = spawn_worker(self.agent.clone());
        self.worker = Some(Worker {
            handle,
            started_at: Instant::now(),
            receiver,
        });
    }

    /// Kill any running worker.
    pub fn stop(&mut self) {
        self.worker = None;
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

    /// Call from the TUI on idle.
    ///
    /// Returns `true` when observable state changed (a worker message was
    /// applied or the worker finished), so the TUI can skip redraws when
    /// nothing happened.
    pub fn tick(&mut self) -> bool {
        // Drain all pending worker messages (into a local vec so we can
        // mutate self while the receiver is still borrowed).
        let pending: Vec<WorkerMsg> = self
            .worker
            .as_ref()
            .map(|w| {
                let mut v = Vec::new();
                while let Ok(m) = w.receiver.try_recv() {
                    v.push(m);
                }
                v
            })
            .unwrap_or_default();
        let had_msgs = !pending.is_empty();
        for msg in pending {
            match msg {
                WorkerMsg::Message(m) => self.agent.add_message(m),
                WorkerMsg::Tokens(t) => self.agent.total_tokens = t,
                WorkerMsg::Err(e) => self.err = Some(e),
            }
        }

        // Worker finished.
        let mut finished = false;
        if self.worker.as_ref().is_some_and(|w| w.handle.is_finished()) {
            self.stop();
            finished = true;

            if self.compacting {
                let _ = self.finish_compact();
            }
        }

        had_msgs || finished
    }

    // ---------------------------------------------------------- messages

    /// Append the user message and run.
    pub fn send(&mut self, msg: impl Into<String>) {
        self.agent.add_message(Message::user(msg));
        self.start();
    }

    /// Keep only the system message.
    pub fn clear(&mut self) {
        self.stop();
        self.agent.messages.truncate(1);
    }

    /// Remove all tool messages, keep the rest.
    pub fn clear_tools(&mut self) {
        self.stop();
        self.agent.messages.retain(|m| m.role != Role::Tool);
    }

    /// Ask the model to summarize; the actual replacement happens in
    /// `finish_compact()` when the worker exits.
    pub fn compact(&mut self) {
        self.compacting = true;
        self.send(COMPACT_PROMPT);
    }

    /// Complete an in-progress compact once the worker has exited.
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

    /// Drop the assistant's response (and tool results) and re-run with
    /// the same user message.
    pub fn retry(&mut self) {
        self.agent.pop_trailing_assistant_and_tools();
        self.start();
    }

    /// Remove trailing assistant/tool messages and the preceding user
    /// message, returning its text (if any) so the TUI can copy it back
    /// into the input buffer for re-prompting.
    /// Returns None when there was nothing to undo.
    pub fn undo(&mut self) -> Option<String> {
        let msg = self.agent.undo()?;
        if msg.role == Role::User {
            return Some(
                msg.content
                    .as_ref()
                    .and_then(|c| c.text())
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        None
    }

    // ----------------------------------------------------------- sessions

    fn apply_snapshot(&mut self, snap: Snapshot) {
        self.agent.messages = snap.messages;
        self.agent.total_tokens = snap.total_tokens;
    }

    /// Save the current conversation to a session file.
    pub fn save(&self) -> io::Result<()> {
        let snap = Snapshot {
            messages: self.agent.messages.clone(),
            total_tokens: self.agent.total_tokens,
        };
        save_session(&snap)
    }

    /// Load a conversation from a session file.
    pub fn load(&mut self, filename: &str) -> io::Result<()> {
        self.stop();
        let snap = load_session(filename)?;
        self.apply_snapshot(snap);
        Ok(())
    }

    /// Load the most recent session, if any.
    pub fn continue_latest(&mut self) -> io::Result<()> {
        if let Some(snap) = continue_latest() {
            self.apply_snapshot(snap);
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
    fn undo_returns_last_user_message_text() {
        let mut c = clown_with_history();
        assert_eq!(c.undo().as_deref(), Some("do the thing"));
        // history now ends at the system message
        assert_eq!(c.agent.messages.len(), 1);
        // nothing left to undo
        assert_eq!(c.undo(), None);
    }

    #[test]
    fn retry_drops_trailing_assistant_and_tools() {
        let mut c = clown_with_history();
        // Don't actually start a worker in the unit test — just check the pop logic.
        c.agent.pop_trailing_assistant_and_tools();
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
    fn dropping_receiver_stops_worker() {
        let agent = dummy_agent();
        // Use a closed port so step() fails quickly.
        let (worker, rx) = spawn_worker(agent);
        drop(rx); // kill switch
                  // The thread exits once its in-flight request fails and the next
                  // send sees the disconnected channel.
        for _ in 0..1000 {
            if worker.is_finished() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            worker.is_finished(),
            "worker should exit once the receiver is dropped"
        );
    }
}
