//! Application state and event handling.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::ListState;

use crate::editor::Editor;
use crate::github::{self, ReviewVerdict};
use crate::model::{
    self, Anchor, Comment, FileDiff, FileStatus, LineKind, Loaded, PrMeta, PrSummary, ReviewThread,
    Side,
};
use crate::syntax::Highlighter;
use crate::{launch, theme, util};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Loading,
    Picker,
    Main,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Files,
    Diff,
}

/// Metadata attached to every rendered line in the diff pane. This is what
/// drives cursor navigation and per-row background colouring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineMeta {
    Hunk,
    Code {
        hunk: usize,
        line: usize,
        kind: LineKind,
    },
    Comment {
        thread: usize,
        comment: usize,
    },
    Spacer,
}

pub struct FileView {
    pub lines: Vec<Line<'static>>,
    pub meta: Vec<LineMeta>,
    pub cursor: usize,
    pub scroll: u16,
    pub width: u16,
    pub built_width: u16,
    pub height: u16,
    pub old_w: u16,
    pub new_w: u16,
    pub file_index: usize,
}

impl Default for FileView {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            meta: Vec::new(),
            cursor: 0,
            scroll: 0,
            width: 0,
            built_width: u16::MAX,
            height: 0,
            old_w: 3,
            new_w: 3,
            file_index: usize::MAX,
        }
    }
}

#[derive(Debug, Clone)]
pub enum DraftTarget {
    NewLine {
        path: String,
        start_line: Option<u32>,
        line: u32,
        side: Side,
        anchor: Anchor,
    },
    /// A code-change suggestion: the editor holds the replacement code.
    Suggestion {
        path: String,
        start_line: Option<u32>,
        line: u32,
        anchor: Anchor,
    },
    Reply {
        thread: usize,
    },
    /// Reopen an existing draft comment in the composer. When `suggestion`
    /// is set the editor holds the bare replacement code (the `suggestion`
    /// fence is added back on save).
    Edit {
        thread: usize,
        comment: usize,
        suggestion: bool,
    },
}

pub struct Draft {
    pub target: DraftTarget,
    pub editor: Editor,
    pub submitting: bool,
    pub error: Option<String>,
}

/// The "finish your review" composer: a summary plus a verdict.
pub struct ReviewDraft {
    pub editor: Editor,
    pub verdict: ReviewVerdict,
    pub submitting: bool,
    pub error: Option<String>,
}

/// Asks whether to check the pull request out into the working copy before
/// opening the editor on it.
pub struct CheckoutPrompt {
    pub running: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RangeSel {
    pub rows: Vec<usize>,
    pub start_line: u32,
    pub line: u32,
    pub side: Side,
    pub anchor: Anchor,
}

/// The line (or range) a comment or suggestion will be attached to.
#[derive(Debug, Clone)]
struct Target {
    rows: Vec<usize>,
    start_line: Option<u32>,
    line: u32,
    side: Side,
    anchor: Anchor,
}

#[derive(Debug)]
pub enum Msg {
    Loaded(Box<Loaded>),
    LoadErr(String),
    PrList(Vec<PrSummary>),
    PrListErr(String),
    PostErr(String),
    ReviewSubmitted(ReviewVerdict),
    SinceReview {
        review: github::LastReview,
        files: Vec<FileDiff>,
    },
    SinceReviewNone,
    SinceReviewErr(String),
    CheckoutDone,
    CheckoutErr(String),
}

/// Per-file syntax highlight cache: `file index -> hunk -> line -> spans`.
type HlCache = HashMap<usize, Vec<Vec<Vec<(Style, String)>>>>;

pub struct App {
    pub repo: String,
    /// Checkout the review is viewed in: the context for the editor and
    /// terminal shortcuts.
    pub root: PathBuf,
    pub screen: Screen,
    pub loading_msg: String,
    pub error: String,
    pub meta: Option<PrMeta>,
    pub files: Vec<FileDiff>,
    /// The complete pull request diff, kept so "since last review" can be undone.
    pub full_files: Vec<FileDiff>,
    pub since_review: bool,
    pub since_review_loading: bool,
    pub last_review: Option<github::LastReview>,
    pub threads: Vec<ReviewThread>,
    pub show_resolved: bool,
    pub focus: Focus,
    pub file_index: usize,
    pub file_list_state: ListState,
    pub view: FileView,
    pub hl: Highlighter,
    hl_cache: HlCache,
    pub draft: Option<Draft>,
    pub review_draft: Option<ReviewDraft>,
    pub checkout_prompt: Option<CheckoutPrompt>,
    pub status: String,
    pub should_quit: bool,
    pub help: bool,
    pub pr_list: Vec<PrSummary>,
    pub picker_state: ListState,
    pub tx: Sender<Msg>,
    pub rx: Receiver<Msg>,
    pub view_dirty: bool,
    /// Offline fixtures are read-only; posting is disabled.
    pub offline: bool,
    /// View row where a visual (range) selection started, if any.
    pub visual_start: Option<usize>,
    /// Counter for locally created (unpublished) comments.
    pub local_id: usize,
    /// Screen rectangles from the last frame, used for mouse hit-testing.
    pub diff_area: Rect,
    pub sidebar_area: Rect,
}

impl App {
    pub fn new(repo: String, tx: Sender<Msg>, rx: Receiver<Msg>) -> Self {
        let mut file_list_state = ListState::default();
        file_list_state.select(Some(0));
        let mut picker_state = ListState::default();
        picker_state.select(Some(0));
        Self {
            repo,
            root: launch::project_dir(),
            screen: Screen::Loading,
            loading_msg: String::new(),
            error: String::new(),
            meta: None,
            files: Vec::new(),
            full_files: Vec::new(),
            since_review: false,
            since_review_loading: false,
            last_review: None,
            threads: Vec::new(),
            show_resolved: false,
            focus: Focus::Diff,
            file_index: 0,
            file_list_state,
            view: FileView::default(),
            hl: Highlighter::new(theme::t().syntax_theme),
            hl_cache: HashMap::new(),
            draft: None,
            review_draft: None,
            checkout_prompt: None,
            status: String::new(),
            should_quit: false,
            help: false,
            pr_list: Vec::new(),
            picker_state,
            tx,
            rx,
            view_dirty: true,
            offline: false,
            visual_start: None,
            local_id: 0,
            diff_area: Rect::default(),
            sidebar_area: Rect::default(),
        }
    }

    pub fn pr_number(&self) -> u64 {
        self.meta.as_ref().map(|m| m.number).unwrap_or(0)
    }

    // -----------------------------------------------------------------------
    // Loading
    // -----------------------------------------------------------------------

    pub fn start_load(&mut self, number: u64) {
        self.screen = Screen::Loading;
        self.loading_msg = format!("Loading {}#{number}…", self.repo);
        let tx = self.tx.clone();
        let repo = self.repo.clone();
        std::thread::spawn(move || match github::fetch_pr(&repo, number) {
            Ok(loaded) => {
                let _ = tx.send(Msg::Loaded(Box::new(loaded)));
            }
            Err(e) => {
                let _ = tx.send(Msg::LoadErr(format!("{e:#}")));
            }
        });
    }

    pub fn start_pr_list(&mut self) {
        self.screen = Screen::Loading;
        self.loading_msg = format!("Fetching open pull requests for {}…", self.repo);
        let tx = self.tx.clone();
        let repo = self.repo.clone();
        std::thread::spawn(move || match github::list_open_prs(&repo) {
            Ok(list) => {
                let _ = tx.send(Msg::PrList(list));
            }
            Err(e) => {
                let _ = tx.send(Msg::PrListErr(format!("{e:#}")));
            }
        });
    }

    /// Install already-loaded pull request data (used by offline fixtures).
    pub fn set_loaded(&mut self, loaded: Loaded) {
        let Loaded {
            meta,
            files,
            mut threads,
        } = loaded;
        model::reanchor_all(&files, &mut threads);
        self.meta = Some(meta);
        self.full_files = files.clone();
        self.files = files;
        self.threads = threads;
        self.since_review = false;
        self.since_review_loading = false;
        self.last_review = None;
        self.hl_cache.clear();
        self.file_index = 0;
        self.file_list_state.select(Some(0));
        self.focus = Focus::Diff;
        self.view = FileView::default();
        self.view_dirty = true;
        self.screen = Screen::Main;
        self.status = format!("Loaded {} files", self.files.len());
    }

    pub fn drain_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle_message(msg);
        }
    }

    fn handle_message(&mut self, msg: Msg) {
        match msg {
            Msg::Loaded(loaded) => {
                self.set_loaded(*loaded);
            }
            Msg::LoadErr(e) => {
                self.error = e;
                self.screen = Screen::Error;
            }
            Msg::PrList(list) => {
                if list.is_empty() {
                    self.error = format!("No open pull requests found in {}.", self.repo);
                    self.screen = Screen::Error;
                } else {
                    self.pr_list = list;
                    self.picker_state.select(Some(0));
                    self.screen = Screen::Picker;
                }
            }
            Msg::PrListErr(e) => {
                self.error = e;
                self.screen = Screen::Error;
            }
            Msg::PostErr(e) => {
                self.status = format!("Failed: {e}");
                if let Some(review) = self.review_draft.as_mut() {
                    review.submitting = false;
                    review.error = Some(e);
                }
            }
            Msg::ReviewSubmitted(verdict) => {
                self.review_draft = None;
                self.status = match verdict {
                    ReviewVerdict::Approve => "Review submitted: approved".to_string(),
                    ReviewVerdict::RequestChanges => {
                        "Review submitted: changes requested".to_string()
                    }
                    ReviewVerdict::Comment => "Review submitted".to_string(),
                };
                // Reload so the published comments appear as real threads.
                let number = self.pr_number();
                self.start_load(number);
            }
            Msg::SinceReview { review, files } => {
                self.enter_since_review(review, files);
            }
            Msg::SinceReviewNone => {
                self.since_review_loading = false;
                self.status = "You have not reviewed this pull request yet".to_string();
            }
            Msg::SinceReviewErr(e) => {
                self.since_review_loading = false;
                self.status = format!("Could not compute changes since your review: {e}");
            }
            Msg::CheckoutDone => {
                self.checkout_prompt = None;
                self.status = format!("Checked out #{}", self.pr_number());
                self.launch_editor();
            }
            Msg::CheckoutErr(e) => {
                if let Some(prompt) = self.checkout_prompt.as_mut() {
                    prompt.running = false;
                    prompt.error = Some(e);
                }
            }
        }
    }

    fn jump_to_thread(&mut self, thread: usize) {
        if let Some(pos) = self
            .view
            .meta
            .iter()
            .position(|m| matches!(m, LineMeta::Comment { thread: t, .. } if *t == thread))
        {
            self.view.cursor = pos;
            self.ensure_visible();
        }
    }

    // -----------------------------------------------------------------------
    // Highlighting & view construction
    // -----------------------------------------------------------------------

    fn ensure_highlight(&mut self, fi: usize) {
        if self.hl_cache.contains_key(&fi) {
            return;
        }
        let file = &self.files[fi];
        let mut contents: Vec<&str> = Vec::new();
        let mut idx: Vec<(usize, usize)> = Vec::new();
        for (hi, hunk) in file.hunks.iter().enumerate() {
            for (li, line) in hunk.lines.iter().enumerate() {
                contents.push(line.content.as_str());
                idx.push((hi, li));
            }
        }
        let mut spans = self.hl.highlight(&file.path, &contents);
        let mut result: Vec<Vec<Vec<(Style, String)>>> = file
            .hunks
            .iter()
            .map(|h| vec![Vec::new(); h.lines.len()])
            .collect();
        for (k, (hi, li)) in idx.into_iter().enumerate() {
            result[hi][li] = std::mem::take(&mut spans[k]);
        }
        self.hl_cache.insert(fi, result);
    }

    pub fn visible_thread_indices(&self) -> Vec<usize> {
        self.threads
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.comments.is_empty() && (self.show_resolved || !t.resolved))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn unresolved_count(&self) -> usize {
        self.threads
            .iter()
            .filter(|t| !t.resolved && !t.comments.is_empty())
            .count()
    }

    pub fn file_thread_count(&self, path: &str) -> usize {
        self.threads
            .iter()
            .filter(|t| {
                t.path == path && !t.comments.is_empty() && (self.show_resolved || !t.resolved)
            })
            .count()
    }

    pub fn ensure_view(&mut self) {
        if self.screen != Screen::Main {
            return;
        }
        if self.files.is_empty() {
            self.view.lines.clear();
            self.view.meta.clear();
            return;
        }
        if self.file_index >= self.files.len() {
            self.file_index = 0;
        }
        if self.view_dirty
            || self.view.file_index != self.file_index
            || self.view.built_width != self.view.width
            || self.view.width == 0
        {
            self.rebuild_view();
            self.view_dirty = false;
        }
    }

    fn rebuild_view(&mut self) {
        let width = self.view.width.max(12);
        let fi = self.file_index;
        if self.files.is_empty() {
            self.view.lines.clear();
            self.view.meta.clear();
            self.view.cursor = 0;
            self.view.scroll = 0;
            self.view.file_index = fi;
            self.view.built_width = width;
            return;
        }

        self.ensure_highlight(fi);
        let file = &self.files[fi];

        // Gutter widths.
        let mut old_max = 1u32;
        let mut new_max = 1u32;
        for hunk in &file.hunks {
            for line in &hunk.lines {
                if let Some(n) = line.old_lineno {
                    old_max = old_max.max(n);
                }
                if let Some(n) = line.new_lineno {
                    new_max = new_max.max(n);
                }
            }
        }
        let old_w = old_max.to_string().len().max(3) as u16;
        let new_w = new_max.to_string().len().max(3) as u16;
        let gutter = 1 + old_w + 1 + new_w + 1 + 1;

        // Group visible threads by anchor.
        let visible = self.visible_thread_indices();
        let mut by_anchor: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        let mut floating: Vec<usize> = Vec::new();
        for &ti in &visible {
            let t = &self.threads[ti];
            if t.path != file.path {
                continue;
            }
            match t.anchor {
                Some(a) => by_anchor.entry((a.hunk, a.line)).or_default().push(ti),
                None => floating.push(ti),
            }
        }

        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut meta: Vec<LineMeta> = Vec::new();
        let hl = &self.hl_cache[&fi];

        for (hi, hunk) in file.hunks.iter().enumerate() {
            // Hunk header, aligned with the code column like GitHub does. The
            // `@@ … @@` ranges are emphasised because they mark a jump to an
            // unrelated part of the file.
            let indent = " ".repeat(gutter as usize);
            let (ranges, context) = split_hunk_header(&hunk.header);
            let ranges_w = unicode_width::UnicodeWidthStr::width(ranges);
            let avail = (width as usize).saturating_sub(gutter as usize + 2);
            let mut spans: Vec<Span<'static>> = vec![
                Span::raw(indent),
                Span::styled(
                    "⋯ ".to_string(),
                    Style::default()
                        .fg(theme::t().hunk_fg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    ranges.to_string(),
                    Style::default()
                        .fg(theme::t().hunk_fg)
                        .add_modifier(Modifier::BOLD),
                ),
            ];
            if !context.is_empty() {
                let room = avail.saturating_sub(ranges_w + 1);
                if room > 1 {
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(
                        util::truncate(context, room - 1),
                        Style::default().fg(theme::t().dim),
                    ));
                }
            }
            lines.push(Line::from(spans));
            meta.push(LineMeta::Hunk);

            for (li, line) in hunk.lines.iter().enumerate() {
                let has_thread = by_anchor.contains_key(&(hi, li));
                let marker = if has_thread { "●" } else { " " };
                let mut spans: Vec<Span<'static>> = vec![Span::styled(
                    marker.to_string(),
                    Style::default().fg(theme::t().purple),
                )];
                spans.push(Span::styled(
                    pad_num(line.old_lineno, old_w),
                    Style::default().fg(theme::t().faint),
                ));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    pad_num(line.new_lineno, new_w),
                    Style::default().fg(theme::t().faint),
                ));
                let (sign, sign_style) = match line.kind {
                    LineKind::Added => ('+', Style::default().fg(theme::t().green)),
                    LineKind::Removed => ('-', Style::default().fg(theme::t().red)),
                    LineKind::Context => (' ', Style::default().fg(theme::t().faint)),
                };
                spans.push(Span::styled(sign.to_string(), sign_style));
                spans.push(Span::raw(" "));
                for (style, text) in &hl[hi][li] {
                    spans.push(Span::styled(text.clone(), *style));
                }
                lines.push(Line::from(spans));
                meta.push(LineMeta::Code {
                    hunk: hi,
                    line: li,
                    kind: line.kind,
                });

                if let Some(threads) = by_anchor.get(&(hi, li)) {
                    for &ti in threads {
                        let box_lines = self.comment_box(ti, gutter as usize, width);
                        for (l, ci) in box_lines {
                            lines.push(l);
                            meta.push(LineMeta::Comment {
                                thread: ti,
                                comment: ci,
                            });
                        }
                        lines.push(Line::from(""));
                        meta.push(LineMeta::Spacer);
                    }
                }
            }
        }

        if file.hunks.is_empty() {
            let msg = if file.binary {
                "Binary file — contents not shown"
            } else {
                "No textual changes to display (mode or rename only)"
            };
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(gutter as usize)),
                Span::styled(msg.to_string(), Style::default().fg(theme::t().dim)),
            ]));
            meta.push(LineMeta::Hunk);
        }

        if !floating.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "── unresolved comments not present in this diff ──".to_string(),
                Style::default().fg(theme::t().yellow),
            )]));
            meta.push(LineMeta::Hunk);
            for ti in floating {
                let box_lines = self.comment_box(ti, gutter as usize, width);
                for (l, ci) in box_lines {
                    lines.push(l);
                    meta.push(LineMeta::Comment {
                        thread: ti,
                        comment: ci,
                    });
                }
                lines.push(Line::from(""));
                meta.push(LineMeta::Spacer);
            }
        }

        self.view.lines = lines;
        self.view.meta = meta;
        self.view.old_w = old_w;
        self.view.new_w = new_w;
        self.view.file_index = fi;
        self.view.built_width = width;
        if self.view.cursor >= self.view.meta.len() {
            self.view.cursor = self.view.meta.len().saturating_sub(1);
        }
        self.ensure_visible();
    }

    /// The rendered lines of a thread, each tagged with the index of the
    /// comment it belongs to (so the cursor can address a single comment).
    fn comment_box(&self, ti: usize, indent: usize, width: u16) -> Vec<(Line<'static>, usize)> {
        let t = &self.threads[ti];
        let indent = indent.min(width as usize);
        let avail = (width as usize).saturating_sub(indent);
        let inner = avail.saturating_sub(4).max(8);
        let pad: String = " ".repeat(indent);
        let mut out: Vec<(Line<'static>, usize)> = Vec::new();

        for (ci, c) in t.comments.iter().enumerate() {
            let corner = if ci == 0 { "╭─" } else { "├─" };
            let mut head = vec![
                Span::styled(pad.clone(), Style::default()),
                Span::styled(corner.to_string(), Style::default().fg(theme::t().faint)),
                Span::styled(
                    format!(" @{} ", c.author),
                    Style::default()
                        .fg(theme::t().purple)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    if c.pending {
                        "· draft".to_string()
                    } else {
                        format!("· {}", util::short_time(&c.created_at))
                    },
                    Style::default().fg(if c.pending {
                        theme::t().yellow
                    } else {
                        theme::t().faint
                    }),
                ),
            ];
            if ci == 0
                && let (Some(s), Some(e)) = (t.start_line, t.line)
                && s != e
            {
                head.push(Span::styled(
                    format!("  L{s}–{e}"),
                    Style::default().fg(theme::t().faint),
                ));
            }
            if ci == 0 && t.outdated {
                head.push(Span::styled(
                    "  (outdated)".to_string(),
                    Style::default().fg(theme::t().yellow),
                ));
            }
            if ci == 0 && t.resolved {
                head.push(Span::styled(
                    "  ✓ resolved".to_string(),
                    Style::default().fg(theme::t().green),
                ));
            }
            out.push((Line::from(head), ci));

            let mut body: Vec<Line<'static>> = Vec::new();
            push_comment_body(&mut body, &pad, &c.body, inner);
            out.extend(body.into_iter().map(|l| (l, ci)));
        }
        // The closing corner belongs to the last comment of the thread.
        let last = t.comments.len().saturating_sub(1);
        out.push((
            Line::from(vec![
                Span::styled(pad, Style::default()),
                Span::styled("╰─".to_string(), Style::default().fg(theme::t().faint)),
            ]),
            last,
        ));
        out
    }

    // -----------------------------------------------------------------------
    // Scrolling
    // -----------------------------------------------------------------------

    pub fn ensure_visible(&mut self) {
        let height = self.view.height.max(1) as usize;
        let cursor = self.view.cursor;
        let scroll = self.view.scroll as usize;
        if cursor < scroll {
            self.view.scroll = cursor as u16;
        } else if cursor >= scroll + height {
            self.view.scroll = (cursor + 1 - height) as u16;
        }
        let max_scroll = self.view.meta.len().saturating_sub(height) as u16;
        if self.view.scroll > max_scroll {
            self.view.scroll = max_scroll;
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        if self.view.meta.is_empty() {
            return;
        }
        let max = self.view.meta.len() - 1;
        let cur = self.view.cursor as isize;
        let next = (cur + delta).clamp(0, max as isize) as usize;
        self.view.cursor = next;
        self.ensure_visible();
    }

    fn page(&mut self, down: bool) {
        let h = self.view.height.max(1) as isize;
        let step = if down { h } else { -h };
        self.move_cursor(step);
    }

    fn half_page(&mut self, down: bool) {
        let h = (self.view.height.max(2) as isize) / 2;
        let step = if down { h } else { -h };
        self.move_cursor(step);
    }

    // -----------------------------------------------------------------------
    // Navigation
    // -----------------------------------------------------------------------

    pub fn select_file(&mut self, index: usize) {
        if self.files.is_empty() {
            return;
        }
        let index = index.min(self.files.len() - 1);
        if index != self.file_index {
            self.file_index = index;
            self.view.cursor = 0;
            self.view.scroll = 0;
            self.view_dirty = true;
            self.visual_start = None;
            self.ensure_view();
        }
        self.file_list_state.select(Some(index));
    }

    fn next_file(&mut self, delta: isize) {
        if self.files.is_empty() {
            return;
        }
        let max = self.files.len() as isize - 1;
        let next = (self.file_index as isize + delta).clamp(0, max) as usize;
        self.select_file(next);
    }

    // -----------------------------------------------------------------------
    // Changes since the last review
    // -----------------------------------------------------------------------

    /// Swap in a new set of files, re-anchoring threads and resetting the view.
    fn apply_files(&mut self, files: Vec<FileDiff>) {
        self.files = files;
        self.hl_cache.clear();
        model::reanchor_all(&self.files, &mut self.threads);
        self.file_index = 0;
        self.file_list_state.select(Some(0));
        self.view = FileView::default();
        self.view_dirty = true;
        self.visual_start = None;
    }

    /// Show only the changes made since the viewer's last review.
    pub fn enter_since_review(&mut self, review: github::LastReview, files: Vec<FileDiff>) {
        self.since_review = true;
        self.since_review_loading = false;
        self.last_review = Some(review.clone());
        self.apply_files(files);
        let short = &review.sha[..review.sha.len().min(7)];
        let state = match review.state.as_str() {
            "APPROVED" => "approved",
            "CHANGES_REQUESTED" => "requested changes",
            "DISMISSED" => "reviewed",
            _ => "commented",
        };
        self.status = format!(
            "Changes since you {state} on {} · {short}",
            util::short_time(&review.submitted_at)
        );
    }

    /// Restore the complete pull request diff.
    pub fn exit_since_review(&mut self) {
        self.since_review = false;
        self.since_review_loading = false;
        self.last_review = None;
        let full = self.full_files.clone();
        self.apply_files(full);
        self.status = "Showing the full pull request diff".to_string();
    }

    fn toggle_since_review(&mut self) {
        if self.since_review {
            self.exit_since_review();
            return;
        }
        if self.meta.is_none() || self.since_review_loading {
            return;
        }
        self.since_review_loading = true;
        self.status = "Looking for your last review…".to_string();
        let tx = self.tx.clone();
        let repo = self.repo.clone();
        let number = self.pr_number();
        let head = self
            .meta
            .as_ref()
            .map(|m| m.head_sha.clone())
            .unwrap_or_default();
        std::thread::spawn(
            move || match github::diff_since_last_review(&repo, number, &head) {
                Ok(Some((review, files))) => {
                    let _ = tx.send(Msg::SinceReview { review, files });
                }
                Ok(None) => {
                    let _ = tx.send(Msg::SinceReviewNone);
                }
                Err(e) => {
                    let _ = tx.send(Msg::SinceReviewErr(format!("{e:#}")));
                }
            },
        );
    }

    // -----------------------------------------------------------------------
    // Comments
    // -----------------------------------------------------------------------

    fn current_code_line(&self) -> Option<(usize, usize, LineKind)> {
        match self.view.meta.get(self.view.cursor) {
            Some(LineMeta::Code { hunk, line, kind }) => Some((*hunk, *line, *kind)),
            _ => None,
        }
    }

    /// The thread and comment indices of the comment row under the cursor.
    fn comment_at_cursor(&self) -> Option<(usize, usize)> {
        match self.view.meta.get(self.view.cursor) {
            Some(LineMeta::Comment { thread, comment }) => Some((*thread, *comment)),
            _ => None,
        }
    }

    fn thread_at_cursor(&self) -> Option<usize> {
        match self.view.meta.get(self.view.cursor) {
            Some(LineMeta::Comment { thread, .. }) => Some(*thread),
            Some(LineMeta::Code { hunk, line, .. }) => {
                self.visible_thread_indices().into_iter().find(|&ti| {
                    self.threads[ti].anchor
                        == Some(Anchor {
                            hunk: *hunk,
                            line: *line,
                        })
                })
            }
            _ => None,
        }
    }

    fn start_comment(&mut self) {
        let Some(path) = self.files.get(self.file_index).map(|f| f.path.clone()) else {
            return;
        };
        let Some(t) = self.comment_target() else {
            self.status = "Move the cursor onto a code line to comment".to_string();
            return;
        };
        self.visual_start = None;
        self.draft = Some(Draft {
            target: DraftTarget::NewLine {
                path,
                start_line: t.start_line,
                line: t.line,
                side: t.side,
                anchor: t.anchor,
            },
            editor: Editor::new(),
            submitting: false,
            error: None,
        });
    }

    /// Open the suggestion editor, pre-filled with the current line (or the
    /// selected range) so it can be rewritten and posted as a GitHub
    /// `suggestion` block.
    fn start_suggestion(&mut self) {
        let Some(path) = self.files.get(self.file_index).map(|f| f.path.clone()) else {
            return;
        };
        let Some(t) = self.comment_target() else {
            self.status = "Move onto a code line to suggest a change".to_string();
            return;
        };
        if t.side != Side::Right {
            self.status = "Suggestions can only target added or context lines".to_string();
            return;
        }
        let code = self.rows_content(&t.rows);
        self.visual_start = None;
        self.draft = Some(Draft {
            target: DraftTarget::Suggestion {
                path,
                start_line: t.start_line,
                line: t.line,
                anchor: t.anchor,
            },
            editor: Editor::from_text(&code),
            submitting: false,
            error: None,
        });
        self.status = "Editing suggested change — Ctrl+S to post, Esc to cancel".to_string();
    }

    /// The new-side source text of the given view rows.
    fn rows_content(&self, rows: &[usize]) -> String {
        let Some(file) = self.files.get(self.file_index) else {
            return String::new();
        };
        let mut out: Vec<&str> = Vec::new();
        for &r in rows {
            if let Some(LineMeta::Code { hunk, line, .. }) = self.view.meta.get(r) {
                out.push(file.hunks[*hunk].lines[*line].content.as_str());
            }
        }
        out.join("\n")
    }

    /// The line (or range) the cursor currently addresses.
    fn comment_target(&self) -> Option<Target> {
        if let Some(sel) = self.selected_range() {
            let start_line = if sel.start_line == sel.line {
                None
            } else {
                Some(sel.start_line)
            };
            return Some(Target {
                rows: sel.rows,
                start_line,
                line: sel.line,
                side: sel.side,
                anchor: sel.anchor,
            });
        }
        let (hunk, line, kind) = self.current_code_line()?;
        let file = self.files.get(self.file_index)?;
        let dl = &file.hunks[hunk].lines[line];
        let (side, lineno) = match kind {
            LineKind::Removed => (Side::Left, dl.old_lineno),
            _ => (Side::Right, dl.new_lineno),
        };
        Some(Target {
            rows: vec![self.view.cursor],
            start_line: None,
            line: lineno?,
            side,
            anchor: Anchor { hunk, line },
        })
    }

    /// Resolve the current visual selection into a commentable range, keeping
    /// only the contiguous run of lines that share a side.
    pub fn selected_range(&self) -> Option<RangeSel> {
        let start = self.visual_start?;
        let end = self.view.cursor;
        if start == end {
            return None;
        }
        let (lo, hi) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let file = self.files.get(self.file_index)?;

        let mut codes: Vec<(usize, Anchor, LineKind)> = Vec::new();
        for r in lo..=hi {
            if let Some(LineMeta::Code { hunk, line, kind }) = self.view.meta.get(r) {
                codes.push((
                    r,
                    Anchor {
                        hunk: *hunk,
                        line: *line,
                    },
                    *kind,
                ));
            }
        }
        if codes.is_empty() {
            return None;
        }

        // Decide which side the range lives on. The line the selection started
        // on wins; otherwise prefer the side that can represent all lines.
        let start_kind = self.view.meta.get(start).and_then(|m| match m {
            LineMeta::Code { kind, .. } => Some(*kind),
            _ => None,
        });
        let side = match start_kind {
            Some(LineKind::Removed) => Side::Left,
            Some(_) => Side::Right,
            None => {
                if codes.iter().any(|(_, _, k)| *k == LineKind::Added) {
                    Side::Right
                } else if codes.iter().all(|(_, _, k)| *k == LineKind::Removed) {
                    Side::Left
                } else {
                    Side::Right
                }
            }
        };

        let mut rows: Vec<usize> = Vec::new();
        let mut chosen: Vec<(Anchor, u32)> = Vec::new();
        for (r, anchor, _) in &codes {
            let dl = &file.hunks[anchor.hunk].lines[anchor.line];
            let n = match side {
                Side::Right => dl.new_lineno,
                Side::Left => dl.old_lineno,
            };
            match n {
                Some(n) => {
                    if let Some((_, last)) = chosen.last()
                        && n != *last + 1
                    {
                        break;
                    }
                    chosen.push((*anchor, n));
                    rows.push(*r);
                }
                None => {
                    if !chosen.is_empty() {
                        break;
                    }
                }
            }
        }
        if chosen.is_empty() {
            return None;
        }
        let start_line = chosen.first().map(|(_, n)| *n).unwrap_or(0);
        let line = chosen.last().map(|(_, n)| *n).unwrap_or(0);
        let anchor = chosen.last().map(|(a, _)| *a).unwrap();
        Some(RangeSel {
            rows,
            start_line,
            line,
            side,
            anchor,
        })
    }

    fn start_reply(&mut self) {
        let Some(ti) = self.thread_at_cursor() else {
            self.status = "No comment thread here to reply to".to_string();
            return;
        };
        if self.threads[ti].id.starts_with("draft-") {
            self.status = "This comment is still a draft; submit your review first".to_string();
            return;
        }
        let root = self.threads[ti].comments.first().map(|c| c.id).unwrap_or(0);
        if root == 0 {
            self.status = "Cannot reply to a comment without an id".to_string();
            return;
        }
        self.draft = Some(Draft {
            target: DraftTarget::Reply { thread: ti },
            editor: Editor::new(),
            submitting: false,
            error: None,
        });
    }

    /// Turn the composer's content into a local draft. Nothing is sent to
    /// GitHub until the review is submitted.
    fn submit_draft(&mut self) {
        let Some(draft) = self.draft.as_ref() else {
            return;
        };
        let raw = draft.editor.text();
        let target = draft.target.clone();
        let is_suggestion = matches!(
            target,
            DraftTarget::Suggestion { .. }
                | DraftTarget::Edit {
                    suggestion: true,
                    ..
                }
        );
        if !is_suggestion && raw.trim().is_empty() {
            if let Some(d) = self.draft.as_mut() {
                d.error = Some("comment is empty".to_string());
            }
            return;
        }
        // A suggestion is published as a fenced `suggestion` block; an empty
        // body means "delete these lines".
        let body = if is_suggestion {
            suggestion_body(&raw)
        } else {
            raw
        };
        let mut new_thread = None;

        match target {
            DraftTarget::NewLine {
                path,
                start_line,
                line,
                side,
                anchor,
            } => {
                self.push_draft_comment(path, start_line, line, side, anchor, body);
                new_thread = Some(self.threads.len() - 1);
            }
            DraftTarget::Suggestion {
                path,
                start_line,
                line,
                anchor,
            } => {
                self.push_draft_comment(path, start_line, line, Side::Right, anchor, body);
                new_thread = Some(self.threads.len() - 1);
            }
            DraftTarget::Reply { thread } => {
                if let Some(t) = self.threads.get_mut(thread) {
                    t.comments.push(draft_comment(body));
                }
            }
            DraftTarget::Edit {
                thread, comment, ..
            } => {
                if let Some(c) = self
                    .threads
                    .get_mut(thread)
                    .and_then(|t| t.comments.get_mut(comment))
                {
                    c.body = body;
                    new_thread = Some(thread);
                }
            }
        }
        self.draft = None;
        self.view_dirty = true;
        self.ensure_view();
        if let Some(idx) = new_thread {
            self.focus = Focus::Diff;
            self.jump_to_thread(idx);
        }
        self.status = format!(
            "Draft saved · {} pending — press S to submit your review",
            self.pending_count()
        );
    }

    /// Add a new draft comment/suggestion as a local, unpublished thread.
    fn push_draft_comment(
        &mut self,
        path: String,
        start_line: Option<u32>,
        line: u32,
        side: Side,
        anchor: Anchor,
        body: String,
    ) {
        self.local_id += 1;
        let id = self.local_id;
        self.threads.push(ReviewThread {
            id: format!("draft-{id}"),
            path,
            line: Some(line),
            start_line,
            side,
            resolved: false,
            outdated: false,
            comments: vec![Comment {
                id: id as u64,
                ..draft_comment(body)
            }],
            anchor: Some(anchor),
        });
    }

    /// The number of comments waiting to be published with the next review.
    pub fn pending_count(&self) -> usize {
        self.threads
            .iter()
            .map(|t| t.comments.iter().filter(|c| c.pending).count())
            .sum()
    }

    /// Reopen the draft comment under the cursor in the composer, pre-filled
    /// with its current body (the bare replacement code for a suggestion).
    fn edit_draft_at_cursor(&mut self) {
        let Some((thread, comment)) = self.comment_at_cursor() else {
            self.status = "Move onto a draft comment to edit it".to_string();
            return;
        };
        let Some(c) = self
            .threads
            .get(thread)
            .and_then(|t| t.comments.get(comment))
        else {
            return;
        };
        if !c.pending {
            self.status = "Only draft comments can be edited".to_string();
            return;
        }
        let (text, suggestion) = match suggestion_code(&c.body) {
            Some(code) => (code.to_string(), true),
            None => (c.body.clone(), false),
        };
        self.visual_start = None;
        self.draft = Some(Draft {
            target: DraftTarget::Edit {
                thread,
                comment,
                suggestion,
            },
            editor: Editor::from_text(&text),
            submitting: false,
            error: None,
        });
        self.status = "Editing draft — Ctrl+S to save, Esc to cancel".to_string();
    }

    /// Remove the draft comment under the cursor. Discarding the only comment
    /// of a draft thread removes the thread itself.
    fn discard_draft_at_cursor(&mut self) {
        let Some((thread, comment)) = self.comment_at_cursor() else {
            self.status = "Move onto a draft comment to discard it".to_string();
            return;
        };
        let Some(t) = self.threads.get(thread) else {
            return;
        };
        if !t.comments.get(comment).map(|c| c.pending).unwrap_or(false) {
            self.status = "Only draft comments can be discarded".to_string();
            return;
        }
        let reply = !self.threads[thread].id.starts_with("draft-");
        self.threads[thread].comments.remove(comment);
        if self.threads[thread].comments.is_empty() {
            self.threads.remove(thread);
        }
        self.status = if reply {
            "Draft reply discarded".to_string()
        } else {
            "Draft comment discarded".to_string()
        };
        self.view_dirty = true;
        self.ensure_view();
    }

    // -----------------------------------------------------------------------
    // Key handling
    // -----------------------------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.review_draft.is_some() {
            self.on_review_key(key);
            return;
        }
        if self.draft.is_some() {
            self.on_draft_key(key);
            return;
        }
        if self.checkout_prompt.is_some() {
            self.on_checkout_key(key);
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        match self.screen {
            Screen::Picker => self.on_picker_key(key),
            Screen::Main => self.on_main_key(key),
            Screen::Loading => {
                if is_quit(&key) {
                    self.should_quit = true;
                }
            }
            Screen::Error => {
                if is_quit(&key) || key.code == KeyCode::Esc || key.code == KeyCode::Enter {
                    self.should_quit = true;
                }
            }
        }
    }

    fn on_picker_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                let max = self.pr_list.len().saturating_sub(1);
                let cur = self.picker_state.selected().unwrap_or(0);
                self.picker_state.select(Some((cur + 1).min(max)));
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let cur = self.picker_state.selected().unwrap_or(0);
                self.picker_state.select(Some(cur.saturating_sub(1)));
            }
            KeyCode::Enter => {
                if let Some(idx) = self.picker_state.selected()
                    && let Some(pr) = self.pr_list.get(idx)
                {
                    let number = pr.number;
                    self.start_load(number);
                }
            }
            _ => {
                if is_quit(&key) || key.code == KeyCode::Esc {
                    self.should_quit = true;
                }
            }
        }
    }

    fn on_main_key(&mut self, key: KeyEvent) {
        if is_quit(&key) {
            self.should_quit = true;
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.visual_start = None,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Files => Focus::Diff,
                    Focus::Diff => Focus::Files,
                };
            }
            KeyCode::Char('t') => {
                self.show_resolved = !self.show_resolved;
                self.view_dirty = true;
                self.status = if self.show_resolved {
                    "Showing resolved threads".to_string()
                } else {
                    "Hiding resolved threads".to_string()
                };
            }
            KeyCode::Char('R') => {
                let n = self.pr_number();
                self.start_load(n);
            }
            KeyCode::Char('o') => self.open_in_browser(),
            KeyCode::Char('e') => self.open_in_editor(),
            KeyCode::Char('T') => self.open_in_terminal(),
            KeyCode::Char('S') => self.start_review(),
            KeyCode::Char('L') => self.toggle_since_review(),
            KeyCode::Char(']') => self.next_file(1),
            KeyCode::Char('[') => self.next_file(-1),
            _ => match self.focus {
                Focus::Files => self.on_files_key(key),
                Focus::Diff => self.on_diff_key(key, ctrl),
            },
        }
    }

    fn on_files_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.next_file(1),
            KeyCode::Char('k') | KeyCode::Up => self.next_file(-1),
            KeyCode::Char('g') => self.select_file(0),
            KeyCode::Char('G') => {
                if !self.files.is_empty() {
                    self.select_file(self.files.len() - 1);
                }
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                self.focus = Focus::Diff;
            }
            _ => {}
        }
    }

    fn on_diff_key(&mut self, key: KeyEvent, ctrl: bool) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Files,
            KeyCode::PageDown => self.page(true),
            KeyCode::PageUp => self.page(false),
            KeyCode::Char('d') if ctrl => self.half_page(true),
            KeyCode::Char('u') if ctrl => self.half_page(false),
            KeyCode::Char('d') => self.discard_draft_at_cursor(),
            KeyCode::Char('i') => self.edit_draft_at_cursor(),
            KeyCode::Char('g') => {
                self.view.cursor = 0;
                self.view.scroll = 0;
            }
            KeyCode::Char('G') => {
                if !self.view.meta.is_empty() {
                    self.view.cursor = self.view.meta.len() - 1;
                    self.ensure_visible();
                }
            }
            KeyCode::Char('c') => self.start_comment(),
            KeyCode::Char('s') => self.start_suggestion(),
            KeyCode::Char('r') => self.start_reply(),
            KeyCode::Char('v') | KeyCode::Char('V') => self.toggle_visual(),
            _ => {}
        }
    }

    fn toggle_visual(&mut self) {
        if self.visual_start.is_some() {
            self.visual_start = None;
            return;
        }
        if self.current_code_line().is_some() {
            self.visual_start = Some(self.view.cursor);
            self.status =
                "Select a range with j/k, then press c to comment (Esc cancels)".to_string();
        } else {
            self.status = "Move onto a code line to start a range selection".to_string();
        }
    }

    /// Handle keys while the "check out this pull request?" prompt is open.
    fn on_checkout_key(&mut self, key: KeyEvent) {
        if is_quit(&key) {
            self.should_quit = true;
            return;
        }
        let running = self
            .checkout_prompt
            .as_ref()
            .map(|p| p.running)
            .unwrap_or(false);
        if running {
            return;
        }
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => self.start_checkout(),
            KeyCode::Char('n') | KeyCode::Char('N') => {
                // Go ahead with whatever is checked out right now.
                self.checkout_prompt = None;
                self.launch_editor();
            }
            KeyCode::Esc => {
                self.checkout_prompt = None;
                self.status = "Checkout cancelled".to_string();
            }
            _ => {}
        }
    }

    fn on_draft_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Submit: Ctrl+S (and Ctrl+Enter where the terminal reports it).
        if ctrl && (matches!(key.code, KeyCode::Char('s')) || key.code == KeyCode::Enter) {
            self.submit_draft();
            return;
        }
        if key.code == KeyCode::Esc {
            self.draft = None;
            return;
        }
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        if draft.submitting {
            return;
        }
        edit_key(&mut draft.editor, key, ctrl);
    }

    // -----------------------------------------------------------------------
    // Review submission
    // -----------------------------------------------------------------------

    fn start_review(&mut self) {
        if self.meta.is_none() {
            return;
        }
        self.review_draft = Some(ReviewDraft {
            editor: Editor::new(),
            verdict: ReviewVerdict::Comment,
            submitting: false,
            error: None,
        });
        self.status =
            "Writing a review — Tab changes the verdict, Ctrl+S submits, Esc cancels".to_string();
    }

    fn on_review_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('s')) {
            self.submit_review_draft();
            return;
        }
        if key.code == KeyCode::Esc {
            self.review_draft = None;
            return;
        }
        let Some(review) = self.review_draft.as_mut() else {
            return;
        };
        if review.submitting {
            return;
        }
        match key.code {
            KeyCode::Tab => review.verdict = review.verdict.next(),
            KeyCode::BackTab => review.verdict = review.verdict.previous(),
            _ => edit_key(&mut review.editor, key, ctrl),
        }
    }

    fn submit_review_draft(&mut self) {
        let Some(review) = self.review_draft.as_ref() else {
            return;
        };
        if review.submitting {
            return;
        }
        let body = review.editor.text();
        let verdict = review.verdict;
        let has_comments = self.pending_count() > 0;
        if let Err(msg) = review_validation(verdict, &body, has_comments) {
            if let Some(review) = self.review_draft.as_mut() {
                review.error = Some(msg.to_string());
            }
            return;
        }
        if self.offline {
            if let Some(review) = self.review_draft.as_mut() {
                review.error = Some("offline mode: reviews are not submitted".to_string());
            }
            return;
        }

        // Collect everything that should be published with this review.
        let comments: Vec<github::PendingComment> = self
            .threads
            .iter()
            .filter(|t| t.id.starts_with("draft-"))
            .flat_map(|t| {
                t.comments
                    .iter()
                    .filter(|c| c.pending)
                    .filter_map(move |c| {
                        Some(github::PendingComment {
                            path: t.path.clone(),
                            line: t.line?,
                            start_line: t.start_line,
                            side: t.side,
                            body: c.body.clone(),
                        })
                    })
            })
            .collect();
        // Replies to already-published threads.
        let replies: Vec<(u64, String)> = self
            .threads
            .iter()
            .filter(|t| !t.id.starts_with("draft-"))
            .flat_map(|t| {
                let root = t.comments.first().map(|c| c.id).unwrap_or(0);
                t.comments
                    .iter()
                    .filter(|c| c.pending)
                    .map(move |c| (root, c.body.clone()))
            })
            .filter(|(root, _)| *root != 0)
            .collect();

        let repo = self.repo.clone();
        let number = self.pr_number();
        let commit_id = self
            .meta
            .as_ref()
            .map(|m| m.head_sha.clone())
            .unwrap_or_default();
        let tx = self.tx.clone();
        if let Some(review) = self.review_draft.as_mut() {
            review.submitting = true;
            review.error = None;
        }

        std::thread::spawn(move || {
            if let Err(e) =
                github::submit_review(&repo, number, &commit_id, verdict, &body, &comments)
            {
                let _ = tx.send(Msg::PostErr(format!("{e:#}")));
                return;
            }
            for (root, body) in &replies {
                if let Err(e) = github::reply_to_comment(&repo, number, *root, body) {
                    let _ = tx.send(Msg::PostErr(format!("{e:#}")));
                    return;
                }
            }
            let _ = tx.send(Msg::ReviewSubmitted(verdict));
        });
    }

    pub fn on_mouse(&mut self, m: MouseEvent) {
        if self.screen != Screen::Main
            || self.draft.is_some()
            || self.checkout_prompt.is_some()
            || self.help
        {
            return;
        }
        match m.kind {
            MouseEventKind::ScrollDown => self.mouse_scroll((m.column, m.row), 3),
            MouseEventKind::ScrollUp => self.mouse_scroll((m.column, m.row), -3),
            MouseEventKind::Down(MouseButton::Left) => {
                self.mouse_down((m.column, m.row), m.modifiers)
            }
            MouseEventKind::Drag(MouseButton::Left) => self.mouse_drag((m.column, m.row)),
            MouseEventKind::Up(MouseButton::Left) => self.mouse_up(),
            _ => {}
        }
    }

    fn in_rect(pos: (u16, u16), r: Rect) -> bool {
        r.width > 0
            && r.height > 0
            && pos.0 >= r.x
            && pos.0 < r.x + r.width
            && pos.1 >= r.y
            && pos.1 < r.y + r.height
    }

    fn mouse_scroll(&mut self, pos: (u16, u16), delta: isize) {
        if Self::in_rect(pos, self.sidebar_area) {
            self.focus = Focus::Files;
            self.next_file(delta.signum());
        } else if Self::in_rect(pos, self.diff_area) {
            self.focus = Focus::Diff;
            self.move_cursor(delta);
        }
    }

    /// Map a screen row in the diff pane to a view row.
    fn row_at(&self, y: u16) -> Option<usize> {
        if self.view.meta.is_empty() {
            return None;
        }
        let rel = y.saturating_sub(self.diff_area.y) as usize;
        let idx = self.view.scroll as usize + rel;
        Some(idx.min(self.view.meta.len() - 1))
    }

    /// Map a screen row in the sidebar to a file index.
    fn file_at(&self, y: u16) -> Option<usize> {
        let inner_top = self.sidebar_area.y + 1;
        if y < inner_top {
            return None;
        }
        let rel = (y - inner_top) as usize;
        let idx = self.file_list_state.offset() + rel;
        if idx < self.files.len() {
            Some(idx)
        } else {
            None
        }
    }

    fn mouse_down(&mut self, pos: (u16, u16), mods: KeyModifiers) {
        if Self::in_rect(pos, self.sidebar_area) {
            if let Some(idx) = self.file_at(pos.1) {
                self.focus = Focus::Files;
                self.select_file(idx);
            }
            return;
        }
        if Self::in_rect(pos, self.diff_area) {
            let Some(row) = self.row_at(pos.1) else {
                return;
            };
            self.focus = Focus::Diff;
            if mods.contains(KeyModifiers::SHIFT) && self.visual_start.is_none() {
                // Shift-click extends from the current cursor.
                self.visual_start = Some(self.view.cursor);
            } else {
                // A plain press starts a potential drag-selection.
                self.visual_start = Some(row);
            }
            self.view.cursor = row;
            self.ensure_visible();
        }
    }

    fn mouse_drag(&mut self, pos: (u16, u16)) {
        if Self::in_rect(pos, self.diff_area)
            && let Some(row) = self.row_at(pos.1)
        {
            self.view.cursor = row;
            self.ensure_visible();
        }
    }

    fn mouse_up(&mut self) {
        // A press without a drag is just a cursor move, not a selection.
        if self.visual_start == Some(self.view.cursor) {
            self.visual_start = None;
        }
    }

    fn open_in_browser(&mut self) {
        let url = self
            .meta
            .as_ref()
            .map(|m| m.url.clone())
            .unwrap_or_default();
        if url.is_empty() {
            return;
        }
        let url_for_thread = url.clone();
        std::thread::spawn(move || {
            let opener = if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            let _ = std::process::Command::new(opener)
                .arg(&url_for_thread)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        });
        self.status = format!("Opened {url}");
    }

    /// The file under review as it exists in the local checkout, together with
    /// the line the cursor is on. `None` when the file is not checked out
    /// (deleted by the pull request, or the checkout is on another branch).
    pub fn current_location(&self) -> Option<(PathBuf, u32)> {
        let file = self.files.get(self.file_index)?;
        if file.status == FileStatus::Deleted {
            return None;
        }
        let path = self.root.join(&file.path);
        if !path.is_file() {
            return None;
        }
        Some((path, self.cursor_lineno()))
    }

    /// The new-side line under the cursor, falling back to the closest code
    /// line above it (comment boxes and hunk headers have no line of their own).
    /// Removed lines have no new-side number, so the nearest one above is only
    /// used when there is no better anchor.
    fn cursor_lineno(&self) -> u32 {
        let Some(file) = self.files.get(self.file_index) else {
            return 1;
        };
        let start = self.view.cursor.min(self.view.meta.len().saturating_sub(1));
        let mut removed = None;
        for row in (0..=start).rev() {
            if let Some(LineMeta::Code { hunk, line, .. }) = self.view.meta.get(row)
                && let Some(dl) = file.hunks.get(*hunk).and_then(|h| h.lines.get(*line))
            {
                if let Some(n) = dl.new_lineno {
                    return n;
                }
                removed.get_or_insert(dl.old_lineno.unwrap_or(1));
            }
        }
        removed.unwrap_or(1)
    }

    /// Open the checkout in zeditor/VS Code, focused on the file being viewed.
    /// When the pull request's head is not checked out, offer to run
    /// `gh pr checkout` first.
    fn open_in_editor(&mut self) {
        if self.offline || self.meta.is_none() || self.repo.is_empty() || self.pr_checked_out() {
            self.launch_editor();
            return;
        }
        self.checkout_prompt = Some(CheckoutPrompt {
            running: false,
            error: None,
        });
        self.status = format!("#{} is not checked out locally", self.pr_number());
    }

    /// Is the commit under review the one in the working copy?
    pub fn pr_checked_out(&self) -> bool {
        let Some(meta) = &self.meta else {
            return true;
        };
        if meta.head_sha.is_empty() {
            return true;
        }
        // Without a readable HEAD there is nothing to offer to check out;
        // behave as before and open what is there.
        let Some(head) = self.local_head() else {
            return true;
        };
        head.eq_ignore_ascii_case(&meta.head_sha)
    }

    fn local_head(&self) -> Option<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["rev-parse", "HEAD"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let head = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!head.is_empty()).then_some(head)
    }

    /// Check the pull request out into the working copy, then open the editor.
    fn start_checkout(&mut self) {
        let Some(prompt) = self.checkout_prompt.as_mut() else {
            return;
        };
        if prompt.running {
            return;
        }
        prompt.running = true;
        prompt.error = None;
        let repo = self.repo.clone();
        let number = self.pr_number();
        let dir = self.root.clone();
        let tx = self.tx.clone();
        self.status = format!("Checking out {repo}#{number}…");
        std::thread::spawn(move || match github::checkout_pr(&repo, number, &dir) {
            Ok(()) => {
                let _ = tx.send(Msg::CheckoutDone);
            }
            Err(e) => {
                let _ = tx.send(Msg::CheckoutErr(format!("{e:#}")));
            }
        });
    }

    /// Open the editor on the file being viewed (or the checkout itself).
    fn launch_editor(&mut self) {
        let location = self.current_location();
        let file = location.as_ref().map(|(p, l)| (p.as_path(), *l));
        match launch::editor_launch(&self.root, file) {
            Ok(launch) => match launch::run(&launch) {
                Ok(()) => {
                    let what = match &location {
                        Some((path, line)) => {
                            let rel = path.strip_prefix(&self.root).unwrap_or(path);
                            format!("{}:{line}", rel.display())
                        }
                        None => self.root.display().to_string(),
                    };
                    self.status = format!("Opening {what} in {}", launch.name);
                }
                Err(e) => self.status = format!("Could not open the editor: {e:#}"),
            },
            Err(e) => self.status = format!("Could not open the editor: {e:#}"),
        }
    }

    /// Open a terminal window in the checkout being reviewed.
    fn open_in_terminal(&mut self) {
        match launch::terminal_launch(&self.root) {
            Ok(launch) => match launch::run(&launch) {
                Ok(()) => {
                    self.status = format!("Opening a terminal in {}", self.root.display());
                }
                Err(e) => self.status = format!("Could not open a terminal: {e:#}"),
            },
            Err(e) => self.status = format!("Could not open a terminal: {e:#}"),
        }
    }
}

fn is_quit(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('q')
        || (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c'))
}

/// Wrap replacement code in a GitHub `suggestion` fence.
pub fn suggestion_body(code: &str) -> String {
    format!("```suggestion\n{code}\n```")
}

/// The inverse of [`suggestion_body`]: the bare replacement code of a comment
/// that is exactly one `suggestion` fence.
pub fn suggestion_code(body: &str) -> Option<&str> {
    body.strip_prefix("```suggestion\n")?.strip_suffix("\n```")
}

/// Render a comment body, giving GitHub `suggestion` blocks a diff-like tint.
fn push_comment_body(out: &mut Vec<Line<'static>>, pad: &str, body: &str, inner: usize) {
    let th = theme::t();
    let mut in_suggestion = false;
    for raw in body.split('\n') {
        let trimmed = raw.trim_end();
        if !in_suggestion && trimmed.trim_start().starts_with("```suggestion") {
            in_suggestion = true;
            out.push(Line::from(vec![
                Span::styled(pad.to_string(), Style::default()),
                Span::styled("│ ".to_string(), Style::default().fg(th.faint)),
                Span::styled(
                    "Suggested change".to_string(),
                    Style::default().fg(th.green).add_modifier(Modifier::BOLD),
                ),
            ]));
            continue;
        }
        if in_suggestion && trimmed.trim_start().starts_with("```") {
            in_suggestion = false;
            continue;
        }
        if in_suggestion {
            for wrapped in util::wrap_text(raw, inner) {
                let mut text = wrapped;
                let w = unicode_width::UnicodeWidthStr::width(text.as_str());
                if w < inner {
                    text.push_str(&" ".repeat(inner - w));
                }
                out.push(Line::from(vec![
                    Span::styled(pad.to_string(), Style::default()),
                    Span::styled("│ ".to_string(), Style::default().fg(th.green)),
                    Span::styled(text, Style::default().fg(th.text).bg(th.added_bg)),
                ]));
            }
        } else {
            for wrapped in util::wrap_text(raw, inner) {
                out.push(Line::from(vec![
                    Span::styled(pad.to_string(), Style::default()),
                    Span::styled("│ ".to_string(), Style::default().fg(th.faint)),
                    Span::styled(wrapped, Style::default().fg(th.text)),
                ]));
            }
        }
    }
}

/// Apply a key to a text editor (shared by the comment and review composers).
fn edit_key(editor: &mut Editor, key: KeyEvent, ctrl: bool) {
    match key.code {
        KeyCode::Enter => editor.insert_newline(),
        KeyCode::Char(c) if !ctrl => editor.insert_char(c),
        KeyCode::Backspace => editor.backspace(),
        KeyCode::Delete => editor.delete(),
        KeyCode::Left if ctrl => editor.word_left(),
        KeyCode::Right if ctrl => editor.word_right(),
        KeyCode::Left => editor.left(),
        KeyCode::Right => editor.right(),
        KeyCode::Up => editor.up(),
        KeyCode::Down => editor.down(),
        KeyCode::Home => editor.home(),
        KeyCode::End => editor.end(),
        KeyCode::Tab => {
            for _ in 0..4 {
                editor.insert_char(' ');
            }
        }
        _ => {}
    }
}

/// A `Request changes` review always needs a summary. A `Comment` review needs
/// one unless it carries inline comments.
pub fn review_validation(
    verdict: ReviewVerdict,
    body: &str,
    has_comments: bool,
) -> Result<(), &'static str> {
    let needs_body = match verdict {
        ReviewVerdict::Approve => false,
        ReviewVerdict::RequestChanges => true,
        ReviewVerdict::Comment => !has_comments,
    };
    if needs_body && body.trim().is_empty() {
        Err("a summary is required for this verdict")
    } else {
        Ok(())
    }
}

/// Split `@@ -a,b +c,d @@ context` into the `@@ … @@` part and the context.
pub fn split_hunk_header(header: &str) -> (&str, &str) {
    if header.starts_with("@@ ")
        && let Some(idx) = header[3..].find(" @@")
    {
        let end = 3 + idx + 3;
        return (&header[..end], header[end..].trim_start());
    }
    (header, "")
}

/// A locally created comment that has not been published yet.
fn draft_comment(body: String) -> Comment {
    Comment {
        id: 0,
        author: "you".to_string(),
        body,
        created_at: String::new(),
        original_line: None,
        diff_hunk: None,
        pending: true,
    }
}

fn pad_num(n: Option<u32>, width: u16) -> String {
    match n {
        Some(n) => format!("{n:>width$}", width = width as usize),
        None => " ".repeat(width as usize),
    }
}
