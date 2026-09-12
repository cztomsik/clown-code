//! Multi-line input buffer for the TUI footer.
//!
//! Byte-offset cursor, codepoint-unit backspace/delete/arrows,
//! Home/Up → start, End/Down → end, bracketed paste inserts text
//! (tabs become spaces). Enter does NOT insert a newline here — the
//! TUI's main loop always captures it to send.

use crossterm::event::{KeyCode, KeyEvent};

/// Multi-line input buffer with a byte-offset cursor.
#[derive(Default)]
pub struct Input {
    buf: String,
    cursor: usize, // byte offset, always on a char boundary
}

/// Byte offset of the start of the char immediately before `pos`
/// (0 when `pos` is 0).
fn char_start_before(buf: &str, pos: usize) -> usize {
    buf[..pos]
        .char_indices()
        .next_back()
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Byte length of the char at `pos` (0 when `pos` is at the end).
fn char_len_at(buf: &str, pos: usize) -> usize {
    buf[pos..].chars().next().map_or(0, |c| c.len_utf8())
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
        self.buf = text.to_string();
        self.cursor = self.buf.len();
    }

    /// Clear the buffer and return its contents.
    pub fn take(&mut self) -> String {
        let buf = std::mem::take(&mut self.buf);
        self.cursor = 0;
        buf
    }

    /// Handle a key not captured by the main loop. Returns true if the
    /// buffer contents changed.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(c) => {
                self.buf.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                true
            }
            KeyCode::Backspace => {
                let start = char_start_before(&self.buf, self.cursor);
                let changed = start != self.cursor;
                if changed {
                    self.buf.replace_range(start..self.cursor, "");
                    self.cursor = start;
                }
                changed
            }
            KeyCode::Delete => {
                let len = char_len_at(&self.buf, self.cursor);
                self.buf.replace_range(self.cursor..self.cursor + len, "");
                len != 0
            }
            KeyCode::Left => {
                self.cursor = char_start_before(&self.buf, self.cursor);
                false
            }
            KeyCode::Right => {
                self.cursor += char_len_at(&self.buf, self.cursor);
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

    /// Insert pasted text at the cursor: tabs become spaces, everything
    /// else (including newlines) is inserted verbatim.
    pub fn paste(&mut self, text: &str) {
        let insert: String = text
            .chars()
            .map(|c| if c == '\t' { ' ' } else { c })
            .collect();
        let at = self.cursor;
        self.buf.insert_str(at, &insert);
        self.cursor = at + insert.len();
    }
}
