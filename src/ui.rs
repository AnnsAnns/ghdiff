//! Rendering.

use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, Paragraph};

use crate::app::{App, DraftTarget, Focus, LineMeta, Screen};
use crate::editor::Editor;
use crate::model::{FileStatus, LineKind};
use crate::theme;
use crate::util;

pub fn draw(frame: &mut Frame, app: &mut App) {
    match app.screen {
        Screen::Loading => draw_loading(frame, app),
        Screen::Error => draw_error(frame, app),
        Screen::Picker => draw_picker(frame, app),
        Screen::Main => draw_main(frame, app),
    }
}

/// Render a frame into an in-memory buffer and return it as plain text.
/// Used by `--dump` and by the tests.
pub fn render_to_string(app: &mut App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend");
    terminal.draw(|frame| draw(frame, app)).expect("draw");
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn draw_loading(frame: &mut Frame, app: &App) {
    let area = centered_fixed(frame.area(), 60, 3);
    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            app.loading_msg.clone(),
            Style::default().fg(theme::t().accent),
        )),
        Line::from(Span::styled(
            "press q to quit",
            Style::default().fg(theme::t().faint),
        )),
    ];
    frame.render_widget(Paragraph::new(text).centered(), area);
}

fn draw_error(frame: &mut Frame, app: &App) {
    let outer = frame.area();
    let wrap_w = outer.width.saturating_sub(6).clamp(20, 76) as usize;
    let body: Vec<String> = app
        .error
        .lines()
        .flat_map(|l| util::wrap_text(l, wrap_w))
        .collect();
    let height = (body.len() as u16 + 4).min(outer.height);
    let area = centered_fixed(outer, wrap_w as u16 + 2, height);
    let block = Block::bordered()
        .border_style(Style::default().fg(theme::t().red))
        .title(Span::styled(" error ", Style::default().fg(theme::t().red)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines: Vec<Line> = body
        .into_iter()
        .map(|l| Line::from(Span::styled(l, Style::default().fg(theme::t().text))))
        .chain(std::iter::once(Line::from("")))
        .chain(std::iter::once(Line::from(Span::styled(
            "press any key to exit",
            Style::default().fg(theme::t().faint),
        ))))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_picker(frame: &mut Frame, app: &mut App) {
    let area = centered(frame.area(), 90, 80);
    let block = Block::bordered()
        .border_style(Style::default().fg(theme::t().border))
        .title(Span::styled(
            format!(" open pull requests · {} ", app.repo),
            Style::default()
                .fg(theme::t().accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(
            " j/k move · enter open · q quit ",
            Style::default().fg(theme::t().faint),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = inner.width as usize;
    let items: Vec<ListItem> = app
        .pr_list
        .iter()
        .map(|pr| {
            let draft = if pr.is_draft { " [draft]" } else { "" };
            let head = format!(" #{:<6}", pr.number);
            let text = format!(
                "{head}{}{draft}  ·  @{}  ·  {}",
                pr.title,
                pr.author_login(),
                pr.head_ref
            );
            ListItem::new(Line::from(Span::styled(
                util::truncate(&text, width.saturating_sub(2)),
                Style::default().fg(theme::t().text),
            )))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(theme::t().panel_bg)
                .fg(theme::t().accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    frame.render_stateful_widget(list, inner, &mut app.picker_state);
}

fn draw_main(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(frame, app, chunks[0]);

    let body = chunks[1];
    let sidebar_w = (area.width / 3).clamp(26, 46);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(sidebar_w), Constraint::Min(20)])
        .split(body);

    app.sidebar_area = cols[0];
    draw_sidebar(frame, app, cols[0]);

    // Diff pane: a one-line file header, then the content.
    let diff_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(cols[1]);
    draw_file_header(frame, app, diff_rows[0]);

    let content = diff_rows[1];
    app.diff_area = content;
    app.view.width = content.width;
    app.view.height = content.height;
    app.ensure_view();
    draw_diff(frame, app, content);

    draw_footer(frame, app, chunks[2]);

    if let Some(draft) = &app.draft {
        draw_draft(frame, app, draft);
    }
    if let Some(review) = &app.review_draft {
        draw_review(frame, review, app.pending_count());
    }
    if app.help {
        draw_help(frame, area);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered()
        .border_style(Style::default().fg(theme::t().border))
        .title(Span::styled(
            if app.since_review {
                " ghdiff · since last review "
            } else {
                " ghdiff "
            },
            Style::default()
                .fg(theme::t().accent)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (title, meta) = if let Some(m) = &app.meta {
        let title = format!("#{} {}", m.number, m.title);
        let pending = app.pending_count();
        let draft = if pending > 0 {
            format!(
                "  ·  {pending} draft comment{}",
                if pending == 1 { "" } else { "s" }
            )
        } else {
            String::new()
        };
        let meta = format!(
            "@{}  ·  {} ← {}  ·  {} files  +{} -{}  ·  {} unresolved comment{}{}",
            m.author_login(),
            m.base_ref,
            m.head_ref,
            m.changed_files,
            m.additions,
            m.deletions,
            app.unresolved_count(),
            if app.unresolved_count() == 1 { "" } else { "s" },
            draft,
        );
        (title, meta)
    } else {
        ("(no pull request)".to_string(), String::new())
    };

    let lines = vec![
        Line::from(Span::styled(
            util::truncate(&title, inner.width as usize),
            Style::default()
                .fg(theme::t().text)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            util::truncate(&meta, inner.width as usize),
            Style::default().fg(theme::t().dim),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner_w = area.width.saturating_sub(2);
    let text_w = inner_w.saturating_sub(2) as usize;

    let items: Vec<ListItem> = app
        .files
        .iter()
        .map(|f| {
            let (letter, color) = status_glyph(f.status);
            let count = app.file_thread_count(&f.path);
            let stats = format!(" +{} -{}", f.additions, f.deletions);
            let count_str = if count > 0 {
                format!(" ●{count}")
            } else {
                String::new()
            };
            let fixed = 2
                + unicode_width::UnicodeWidthStr::width(count_str.as_str())
                + unicode_width::UnicodeWidthStr::width(stats.as_str());
            let avail = text_w.saturating_sub(fixed);
            let full = f.path.as_str();
            let (dir_span, name_span) = if unicode_width::UnicodeWidthStr::width(full) <= avail {
                (f.dir_name().to_string(), f.file_name().to_string())
            } else {
                (String::new(), util::truncate(full, avail))
            };
            let used = unicode_width::UnicodeWidthStr::width(dir_span.as_str())
                + unicode_width::UnicodeWidthStr::width(name_span.as_str());
            let pad = avail.saturating_sub(used);

            let mut spans = vec![
                Span::styled(
                    format!("{letter} "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(dir_span, Style::default().fg(theme::t().faint)),
                Span::styled(name_span, Style::default().fg(theme::t().text)),
            ];
            if !count_str.is_empty() {
                spans.push(Span::styled(
                    count_str,
                    Style::default().fg(theme::t().purple),
                ));
            }
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(stats, Style::default().fg(theme::t().faint)));
            ListItem::new(Line::from(spans))
        })
        .collect();

    let block = Block::bordered()
        .border_style(Style::default().fg(theme::t().border))
        .title(Span::styled(
            format!(" {} changed files ", app.files.len()),
            Style::default().fg(theme::t().dim),
        ));

    let focused = app.focus == Focus::Files;
    let highlight = if focused {
        Style::default()
            .bg(theme::t().panel_bg)
            .fg(theme::t().accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(theme::t().panel_bg)
    };

    let list = List::new(items)
        .block(block)
        .highlight_style(highlight)
        .highlight_symbol("▸ ");
    frame.render_stateful_widget(list, area, &mut app.file_list_state);
}

fn draw_file_header(frame: &mut Frame, app: &App, area: Rect) {
    let text = if let Some(f) = app.files.get(app.file_index) {
        let (letter, color) = status_glyph(f.status);
        Line::from(vec![
            Span::styled(
                format!(" {letter} "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                f.path.clone(),
                Style::default()
                    .fg(theme::t().text)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("   +{} -{}", f.additions, f.deletions),
                Style::default().fg(theme::t().faint),
            ),
        ])
    } else {
        Line::from(Span::styled(
            " no changed files",
            Style::default().fg(theme::t().faint),
        ))
    };
    frame.render_widget(Paragraph::new(text), area);
}

fn draw_diff(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let buf: &mut Buffer = frame.buffer_mut();
    let scroll = app.view.scroll as usize;
    let cursor = app.view.cursor;

    // Which rows are part of the current visual selection?
    let mut selected = vec![false; app.view.meta.len()];
    if let Some(sel) = app.selected_range() {
        for r in sel.rows {
            if r < selected.len() {
                selected[r] = true;
            }
        }
    }

    // Fill the pane background first.
    buf.set_style(area, Style::default().bg(theme::t().bg));

    for y in 0..area.height {
        let idx = scroll + y as usize;
        let row = Rect::new(area.x, area.y + y, area.width, 1);
        let is_cursor = idx == cursor;
        let is_selected = selected.get(idx).copied().unwrap_or(false);
        let bg = row_bg(app.view.meta.get(idx), is_cursor, is_selected);
        buf.set_style(row, Style::default().bg(bg));
        if let Some(line) = app.view.lines.get(idx) {
            buf.set_line(area.x, area.y + y, line, area.width);
        }
        if is_cursor && let Some(cell) = buf.cell_mut((area.x, area.y + y)) {
            cell.set_symbol("▸").set_style(
                Style::default()
                    .fg(theme::t().accent)
                    .add_modifier(Modifier::BOLD),
            );
        }
    }
}

fn row_bg(meta: Option<&LineMeta>, is_cursor: bool, is_selected: bool) -> ratatui::style::Color {
    match meta {
        Some(LineMeta::Hunk) => {
            if is_selected {
                theme::t().selection_bg
            } else {
                theme::t().hunk_bg
            }
        }
        Some(LineMeta::Comment { .. }) => theme::t().comment_bg,
        Some(LineMeta::Code { kind, .. }) => match kind {
            LineKind::Added => {
                if is_selected {
                    theme::t().selection_added_bg
                } else if is_cursor {
                    theme::t().added_cursor_bg
                } else {
                    theme::t().added_bg
                }
            }
            LineKind::Removed => {
                if is_selected {
                    theme::t().selection_removed_bg
                } else if is_cursor {
                    theme::t().removed_cursor_bg
                } else {
                    theme::t().removed_bg
                }
            }
            LineKind::Context => {
                if is_selected {
                    theme::t().selection_bg
                } else if is_cursor {
                    theme::t().cursor_bg
                } else {
                    theme::t().bg
                }
            }
        },
        _ => {
            if is_selected {
                theme::t().selection_bg
            } else if is_cursor {
                theme::t().cursor_bg
            } else {
                theme::t().bg
            }
        }
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hints = if app.draft.is_some() {
        " Ctrl+S save · Esc cancel · Enter newline"
    } else {
        " Tab focus · j/k move · c comment · s suggest · v range · d discard · S review · L since · e editor · T terminal · ? help · q quit"
    };
    let line = if !app.status.is_empty() {
        Line::from(vec![
            Span::styled(
                format!(" {} ", app.status),
                Style::default().fg(theme::t().green),
            ),
            Span::styled(hints, Style::default().fg(theme::t().faint)),
        ])
    } else {
        Line::from(Span::styled(hints, Style::default().fg(theme::t().faint)))
    };

    // Right-aligned readout of the line under the cursor, VS Code style.
    let info = line_info(app);
    let info_w = unicode_width::UnicodeWidthStr::width(info.as_str()) as u16;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(info_w)])
        .split(area);
    frame.render_widget(Paragraph::new(line), cols[0]);
    if info_w > 0 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                info,
                Style::default().fg(theme::t().faint),
            ))),
            cols[1],
        );
    }
}

/// `Ln 42 · 87 chars` for the code line under the cursor.
fn line_info(app: &App) -> String {
    let Some(LineMeta::Code { hunk, line, .. }) = app.view.meta.get(app.view.cursor) else {
        return String::new();
    };
    let Some(file) = app.files.get(app.file_index) else {
        return String::new();
    };
    let Some(dl) = file.hunks.get(*hunk).and_then(|h| h.lines.get(*line)) else {
        return String::new();
    };
    let lineno = dl.new_lineno.or(dl.old_lineno).unwrap_or(0);
    let len = dl.content.chars().count();
    let cols = unicode_width::UnicodeWidthStr::width(dl.content.as_str());
    let label = if len == cols {
        format!("{len} chars")
    } else {
        format!("{len} chars · {cols} cols")
    };
    format!(" Ln {lineno} · {label} ")
}

/// The soft-wrapped rendering of an editor, plus where the cursor lands.
struct EditorLayout {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
}

fn char_width(c: char) -> usize {
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Soft-wrap every logical row of `editor` to `width` display columns.
fn wrap_editor(editor: &Editor, width: usize) -> EditorLayout {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut cursor_line = 0usize;
    let mut cursor_col = 0usize;

    for (ri, row) in editor.rows().iter().enumerate() {
        // Split the logical row into display-width segments.
        let mut segments: Vec<(usize, String)> = Vec::new();
        let mut start = 0usize;
        let mut current = String::new();
        let mut w = 0usize;
        for (ci, &ch) in row.iter().enumerate() {
            let cw = char_width(ch);
            if w + cw > width && !current.is_empty() {
                segments.push((start, std::mem::take(&mut current)));
                start = ci;
                w = 0;
            }
            current.push(ch);
            w += cw;
        }
        segments.push((start, current));

        // A full last segment needs a trailing empty one for an end-of-line
        // cursor to sit on.
        if ri == editor.row && editor.col == row.len() {
            let last_w: usize = segments
                .last()
                .map(|(_, s)| s.chars().map(char_width).sum())
                .unwrap_or(0);
            if last_w >= width {
                segments.push((row.len(), String::new()));
            }
        }

        if ri == editor.row {
            let last_idx = segments.len() - 1;
            for (si, (seg_start, seg)) in segments.iter().enumerate() {
                let seg_end = seg_start + seg.chars().count();
                if editor.col < seg_end || si == last_idx {
                    cursor_line = lines.len() + si;
                    let upto = editor
                        .col
                        .saturating_sub(*seg_start)
                        .min(seg.chars().count());
                    cursor_col = seg.chars().take(upto).map(char_width).sum();
                    break;
                }
            }
        }

        for (_, seg) in segments {
            lines.push(seg);
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    EditorLayout {
        lines,
        cursor_line,
        cursor_col,
    }
}

/// Render a soft-wrapped editor into `rect`, following the cursor.
fn draw_editor(
    frame: &mut Frame,
    editor: &Editor,
    rect: Rect,
    layout: &EditorLayout,
    show_cursor: bool,
) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let visible_h = rect.height as usize;
    let offset = if layout.cursor_line >= visible_h {
        layout.cursor_line + 1 - visible_h
    } else {
        0
    };
    let rendered: Vec<Line> = (0..visible_h)
        .map(|i| match layout.lines.get(offset + i) {
            Some(text) => Line::from(Span::styled(
                text.clone(),
                Style::default().fg(theme::t().text),
            )),
            None => Line::from(""),
        })
        .collect();
    frame.render_widget(Paragraph::new(rendered), rect);

    if !show_cursor {
        return;
    }
    let cy = rect.y + (layout.cursor_line - offset) as u16;
    let cx = rect.x + layout.cursor_col.min(rect.width.saturating_sub(1) as usize) as u16;
    if cy >= rect.y + rect.height {
        return;
    }
    let ch = editor
        .rows()
        .get(editor.row)
        .and_then(|r| r.get(editor.col))
        .map(|c| c.to_string())
        .unwrap_or_else(|| " ".to_string());
    if let Some(cell) = frame.buffer_mut().cell_mut((cx, cy)) {
        cell.set_symbol(&ch).set_style(
            Style::default()
                .fg(theme::t().bg)
                .bg(theme::t().accent)
                .add_modifier(Modifier::BOLD),
        );
    }
}

fn draw_draft(frame: &mut Frame, app: &App, draft: &crate::app::Draft) {
    let area = frame.area();
    let width = area.width.saturating_sub(8).clamp(30, 100);
    let inner_width = width.saturating_sub(2);
    let layout = wrap_editor(&draft.editor, inner_width as usize);
    let height = ((layout.lines.len() + 2) as u16)
        .min(area.height.saturating_sub(4))
        .max(5);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + area.height.saturating_sub(height + 2);
    let rect = Rect::new(x, y, width, height);

    let is_suggestion = matches!(draft.target, DraftTarget::Suggestion { .. });
    let title = match &draft.target {
        DraftTarget::NewLine {
            path,
            start_line,
            line,
            ..
        } => {
            let range = match start_line {
                Some(s) => format!("{path}:{s}-{line}"),
                None => format!("{path}:{line}"),
            };
            format!(" comment · {range} ")
        }
        DraftTarget::Suggestion {
            path,
            start_line,
            line,
            ..
        } => {
            let range = match start_line {
                Some(s) => format!("{path}:{s}-{line}"),
                None => format!("{path}:{line}"),
            };
            format!(" suggested change · {range} ")
        }
        DraftTarget::Reply { thread, .. } => {
            let who = app
                .threads
                .get(*thread)
                .and_then(|t| t.comments.first())
                .map(|c| c.author.clone())
                .unwrap_or_default();
            format!(" reply to @{who} ")
        }
    };
    let border = if is_suggestion {
        theme::t().green
    } else {
        theme::t().accent
    };

    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_style(Style::default().fg(border))
        .title(Span::styled(
            title,
            Style::default().fg(border).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(
            if let Some(err) = &draft.error {
                format!(" {err} ")
            } else if draft.submitting {
                " posting… ".to_string()
            } else if is_suggestion {
                " Ctrl+S post suggestion · Esc cancel ".to_string()
            } else {
                " Ctrl+S submit · Esc cancel ".to_string()
            },
            Style::default().fg(if draft.error.is_some() {
                theme::t().red
            } else {
                theme::t().faint
            }),
        ));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    draw_editor(frame, &draft.editor, inner, &layout, !draft.submitting);
}

fn draw_review(frame: &mut Frame, review: &crate::app::ReviewDraft, pending: usize) {
    use crate::github::ReviewVerdict;

    let area = frame.area();
    let width = area.width.saturating_sub(8).clamp(30, 100);
    let inner_width = width.saturating_sub(2);
    let layout = wrap_editor(&review.editor, inner_width as usize);
    let height = ((layout.lines.len() + 3) as u16)
        .min(area.height.saturating_sub(4))
        .max(7);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + area.height.saturating_sub(height + 2);
    let rect = Rect::new(x, y, width, height);

    frame.render_widget(Clear, rect);
    let title = if pending > 0 {
        format!(
            " submit review · {pending} draft comment{} ",
            if pending == 1 { "" } else { "s" }
        )
    } else {
        " submit review ".to_string()
    };
    let block = Block::bordered()
        .border_style(Style::default().fg(theme::t().accent))
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme::t().accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(
            if let Some(err) = &review.error {
                format!(" {err} ")
            } else if review.submitting {
                " submitting… ".to_string()
            } else {
                " Tab verdict · Ctrl+S submit · Esc cancel ".to_string()
            },
            Style::default().fg(if review.error.is_some() {
                theme::t().red
            } else {
                theme::t().faint
            }),
        ));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.height == 0 {
        return;
    }

    // Verdict selector.
    let verdicts = [
        ReviewVerdict::Comment,
        ReviewVerdict::Approve,
        ReviewVerdict::RequestChanges,
    ];
    let mut spans: Vec<Span> = Vec::new();
    for (i, verdict) in verdicts.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("   "));
        }
        let selected = *verdict == review.verdict;
        let marker = if selected { "●" } else { "○" };
        let style = if selected {
            Style::default()
                .fg(match verdict {
                    ReviewVerdict::Comment => theme::t().accent,
                    ReviewVerdict::Approve => theme::t().green,
                    ReviewVerdict::RequestChanges => theme::t().red,
                })
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::t().faint)
        };
        spans.push(Span::styled(format!("{marker} {}", verdict.label()), style));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    // Summary editor.
    let editor_rect = Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(1),
    );
    draw_editor(
        frame,
        &review.editor,
        editor_rect,
        &layout,
        !review.submitting,
    );
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let rect = centered(area, 70, 80);
    frame.render_widget(Clear, rect);
    let mode = match theme::t().mode {
        crate::termbg::Mode::Dark => "dark",
        crate::termbg::Mode::Light => "light",
    };
    let block = Block::bordered()
        .border_style(Style::default().fg(theme::t().accent))
        .title(Span::styled(
            format!(" help · {mode} theme "),
            Style::default()
                .fg(theme::t().accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(
            " press any key to close ",
            Style::default().fg(theme::t().faint),
        ));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let entries = [
        ("Tab", "switch focus between file list and diff"),
        ("j / k", "move down / up"),
        ("g / G", "jump to top / bottom"),
        ("Ctrl+d / Ctrl+u", "half page down / up"),
        ("[ / ]", "previous / next file"),
        ("Enter", "open the selected file (file list)"),
        (
            "c",
            "write a review comment on the current line (held as a draft)",
        ),
        (
            "s",
            "suggest a code change on the line or selection (a draft)",
        ),
        ("d", "discard the draft comment under the cursor"),
        ("v / V", "start a range selection, then c to comment on it"),
        ("r", "reply to the comment thread at the cursor"),
        ("Ctrl+S", "submit the comment (while writing)"),
        ("t", "toggle display of resolved threads"),
        ("o", "open the pull request in a browser"),
        ("e", "open the current file in zeditor / VS Code"),
        ("T", "open a terminal in the checkout"),
        ("S", "submit your review, publishing all drafts"),
        ("L", "show only changes since your last review"),
        ("R", "reload the pull request"),
        (
            "mouse",
            "click to move, drag or shift-click to select, wheel to scroll",
        ),
        ("q", "quit"),
    ];
    let lines: Vec<Line> = entries
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(
                    format!(" {k:<18}"),
                    Style::default()
                        .fg(theme::t().accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled((*v).to_string(), Style::default().fg(theme::t().text)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn status_glyph(status: FileStatus) -> (char, ratatui::style::Color) {
    match status {
        FileStatus::Added => ('A', theme::t().green),
        FileStatus::Deleted => ('D', theme::t().red),
        FileStatus::Modified => ('M', theme::t().yellow),
        FileStatus::Renamed => ('R', theme::t().accent),
        FileStatus::Copied => ('C', theme::t().accent),
    }
}

fn centered(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

/// Center a fixed-size rectangle inside `area`, clamped to fit.
fn centered_fixed(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let x = area.x + (area.width - width) / 2;
    let y = area.y + (area.height - height) / 2;
    Rect::new(x, y, width, height)
}
