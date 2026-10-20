//! Runtime colour palette.
//!
//! Rather than hard-coding a dark GitHub palette, the theme is derived from the
//! terminal's own background colour (when it can be detected). The base
//! background and default text use the terminal defaults (`Color::Reset`) so
//! the UI blends into whatever colour scheme the terminal — e.g. an alacritty
//! theme — is using. Accent colours are the terminal's ANSI colours, and the
//! diff tints are blended from the detected background so they stay subtle on
//! both light and dark themes.

use std::sync::RwLock;

use ratatui::style::Color;

use crate::termbg::Mode;

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub mode: Mode,

    // Text and chrome.
    pub text: Color,
    pub dim: Color,
    pub faint: Color,
    pub border: Color,
    pub bg: Color,
    pub panel_bg: Color,

    // Accents (ANSI, so they follow the terminal palette).
    pub accent: Color,
    pub green: Color,
    pub red: Color,
    pub purple: Color,
    pub yellow: Color,

    // Diff tints.
    pub added_bg: Color,
    pub removed_bg: Color,
    pub added_cursor_bg: Color,
    pub removed_cursor_bg: Color,
    pub hunk_bg: Color,
    pub hunk_fg: Color,
    pub cursor_bg: Color,
    pub comment_bg: Color,
    pub selection_bg: Color,
    pub selection_added_bg: Color,
    pub selection_removed_bg: Color,

    pub syntax_theme: &'static str,
}

const GREEN_RGB: (u8, u8, u8) = (63, 185, 80);
const RED_RGB: (u8, u8, u8) = (248, 81, 73);
const ACCENT_RGB: (u8, u8, u8) = (88, 166, 255);

fn blend(base: (u8, u8, u8), other: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let mix = |a: u8, b: u8| {
        (a as f32 * (1.0 - t) + b as f32 * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    (
        mix(base.0, other.0),
        mix(base.1, other.1),
        mix(base.2, other.2),
    )
}

fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
}

impl Theme {
    pub fn new(mode: Mode, bg: Option<(u8, u8, u8)>) -> Self {
        let base = bg.unwrap_or(match mode {
            Mode::Dark => (13, 17, 23),
            Mode::Light => (255, 255, 255),
        });
        let contrast = match mode {
            Mode::Dark => (255, 255, 255),
            Mode::Light => (0, 0, 0),
        };

        let added = blend(base, GREEN_RGB, 0.16);
        let removed = blend(base, RED_RGB, 0.16);
        // The hunk header separates unrelated parts of the file, so give it a
        // clearly visible band rather than a barely-there tint.
        let hunk = blend(base, contrast, 0.18);
        let cursor = blend(base, contrast, 0.11);
        let selection = blend(base, ACCENT_RGB, 0.26);

        let (dim, faint, border) = match mode {
            Mode::Dark => (
                Color::Rgb(139, 148, 158),
                Color::Rgb(110, 118, 129),
                Color::Rgb(48, 54, 61),
            ),
            Mode::Light => (
                Color::Rgb(87, 96, 106),
                Color::Rgb(110, 118, 129),
                Color::Rgb(208, 215, 222),
            ),
        };
        let hunk_fg = match mode {
            Mode::Dark => Color::Rgb(121, 192, 255),
            Mode::Light => Color::Rgb(9, 105, 218),
        };

        Self {
            mode,
            text: Color::Reset,
            dim,
            faint,
            border,
            bg: Color::Reset,
            panel_bg: rgb(hunk),
            accent: Color::Blue,
            green: Color::Green,
            red: Color::Red,
            purple: Color::Magenta,
            yellow: Color::Yellow,
            added_bg: rgb(added),
            removed_bg: rgb(removed),
            added_cursor_bg: rgb(blend(added, contrast, 0.12)),
            removed_cursor_bg: rgb(blend(removed, contrast, 0.12)),
            hunk_bg: rgb(hunk),
            hunk_fg,
            cursor_bg: rgb(cursor),
            comment_bg: rgb(blend(base, contrast, 0.04)),
            selection_bg: rgb(selection),
            selection_added_bg: rgb(blend(base, GREEN_RGB, 0.32)),
            selection_removed_bg: rgb(blend(base, RED_RGB, 0.32)),
            syntax_theme: match mode {
                Mode::Dark => "base16-ocean.dark",
                Mode::Light => "InspiredGitHub",
            },
        }
    }
}

static CURRENT: RwLock<Option<Theme>> = RwLock::new(None);

/// Install the process-wide theme.
pub fn set(mode: Mode, bg: Option<(u8, u8, u8)>) {
    *CURRENT.write().expect("theme lock") = Some(Theme::new(mode, bg));
}

/// The active theme. Defaults to dark if [`set`] was never called.
pub fn t() -> Theme {
    CURRENT
        .read()
        .expect("theme lock")
        .unwrap_or_else(|| Theme::new(Mode::Dark, None))
}

/// Parse a `--theme` / `GHDIFF_THEME` value.
pub fn parse_mode(value: &str) -> Option<Option<Mode>> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Some(None),
        "dark" => Some(Some(Mode::Dark)),
        "light" => Some(Some(Mode::Light)),
        _ => None,
    }
}
