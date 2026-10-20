//! A tiny multi-line text editor used for composing review comments.

/// Characters that make up a word for Ctrl+Left / Ctrl+Right navigation.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub struct Editor {
    rows: Vec<Vec<char>>,
    pub row: usize,
    pub col: usize,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    pub fn new() -> Self {
        Self {
            rows: vec![Vec::new()],
            row: 0,
            col: 0,
        }
    }

    /// Build an editor pre-filled with `text` (used for code suggestions).
    pub fn from_text(text: &str) -> Self {
        let rows: Vec<Vec<char>> = text.split('\n').map(|l| l.chars().collect()).collect();
        let rows = if rows.is_empty() {
            vec![Vec::new()]
        } else {
            rows
        };
        let row = rows.len() - 1;
        let col = rows[row].len();
        Self { rows, row, col }
    }

    pub fn text(&self) -> String {
        self.rows
            .iter()
            .map(|r| r.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn rows(&self) -> &[Vec<char>] {
        &self.rows
    }

    pub fn insert_char(&mut self, c: char) {
        self.rows[self.row].insert(self.col, c);
        self.col += 1;
    }

    pub fn insert_newline(&mut self) {
        let tail = self.rows[self.row].split_off(self.col);
        self.rows.insert(self.row + 1, tail);
        self.row += 1;
        self.col = 0;
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            self.col -= 1;
            self.rows[self.row].remove(self.col);
        } else if self.row > 0 {
            let current = self.rows.remove(self.row);
            self.row -= 1;
            self.col = self.rows[self.row].len();
            self.rows[self.row].extend(current);
        }
    }

    pub fn delete(&mut self) {
        if self.col < self.rows[self.row].len() {
            self.rows[self.row].remove(self.col);
        } else if self.row + 1 < self.rows.len() {
            let next = self.rows.remove(self.row + 1);
            self.rows[self.row].extend(next);
        }
    }

    pub fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.rows[self.row].len();
        }
    }

    pub fn right(&mut self) {
        if self.col < self.rows[self.row].len() {
            self.col += 1;
        } else if self.row + 1 < self.rows.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    /// Move to the start of the previous word (Ctrl+Left).
    pub fn word_left(&mut self) {
        if self.col == 0 {
            self.left();
            return;
        }
        let row = &self.rows[self.row];
        let mut i = self.col;
        while i > 0 && !is_word_char(row[i - 1]) {
            i -= 1;
        }
        while i > 0 && is_word_char(row[i - 1]) {
            i -= 1;
        }
        self.col = i;
    }

    /// Move past the end of the next word (Ctrl+Right).
    pub fn word_right(&mut self) {
        let len = self.rows[self.row].len();
        if self.col >= len {
            self.right();
            return;
        }
        let row = &self.rows[self.row];
        let mut i = self.col;
        while i < len && !is_word_char(row[i]) {
            i += 1;
        }
        while i < len && is_word_char(row[i]) {
            i += 1;
        }
        self.col = i;
    }

    pub fn up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.rows[self.row].len());
        }
    }

    pub fn down(&mut self) {
        if self.row + 1 < self.rows.len() {
            self.row += 1;
            self.col = self.col.min(self.rows[self.row].len());
        }
    }

    pub fn home(&mut self) {
        self.col = 0;
    }

    pub fn end(&mut self) {
        self.col = self.rows[self.row].len();
    }
}
