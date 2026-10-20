//! Small text helpers.

use unicode_width::UnicodeWidthStr;

/// Word-wrap `text` to at most `width` display columns. Explicit newlines are
/// preserved. Words longer than `width` are hard-split.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for raw_line in text.split('\n') {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut current = String::new();
        let mut current_w = 0usize;
        for word in line.split_inclusive(' ') {
            let word = word.trim_end_matches(' ');
            let word_w = UnicodeWidthStr::width(word);
            if current_w == 0 {
                if word_w <= width {
                    current.push_str(word);
                    current_w = word_w;
                } else {
                    // Hard-split an over-long word.
                    let mut chunk = String::new();
                    let mut chunk_w = 0;
                    for ch in word.chars() {
                        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                        if chunk_w + cw > width {
                            out.push(std::mem::take(&mut chunk));
                            chunk_w = 0;
                        }
                        chunk.push(ch);
                        chunk_w += cw;
                    }
                    current = chunk;
                    current_w = chunk_w;
                }
            } else if current_w + 1 + word_w <= width {
                current.push(' ');
                current.push_str(word);
                current_w += 1 + word_w;
            } else {
                out.push(std::mem::take(&mut current));
                if word_w <= width {
                    current.push_str(word);
                    current_w = word_w;
                } else {
                    let mut chunk = String::new();
                    let mut chunk_w = 0;
                    for ch in word.chars() {
                        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                        if chunk_w + cw > width {
                            out.push(std::mem::take(&mut chunk));
                            chunk_w = 0;
                        }
                        chunk.push(ch);
                        chunk_w += cw;
                    }
                    current = chunk;
                    current_w = chunk_w;
                }
            }
        }
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Render an ISO-8601 timestamp (`2026-10-16T09:25:31Z`) as a compact
/// `2026-10-16 09:25` string, falling back to the raw value.
pub fn short_time(iso: &str) -> String {
    if iso.len() >= 16 && iso.as_bytes().get(10) == Some(&b'T') {
        let date = &iso[..10];
        let time = &iso[11..16];
        format!("{date} {time}")
    } else {
        iso.to_string()
    }
}

/// Truncate a string to `width` display columns, appending `…` when clipped.
pub fn truncate(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in text.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > width.saturating_sub(1) {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}
