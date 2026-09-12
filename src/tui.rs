//! The TUI — event loop, multi-line input buffer, and rendering.
//!
//! The transcript is a single `Paragraph` with `Paragraph::scroll` for
//! scrollback, the footer is built with `Layout`, and the event loop is
//! the standard ratatui draw-then-poll pattern driven by a fixed tick rate.
//!
//! (ratatui 0.30 still ships no text-input widget and no stable wrapped
//! line-count API, so lines are pre-wrapped to the content width by
//! [`wrap_text`] — that keeps per-message line caps and the scroll offset
//! exact.)

use std::io;
use std::time::{Duration, Instant};

use crossterm::cursor::Show;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::config::Config;
use crate::llm::Role;
use crate::model::Clown;

// ============================================================== input

// Multi-line input buffer.
//
// Byte-offset cursor, codepoint-unit backspace/delete/arrows,
// Home/Up → start, End/Down → end, bracketed paste inserts text
// (tabs become spaces). Enter does NOT insert a newline here — the
// main loop always captures it to send.

const MAX_LEN: usize = 4096; // bytes

#[derive(Default)]
pub struct Input {
    buf: String,
    cursor: usize, // byte offset
}

impl Input {
    pub fn as_str(&self) -> &str {
        &self.buf
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Replace the buffer contents (used by `/undo`).
    pub fn set(&mut self, text: &str) {
        // Cap at MAX_LEN bytes, on a char boundary.
        let mut buf = text.to_string();
        if buf.len() > MAX_LEN {
            while buf.len() > MAX_LEN {
                buf.pop();
            }
        }
        self.cursor = buf.len();
        self.buf = buf;
    }

    pub fn take(&mut self) -> String {
        let buf = std::mem::take(&mut self.buf);
        self.cursor = 0;
        buf
    }

    /// Handle a key not captured by the main loop. Returns true if the
    /// buffer changed (triggering a redraw in the caller).
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(c) => self.insert_char(c),
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let start = self.buf[..self.cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap();
                    self.buf.replace_range(start..self.cursor, "");
                    self.cursor = start;
                }
                true
            }
            KeyCode::Delete => {
                if self.cursor < self.buf.len() {
                    let end = self.buf[self.cursor..]
                        .char_indices()
                        .next()
                        .map(|(i, _)| i + self.cursor)
                        .unwrap_or(self.buf.len());
                    self.buf.replace_range(self.cursor..end, "");
                }
                true
            }
            KeyCode::Left => {
                if self.cursor > 0 {
                    let start = self.buf[..self.cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap();
                    self.cursor = start;
                }
                false
            }
            KeyCode::Right => {
                if self.cursor < self.buf.len() {
                    let end = self.buf[self.cursor..]
                        .char_indices()
                        .next()
                        .map(|(i, _)| i + self.cursor)
                        .unwrap_or(self.buf.len());
                    self.cursor = end.min(self.buf.len());
                }
                false
            }
            KeyCode::Home | KeyCode::Up => {
                self.cursor = 0;
                false
            }
            KeyCode::End | KeyCode::Down => {
                self.cursor = self.buf.len();
                false
            }
            _ => false,
        }
    }

    /// Port of `pasteText` (preserve_newlines mode): tabs become spaces,
    /// everything else is inserted verbatim at the cursor.
    pub fn paste(&mut self, text: &str) {
        let mut insert: String = text
            .chars()
            .map(|c| if c == '\t' { ' ' } else { c })
            .collect();
        // Enforce the byte cap.
        while insert.len() + self.buf.len() > MAX_LEN {
            let chars: Vec<char> = insert.chars().collect();
            insert = chars[..chars.len() - 1].iter().collect();
        }
        let at = self.cursor;
        self.buf.insert_str(at, &insert);
        self.cursor = at + insert.len();
    }

    fn insert_char(&mut self, c: char) -> bool {
        if self.buf.len() + c.len_utf8() > MAX_LEN {
            return false;
        }
        self.buf.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        true
    }
}

// =============================================================== tui

/// Redraw at most this often — also bounds how fast the
/// "Processing… Ns" counter updates while a worker is busy.
const TICK_RATE: Duration = Duration::from_millis(250);
/// Double Ctrl-C within this window exits; a single one stops the worker.
const CTRL_C_WINDOW: Duration = Duration::from_millis(500);
/// Double Esc within this window clears the input buffer.
const ESC_WINDOW: Duration = Duration::from_secs(2);

/// Restores the terminal (mouse capture, alt screen, raw mode, cursor)
/// when the event loop ends — including when a panic unwinds through it.
struct TermGuard;

impl Drop for TermGuard {
    fn drop(&mut self) {
        // Ignore errors — the terminal is already mid-shutdown if we
        // got here via a panic.
        let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), Show);
    }
}

pub struct Tui {
    pub clown: Clown,
    input: Input,
    /// Lines scrolled back from the bottom of the transcript
    /// (0 = pinned to the newest line).
    scroll: u16,
    last_esc: Option<Instant>,
    last_ctrl_c: Option<Instant>,
}

pub fn run(config: &Config) -> io::Result<()> {
    let clown = Clown::new(config)?;
    let mut tui = Tui {
        clown,
        input: Input::default(),
        scroll: 0,
        last_esc: None,
        last_ctrl_c: None,
    };

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // EnterAlternateScreen + mouse report modes ?1000h ?1002h ?1003h
    // ?1006h (buttons, drag, all motion, SGR) so wheel events arrive.
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    tui.run(&mut terminal)
}

impl Tui {
    /// Draw-then-poll event loop. `event::poll` blocks until the next
    /// tick, so an idle screen redraws at most ~4 times per second.
    fn run(
        &mut self,
        terminal: &mut ratatui::Terminal<CrosstermBackend<io::Stdout>>,
    ) -> io::Result<()> {
        let _guard = TermGuard;
        let mut next_tick = Instant::now() + TICK_RATE;

        loop {
            terminal.draw(|f| self.render(f))?;

            let timeout = next_tick.saturating_duration_since(Instant::now());
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if self.handle_key(key) {
                            return Ok(());
                        }
                    }
                    Event::Mouse(m) => match m.kind {
                        MouseEventKind::ScrollUp => self.scroll = self.scroll.saturating_add(1),
                        MouseEventKind::ScrollDown => self.scroll = self.scroll.saturating_sub(1),
                        _ => {}
                    },
                    // Bracketed paste goes straight into the input box.
                    Event::Paste(text) => self.input.paste(&text),
                    _ => {}
                }
            }

            // Drain new worker messages into the conversation.
            let _ = self.clown.tick();

            // Advance the tick schedule (fast-forward after a stall).
            while Instant::now() >= next_tick {
                next_tick += TICK_RATE;
            }
        }
    }

    /// Returns true when the app should exit.
    fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let now = Instant::now();
                if self
                    .last_ctrl_c
                    .is_some_and(|t| now.duration_since(t) < CTRL_C_WINDOW)
                {
                    return true;
                }
                self.clown.stop();
                self.last_ctrl_c = Some(now);
            }
            KeyCode::Esc => {
                let now = Instant::now();
                if self
                    .last_esc
                    .is_some_and(|t| now.duration_since(t) < ESC_WINDOW)
                {
                    self.input.take();
                }
                self.last_esc = Some(now);
            }
            KeyCode::Enter => {
                let input = self.input.take();
                if input.is_empty() {
                    return false;
                }

                // Handle special commands. The argument is the entire
                // rest of the line (so filenames with spaces work).
                if let Some(rest) = input.strip_prefix('/') {
                    let (cmd, arg) = match rest.split_once(' ') {
                        Some((c, a)) => (c, a.trim_start()),
                        None => (rest, ""),
                    };
                    if !self.handle_command(cmd, arg) {
                        return true;
                    }
                } else {
                    self.clown.send(input);
                }
            }
            // Everything else goes to the input box.
            _ => {
                self.input.handle_key(key);
            }
        }
        false
    }

    /// Returns false to exit.
    fn handle_command(&mut self, cmd: &str, arg: &str) -> bool {
        match cmd {
            "exit" | "quit" => return false,
            "stop" => self.clown.stop(),
            "clear" => self.clown.clear(),
            "clear-tools" => self.clown.clear_tools(),
            "compact" => self.clown.compact(),
            "init" => self.clown.send("Could you /init this project?"),
            "retry" => self.clown.retry(),
            "undo" => {
                if let Some(text) = self.clown.undo() {
                    self.input.set(&text);
                }
            }
            "save" => {
                if let Err(e) = self.clown.save() {
                    tracing::error!("save failed: {e}");
                }
            }
            "load" => {
                if let Err(e) = self.clown.load(arg) {
                    tracing::error!("load failed: {e}");
                }
            }
            "continue" => {
                if let Err(e) = self.clown.continue_latest() {
                    tracing::error!("continue failed: {e}");
                }
            }
            _ => {}
        }
        true
    }

    // ------------------------------------------------------------ rendering

    /// Draw the whole screen: transcript on top, an 8-row footer below
    /// (status row + 3-row input box).
    fn render(&self, f: &mut ratatui::Frame) {
        let area = f.area();
        if area.width == 0 || area.height == 0 {
            return;
        }
        // Fill the whole frame with base1 first, so the 2-col side
        // margins of the message area are base1, not the terminal default.
        f.render_widget(
            Block::default().style(Style::default().bg(theme::BASE1)),
            area,
        );

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(FOOTER_HEIGHT)])
            .split(area);

        self.render_messages(f, chunks[0]);
        self.render_footer(f, chunks[1]);
    }

    /// The transcript: every non-system message as pre-wrapped lines,
    /// with the scrollback applied via `Paragraph::scroll`.
    fn render_messages(&self, f: &mut ratatui::Frame, area: Rect) {
        let width = area.width.saturating_sub(4).max(1); // 2-col margin each side
        let lines = build_message_lines(&self.clown, width as usize);

        // `scroll` is measured from the bottom; convert to a top offset.
        // (usize throughout: `saturating_sub` on a signed type only
        // saturates at the type's min, so it would not clamp to 0 here.)
        let max_offset = lines.len().saturating_sub(area.height as usize);
        let offset = max_offset.saturating_sub(self.scroll as usize);
        let offset = offset.min(u16::MAX as usize) as u16;

        let para = Paragraph::new(lines)
            .scroll((offset, 0))
            .block(Block::default().style(Style::default().bg(theme::BASE1)));
        f.render_widget(para, Rect::new(area.x + 2, area.y, width, area.height));
    }

    fn render_footer(&self, f: &mut ratatui::Frame, area: Rect) {
        f.render_widget(
            Block::default().style(Style::default().bg(theme::BASE2)),
            area,
        );

        // Footer geometry: 1 row padding, the status row, 1 row gap,
        // the 3-row input box, then the remaining rows as padding.
        let inner = Rect::new(
            area.x + 2,
            area.y,
            area.width.saturating_sub(4).max(1),
            area.height,
        );
        let [status, input_box] = {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(INPUT_ROWS as u16),
                    Constraint::Min(0),
                ])
                .split(inner);
            // rows: 1 top padding, status, 1 gap, input box, padding.
            [chunks[1], chunks[3]]
        };

        // Status row: "User:" left, "Tokens: N" right.
        let status_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(5),
                Constraint::Min(1),
                Constraint::Min(1),
            ])
            .split(status);
        f.render_widget(
            Paragraph::new(Line::from("User:").style(Style::default().fg(theme::TEXT))),
            status_chunks[0],
        );
        let token_spans = vec![
            Span::styled("Tokens:", Style::default().fg(theme::SECONDARY)),
            Span::raw(" "),
            Span::styled(
                self.clown.agent.total_tokens.to_string(),
                Style::default().fg(theme::SECONDARY),
            ),
        ];
        f.render_widget(
            Paragraph::new(Line::from(token_spans).right_aligned()),
            status_chunks[2],
        );

        // Input box: the buffer on base1, with a filled cursor span.
        f.render_widget(
            Block::default().style(Style::default().bg(theme::BASE1)),
            input_box,
        );
        f.render_widget(self.input_widget(input_box.width as usize), input_box);
    }

    /// The input buffer as `INPUT_ROWS` pre-wrapped lines, with a
    /// filled cursor span at the cursor position.
    fn input_widget(&self, width: usize) -> Paragraph<'_> {
        let content = self.input.as_str();
        let cursor_byte = self.input.cursor();
        let wrapped_all = wrap_text(content, width);
        let wrapped_up_to_cursor = wrap_text(&content[..cursor_byte], width);
        let cursor_line = wrapped_up_to_cursor.len().saturating_sub(1);
        let cursor_col = wrapped_up_to_cursor
            .last()
            .map(|l| l.chars().count())
            .unwrap_or(0);

        let mut rows = Vec::with_capacity(INPUT_ROWS);
        for i in 0..INPUT_ROWS {
            let line = wrapped_all.get(i).cloned().unwrap_or_default();
            let style = Style::default().fg(theme::PRIMARY);
            if i == cursor_line {
                let (left, right) = split_at_char(&line, cursor_col);
                rows.push(Line::from(vec![
                    Span::styled(left, style),
                    Span::raw(" ").style(Style::default().bg(theme::PRIMARY)),
                    Span::styled(right, style),
                ]));
            } else {
                rows.push(Line::from(line).style(style));
            }
        }
        Paragraph::new(rows)
    }
}

// ============================================================ rendering

// Rendering.
//
// Nord theme, layout: message area fills the screen, an 8-row footer
// at the bottom (status row + 3-row input box).

/// Nord palette.
///
/// Colors are 256-color indexed (SGR `38;5;N;48;5;N`) rather than 24-bit
/// truecolor — terminals that don't honor truecolor otherwise
/// misrender the background. The hex values are kept in the comments.
mod theme {
    use ratatui::style::Color;

    pub const TEXT: Color = Color::Indexed(255); // 0xECEFF4
    pub const BASE1: Color = Color::Indexed(237); // 0x2E3440
    pub const BASE2: Color = Color::Indexed(238); // 0x3B4252
    pub const PRIMARY: Color = Color::Indexed(110); // 0x88C0D0
    pub const SECONDARY: Color = Color::Indexed(109); // 0x81A1C1
    pub const ACCENT: Color = Color::Indexed(144); // 0xA3BE8C
}

const FOOTER_HEIGHT: u16 = 8;
const INPUT_ROWS: usize = 3;
/// Tool results are clamped to 10 lines.
const TOOL_MAX_LINES: usize = 10;

fn role_style(role: Role) -> Color {
    match role {
        Role::User => theme::ACCENT,
        Role::Tool => theme::SECONDARY,
        _ => theme::TEXT,
    }
}

/// Build every message as flat, pre-wrapped lines.
fn build_message_lines(clown: &Clown, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for msg in &clown.agent.messages {
        if msg.role == Role::System {
            continue;
        }
        let color = role_style(msg.role);
        let max_lines = if msg.role == Role::Tool {
            TOOL_MAX_LINES
        } else {
            usize::MAX
        };

        if let Some(text) = msg.content.as_ref().and_then(|c| c.text()) {
            let wrapped: Vec<String> = wrap_text(text, width);
            for l in wrapped.into_iter().take(max_lines) {
                lines.push(Line::from(l).style(Style::default().fg(color)));
            }
        }

        // Tool calls rendered as "name<20>arguments".
        if let Some(tcs) = &msg.tool_calls {
            for tc in tcs {
                lines.push(Line::from(Span::styled(
                    format!("{:<20}{}", tc.function.name, tc.function.arguments),
                    Style::default().fg(theme::SECONDARY),
                )));
            }
        }
    }

    if clown.busy() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("Processing... {}s", clown.elapsed()),
            Style::default().fg(theme::TEXT),
        )));
    }

    if let Some(e) = &clown.err {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("Error: {e}"),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
    }

    lines
}

/// Split `s` after `col` characters into `(left, right)`, char-boundary safe.
fn split_at_char(s: &str, col: usize) -> (String, String) {
    let byte = s.char_indices().nth(col).map(|(i, _)| i).unwrap_or(s.len());
    (s[..byte].to_string(), s[byte..].to_string())
}

/// Word wrap: greedy word packing, hard-breaks for over-long tokens,
/// `\n` always breaks.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out: Vec<String> = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        let mut cur_w = 0usize;

        for tok in para.split_inclusive(' ') {
            let tok_w = tok.chars().count();

            // Hard-break tokens longer than the width.
            if tok_w > width {
                if !cur.is_empty() {
                    cur.pop(); // drop trailing space
                    out.push(std::mem::take(&mut cur));
                }
                let mut rest = tok;
                while rest.chars().count() > width {
                    let cut: String = rest.chars().take(width).collect();
                    let tail = cut.len();
                    out.push(cut);
                    rest = &rest[tail..];
                }
                cur = rest.to_string();
                cur_w = cur.chars().count();
                continue;
            }

            // `cur_w` counts the real width of `cur` (trailing spaces
            // included — they take a column when more content follows),
            // but a token's own trailing space only matters if something
            // follows it, so fit is checked against the content width.
            let content_w = tok.trim_end().chars().count();
            if cur_w + content_w <= width {
                cur.push_str(tok);
                cur_w = cur.chars().count();
            } else {
                cur.pop(); // drop trailing space
                out.push(std::mem::take(&mut cur));
                cur.push_str(tok);
                cur_w = cur.chars().count();
            }
        }
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::wrap_text;

    #[test]
    fn wraps_words() {
        assert_eq!(wrap_text("hello world foo", 11), vec!["hello world", "foo"]);
        assert_eq!(wrap_text("a b c", 2), vec!["a", "b", "c"]);
        assert_eq!(wrap_text("line1\nline2", 5), vec!["line1", "line2"]);
        assert_eq!(wrap_text("", 5), vec![""]);
        // over-long token hard-breaks
        assert_eq!(wrap_text("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }
}
