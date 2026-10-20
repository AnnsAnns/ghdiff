//! Syntax highlighting built on `syntect`.
//!
//! Only the foreground colour and font attributes are used; backgrounds are
//! intentionally ignored so that the diff's added/removed tint shows through.

use ratatui::style::{Color, Modifier, Style};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

pub struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

impl Highlighter {
    pub fn new(theme_name: &str) -> Self {
        let syntaxes = SyntaxSet::load_defaults_newlines();
        let themes = ThemeSet::load_defaults();
        let theme = themes
            .themes
            .get(theme_name)
            .or_else(|| themes.themes.get("base16-ocean.dark"))
            .or_else(|| themes.themes.values().next())
            .cloned()
            .expect("syntect ships at least one theme");
        Self { syntaxes, theme }
    }

    fn syntax_for(&self, path: &str) -> &SyntaxReference {
        if let Ok(Some(s)) = self.syntaxes.find_syntax_for_file(path) {
            return s;
        }
        let ext = path.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        self.syntaxes
            .find_syntax_by_extension(ext)
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text())
    }

    /// Highlight a run of logical lines. The result has one span list per
    /// input line, in the same order.
    pub fn highlight(&self, path: &str, lines: &[&str]) -> Vec<Vec<(Style, String)>> {
        let syntax = self.syntax_for(path);
        let mut hl = HighlightLines::new(syntax, &self.theme);
        let mut out = Vec::with_capacity(lines.len());
        for line in lines {
            let mut buf = String::with_capacity(line.len() + 1);
            buf.push_str(line);
            buf.push('\n');
            let spans = hl.highlight_line(&buf, &self.syntaxes).unwrap_or_default();
            let mut converted: Vec<(Style, String)> = spans
                .into_iter()
                .map(|(style, text)| (to_ratatui(style), text.to_string()))
                .collect();
            if let Some(last) = converted.last_mut() {
                while last.1.ends_with('\n') || last.1.ends_with('\r') {
                    last.1.pop();
                }
            }
            converted.retain(|(_, text)| !text.is_empty());
            out.push(converted);
        }
        out
    }
}

fn to_ratatui(style: syntect::highlighting::Style) -> Style {
    let mut out = Style::default().fg(Color::Rgb(
        style.foreground.r,
        style.foreground.g,
        style.foreground.b,
    ));
    if style.font_style.contains(FontStyle::BOLD) {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        out = out.add_modifier(Modifier::UNDERLINED);
    }
    out
}
