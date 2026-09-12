//! The TUI — event loop, multi-line input buffer, and rendering.
//!
//! Port of `src/tui.zig` + tokamak's `textArea` control semantics.

use std::io;
use std::time::{Duration, Instant};

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
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

use crate::config::Config;
use crate::llm::Role;
use crate::model::Clown;

// ============================================================== input

// Multi-line input buffer.
//
// Port of tokamak's `editText`/`editTextArea` control semantics:
// byte-offset cursor, codepoint-unit backspace/delete/arrows,
// Home/Up → start, End/Down → end, bracketed paste inserts text
// (tabs become spaces). Enter does NOT insert a newline here — the
// main loop always captures it to send (as in the Zig TUI, where
// `textArea`'s enter handler is unreachable).

const MAX_LEN: usize = 4096; // bytes, as in the Zig `buf: [4096]u8`

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
            // Zig: .home, .up => cur = 0 / .end, .down => cur = len
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

/// How long to wait for an event before ticking/redrawing (Zig's `.idle`).
const POLL_TIMEOUT: Duration = Duration::from_millis(10);
/// Double Ctrl-C within this window exits; a single one stops the worker.
const CTRL_C_WINDOW: Duration = Duration::from_millis(500);
/// Double Esc within this window clears the input buffer.
const ESC_WINDOW: Duration = Duration::from_secs(2);

pub struct Tui {
    pub clown: Clown,
    input: Input,
    /// 0 = auto-scroll to bottom; > 0 = lines from the bottom.
    scroll: i32,
    last_esc: Option<Instant>,
    last_ctrl_c: Option<Instant>,
    /// Value of `clown.elapsed()` at the last draw (the "Processing… Ns"
    /// counter changes once a second while busy).
    last_elapsed: u64,
    /// Whether at least one frame has been drawn.
    drawn: bool,
}

pub fn run(config: &Config) -> io::Result<()> {
    let clown = Clown::new(config)?;
    let mut tui = Tui {
        clown,
        input: Input::default(),
        scroll: 0,
        last_esc: None,
        last_ctrl_c: None,
        last_elapsed: 0,
        drawn: false,
    };

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    // Original (tokamak `screen.zig`) enables mouse report modes
    // ?1000h ?1002h ?1003h ?1006h (buttons, drag, all motion, SGR) so
    // wheel events arrive as <64;...M / <65;...M. crossterm's
    // EnableMouseCapture enables exactly those (plus 1015).
    execute!(stdout, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let result = tui.run(&mut terminal);

    // Teardown mirrors the original `stop()`; ignore errors — the
    // terminal is already mid-shutdown if we got here via a panic.
    // (stdout was moved into the backend, so write via it.)
    let _ = execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
    terminal.show_cursor().ok();

    result
}

impl Tui {
    fn run(
        &mut self,
        terminal: &mut ratatui::Terminal<CrosstermBackend<io::Stdout>>,
    ) -> io::Result<()> {
        loop {
            // Zig: on `.idle` -> clown.tick().
            let ticked = self.clown.tick();

            // Zig: on `.key` / `.render`.
            let mut got_event = false;
            if event::poll(POLL_TIMEOUT)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if self.handle_key(key) {
                            break;
                        }
                    }
                    Event::Mouse(m) => match m.kind {
                        MouseEventKind::ScrollUp => self.scroll += 1,
                        MouseEventKind::ScrollDown => self.scroll = self.scroll.saturating_sub(1),
                        _ => {}
                    },
                    // Bracketed paste goes straight into the input box
                    // (Zig: `paste_start` → `pasteText`).
                    Event::Paste(text) => {
                        self.input.paste(&text);
                    }
                    _ => {}
                }
                got_event = true;
            }

            // Only redraw when something observable changed — mirrors
            // tokamak emitting `.render` on change rather than every
            // poll. The "Processing… Ns" counter ticks once a second
            // while a worker is busy, so redraw on each new second too.
            let elapsed = self.clown.elapsed();
            let changed = ticked
                || got_event
                || !self.drawn
                || (self.clown.busy() && elapsed != self.last_elapsed);
            if changed {
                self.drawn = true;
                self.last_elapsed = elapsed;
                terminal.draw(|f| draw(f, &self.clown, &self.input, self.scroll))?;
            }
        }
        Ok(())
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

                // Handle special commands.
                if let Some(rest) = input.strip_prefix('/') {
                    let (cmd, arg) = match rest.split_once(' ') {
                        Some((c, a)) => (c, a),
                        None => (rest, ""),
                    };
                    if !self.handle_command(cmd, arg) {
                        return true;
                    }
                } else {
                    self.clown.send(input);
                }
            }
            // Everything else goes to the input box (Zig: pending_key).
            _ => {
                self.input.handle_key(key);
            }
        }
        false
    }

    /// Port of `handleCommand` — returns false to exit.
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
                let mut buf = [0u8; 4096];
                let len = self.clown.undo(&mut buf);
                self.input
                    .set(std::str::from_utf8(&buf[..len]).unwrap_or(""));
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
}

// ============================================================ rendering

// Rendering — port of `Tui.render`/`messages`/`footer` in `src/tui.zig`.
//
// Nord theme (tokamak's default), same layout: message area fills the
// screen, an 8-row footer at the bottom (status row + 3-row input box).

/// Nord palette (tokamak's `Theme.nord`).
///
/// The original hardcodes `truecolor = false`, so it always emits
/// 256-color SGR (`38;5;N;48;5;N`) via `Color.to256()`. We match that
/// with `Color::Indexed` using the same indices the original computes —
/// terminals that don't honor 24-bit truecolor otherwise misrender the
/// background. The hex values are kept in the comments.
pub mod theme {
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
/// Tool results are clamped to 10 lines (Zig: `paragraph(..., if tool 10 else -1)`).
const TOOL_MAX_LINES: usize = 10;

/// Geometry inside the footer: 1 row top padding, status row at +1, a 1-row
/// spacing gap at +2, then the 3-row input box at +3 (Zig: footer pads
/// `.{1,2,1,2}` and the layout spacing is 1).
const INPUT_AREA_TOP: u16 = 3;
const INPUT_AREA_HEIGHT: u16 = INPUT_ROWS as u16;

/// Draw the whole screen.
fn draw(f: &mut Frame, clown: &Clown, input: &Input, scroll: i32) {
    let area = f.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    // The original fills the whole root frame with base1 before drawing
    // anything (`self.stack[0].frame.fill(.base1)`), so the 2-col side
    // margins of the message area are base1 too, not the terminal default.
    f.render_widget(
        Block::default().style(Style::default().bg(theme::BASE1)),
        area,
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(FOOTER_HEIGHT)])
        .split(area);

    messages(f, clown, chunks[0], scroll);
    footer(f, clown, input, chunks[1]);
}

fn role_style(role: Role) -> Color {
    // Zig: user => .accent, assistant => .text, tool => .secondary
    match role {
        Role::User => theme::ACCENT,
        Role::Tool => theme::SECONDARY,
        _ => theme::TEXT,
    }
}

/// Build every message as flat, pre-wrapped lines (the Zig `grid` of
/// paragraphs), then apply scrollback.
fn messages(f: &mut Frame, clown: &Clown, area: Rect, scroll: i32) {
    let width = (area.width.saturating_sub(4)).max(1) as usize; // pad 2/2

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
            let mut wrapped: Vec<Line<'static>> =
                wrap_text(text, width).into_iter().map(Line::from).collect();
            wrapped.truncate(max_lines);
            for l in wrapped.iter_mut() {
                l.style = Style::default().fg(color);
            }
            lines.append(&mut wrapped);
        }

        // Tool calls rendered as "name<20>arguments" (Zig: row(20, -1)).
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

    // Scrollback (Zig: `offset = (available - content) + scroll`, i.e.
    // scroll == 0 is pinned to the bottom, scroll > 0 shifts the window
    // UP by that many lines, hiding the last `scroll` lines).
    let available = area.height as i32;
    let total = lines.len() as i32;
    let start = (total - available - scroll).clamp(0, (total - available).max(0));

    let para = Paragraph::new(
        lines
            .iter()
            .skip(start as usize)
            .cloned()
            .collect::<Vec<Line>>(),
    )
    .block(Block::default().style(Style::default().bg(theme::BASE1)))
    .wrap(Wrap { trim: false });

    f.render_widget(
        para,
        Rect {
            x: area.x + 2,
            y: area.y,
            width: width as u16,
            height: area.height,
        },
    );
}

fn footer(f: &mut Frame, clown: &Clown, input: &Input, area: Rect) {
    // Whole footer is base2 (Zig: `p.frame.fill(.base2)`).
    f.render_widget(
        Block::default().style(Style::default().bg(theme::BASE2)),
        area,
    );

    // Status row (Zig: `row(&.{ -20, -10, -1 }`):
    //   "User:"  -> left, col 0, .text
    //   "Tokens:"-> starts 20 cols from the right, .secondary
    //   number   -> starts 10 cols from the right, .secondary
    // The footer is padded 2 left/right, so the inner width is width-4.
    let iw = (area.width as usize).saturating_sub(4).max(1);
    let num = clown.agent.total_tokens.to_string();
    let tokens_col = iw.saturating_sub(20);
    let num_col = iw.saturating_sub(10);

    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled(
        "User:".to_string(),
        Style::default().fg(theme::TEXT),
    ));
    if tokens_col > 5 {
        spans.push(Span::raw(" ".repeat(tokens_col - 5)));
    }
    spans.push(Span::styled(
        "Tokens:".to_string(),
        Style::default().fg(theme::SECONDARY),
    ));
    let after_tokens = tokens_col + 7;
    if num_col > after_tokens {
        spans.push(Span::raw(" ".repeat(num_col - after_tokens)));
    }
    spans.push(Span::styled(num, Style::default().fg(theme::SECONDARY)));
    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: iw as u16,
            height: 1,
        },
    );

    // 3-row input box, inset 2 cols each side, on base1
    // (Zig: `p.stack(3)` + `fill(.base1)` inside the padded footer).
    let box_w = iw as u16;
    let box_rect = Rect {
        x: area.x + 2,
        y: area.y + INPUT_AREA_TOP,
        width: box_w,
        height: INPUT_AREA_HEIGHT,
    };
    f.render_widget(
        Block::default().style(Style::default().bg(theme::BASE1)),
        box_rect,
    );

    // textArea: the buffer wrapped to the box width, clipped to 3 rows,
    // text in .primary (the box is the single control, always focused),
    // with a filled .primary cursor at the (col, line) position.
    let content = input.as_str();
    let cursor_byte = input.cursor();
    let wrapped_all = wrap_text(content, box_w as usize);
    let upto = &content[..cursor_byte];
    let wrapped_up = wrap_text(upto, box_w as usize);
    let cursor_line = wrapped_up.len().saturating_sub(1);
    let cursor_col = wrapped_up.last().map(|l| l.chars().count()).unwrap_or(0);

    for i in 0..INPUT_ROWS {
        let line = wrapped_all.get(i).cloned().unwrap_or_default();
        let mut line_spans: Vec<Span> = Vec::new();
        if i == cursor_line {
            let (left, right) = split_at_char(&line, cursor_col);
            line_spans.push(Span::styled(left, Style::default().fg(theme::PRIMARY)));
            line_spans.push(Span::styled(" ", Style::default().bg(theme::PRIMARY)));
            line_spans.push(Span::styled(right, Style::default().fg(theme::PRIMARY)));
        } else {
            line_spans.push(Span::styled(line, Style::default().fg(theme::PRIMARY)));
        }
        let w: usize = line_spans.iter().map(span_width).sum();
        if w < box_w as usize {
            line_spans.push(Span::raw(" ".repeat(box_w as usize - w)));
        }

        f.render_widget(
            Paragraph::new(Line::from(line_spans)),
            Rect {
                x: box_rect.x,
                y: box_rect.y + i as u16,
                width: box_w,
                height: 1,
            },
        );
    }
}

/// Split `s` after `col` characters into `(left, right)`, char-boundary safe.
fn split_at_char(s: &str, col: usize) -> (String, String) {
    let byte = s.char_indices().nth(col).map(|(i, _)| i).unwrap_or(s.len());
    (s[..byte].to_string(), s[byte..].to_string())
}

fn span_width(s: &Span) -> usize {
    s.content.chars().count()
}

/// Word wrap (port of tokamak's `util.wordWrap` behavior: greedy word
/// packing, hard-breaks for over-long tokens, `\n` always breaks).
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
