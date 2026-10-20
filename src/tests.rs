//! Headless tests that exercise parsing, anchoring and rendering.

use std::sync::mpsc::channel;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;
use crate::github;
use crate::model::{self, FileStatus};

fn fixture_app() -> App {
    let (tx, rx) = channel();
    let mut app = App::new("RIOT-OS/RIOT".to_string(), tx, rx);
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/pr22910"));
    let loaded = github::load_offline(dir).expect("load fixture");
    app.set_loaded(loaded);
    // Fixtures are read-only, so a stray Ctrl+S can never reach GitHub.
    app.offline = true;
    app
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
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

fn press(app: &mut App, c: char) {
    app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
}

fn press_ctrl(app: &mut App, c: char) {
    app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}

#[test]
fn parses_the_diff() {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/pr22910"));
    let loaded = github::load_offline(dir).unwrap();
    assert_eq!(loaded.meta.number, 22910);
    assert!(loaded.files.len() > 20, "expected many files");

    let added = loaded
        .files
        .iter()
        .filter(|f| f.status == FileStatus::Added)
        .count();
    assert!(added > 0, "expected some added files");

    let isrpipe = loaded
        .files
        .iter()
        .find(|f| f.path == "Makefile.dep")
        .expect("Makefile.dep present");
    assert_eq!(isrpipe.status, FileStatus::Modified);
    assert!(isrpipe.additions > 0 && isrpipe.deletions > 0);

    // Every hunk must carry at least one line.
    for f in &loaded.files {
        for h in &f.hunks {
            assert!(!h.lines.is_empty(), "empty hunk in {}", f.path);
        }
    }
}

#[test]
fn anchors_open_threads() {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/pr22910"));
    let loaded = github::load_offline(dir).unwrap();
    let mut threads = loaded.threads.clone();
    model::reanchor_all(&loaded.files, &mut threads);

    let open: Vec<_> = threads.iter().filter(|t| !t.resolved).collect();
    assert_eq!(open.len(), 2);
    for t in &open {
        assert!(t.anchor.is_some(), "open thread on {} not anchored", t.path);
    }

    let resolved = threads.iter().filter(|t| t.resolved).count();
    assert_eq!(resolved, 2);
}

#[test]
fn renders_open_thread_but_hides_resolved() {
    let mut app = fixture_app();
    // Select the file with the open thread.
    let idx = app
        .files
        .iter()
        .position(|f| f.path == "cpu/rpx0xx/periph_common/usb_reset.c")
        .expect("file present");
    app.select_file(idx);
    app.focus = crate::app::Focus::Diff;

    let screen = render(&mut app, 160, 50);
    println!("{screen}");

    assert!(
        screen.contains("reviewer-a"),
        "open thread author should be visible"
    );
    assert!(
        screen.contains("array-bounds"),
        "open thread body should be visible"
    );

    // The resolved thread on compat_layer.h must not be shown while its file
    // is not selected, and not shown at all with default settings.
    let resolved_idx = app
        .files
        .iter()
        .position(|f| f.path == "cpu/rpx0xx/include/compat_layer.h");
    if let Some(ri) = resolved_idx {
        app.select_file(ri);
        let screen2 = render(&mut app, 160, 50);
        assert!(
            !screen2.contains("✓ resolved"),
            "resolved thread should be hidden by default"
        );
    }
}

#[test]
fn toggling_resolved_shows_them() {
    let mut app = fixture_app();
    let ri = app
        .files
        .iter()
        .position(|f| f.path == "cpu/rpx0xx/include/compat_layer.h")
        .expect("file present");
    app.select_file(ri);
    press(&mut app, 't');
    let screen = render(&mut app, 160, 50);
    assert!(
        screen.contains("resolved"),
        "resolved threads should appear"
    );
}

#[test]
fn composing_a_comment_works() {
    let mut app = fixture_app();
    let idx = app
        .files
        .iter()
        .position(|f| f.path == "cpu/rpx0xx/periph_common/usb_reset.c")
        .unwrap();
    app.select_file(idx);
    app.focus = crate::app::Focus::Diff;

    // Move down onto a code line.
    for _ in 0..6 {
        press(&mut app, 'j');
    }
    press(&mut app, 'c');
    assert!(app.draft.is_some(), "draft should be open");

    for c in "hello".chars() {
        press(&mut app, c);
    }
    let text = app.draft.as_ref().unwrap().editor.text();
    assert_eq!(text, "hello");

    // Esc cancels without submitting.
    app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.draft.is_none());
}

#[test]
fn submitting_an_empty_comment_is_rejected() {
    let mut app = fixture_app();
    let idx = app
        .files
        .iter()
        .position(|f| f.path == "cpu/rpx0xx/periph_common/usb_reset.c")
        .unwrap();
    app.select_file(idx);
    for _ in 0..6 {
        press(&mut app, 'j');
    }
    press(&mut app, 'c');
    press_ctrl(&mut app, 's');
    let draft = app.draft.as_ref().expect("draft still open");
    assert!(draft.error.is_some());
    assert!(!draft.submitting);
}

#[test]
fn picker_and_navigation_do_not_panic() {
    let mut app = fixture_app();
    // Walk through every file and render each one.
    for i in 0..app.files.len() {
        app.select_file(i);
        let _ = render(&mut app, 120, 40);
    }
    // Exercise global keys.
    for c in ['t', ']', '[', 'G', 'g'] {
        press(&mut app, c);
    }
    let _ = render(&mut app, 80, 24);
}

/// Target for the opt-in live tests. They never fall back to a real pull
/// request: without `GHDIFF_TEST_REPO` and `GHDIFF_TEST_PR` they skip.
fn live_target() -> Option<(String, u64)> {
    let repo = std::env::var("GHDIFF_TEST_REPO")
        .ok()
        .filter(|r| !r.trim().is_empty())?;
    let number = std::env::var("GHDIFF_TEST_PR").ok()?.trim().parse().ok()?;
    Some((repo, number))
}

fn skip_live(test: &str) {
    eprintln!(
        "skipping {test}: set GHDIFF_TEST_REPO=owner/name and GHDIFF_TEST_PR=<number> to run it"
    );
}

/// End-to-end check of the write path. This talks to a real pull request, so
/// it is ignored by default. It posts a comment and a reply, then deletes both
/// again. Run it explicitly with:
///
///     GHDIFF_TEST_REPO=owner/name GHDIFF_TEST_PR=123 \
///         cargo test -- --ignored live_post_and_delete
#[test]
#[ignore = "posts to a real pull request"]
fn live_post_and_delete_comment() {
    use crate::model::LineKind;

    let Some((repo, number)) = live_target() else {
        skip_live("live_post_and_delete_comment");
        return;
    };

    let loaded = github::fetch_pr(&repo, number).expect("fetch pull request");

    // Pick a file with a run of at least three consecutive added lines so we
    // can exercise both single-line and multi-line comments.
    let mut single = None;
    let mut range = None;
    'outer: for f in &loaded.files {
        for hunk in &f.hunks {
            let run: Vec<u32> = hunk
                .lines
                .iter()
                .filter(|l| l.kind == LineKind::Added)
                .filter_map(|l| l.new_lineno)
                .collect();
            for w in run.windows(3) {
                if w[0] + 1 == w[1] && w[1] + 1 == w[2] {
                    single = Some((f.path.clone(), w[2]));
                    range = Some((f.path.clone(), w[0], w[2]));
                    break 'outer;
                }
            }
        }
    }
    let (path, line) = single.expect("a commentable line");
    println!("posting single-line comment to {path}:{line} of {repo}#{number}");

    let new = github::NewComment {
        path: &path,
        line,
        start_line: None,
        side: crate::model::Side::Right,
        commit_id: &loaded.meta.head_sha,
        body: "ghdiff automated test comment — safe to delete",
    };
    let comment = github::post_review_comment(&repo, number, &new).expect("post comment");
    assert!(comment.id != 0, "expected a comment id");
    println!("posted comment id {}", comment.id);

    let reply = github::reply_to_comment(&repo, number, comment.id, "ghdiff automated test reply")
        .expect("post reply");
    assert!(reply.id != 0);
    println!("posted reply id {}", reply.id);

    github::delete_review_comment(&repo, reply.id).expect("delete reply");
    github::delete_review_comment(&repo, comment.id).expect("delete comment");

    // Multi-line suggestion.
    let (rpath, start_line, end_line) = range.expect("a commentable range");
    println!("posting range suggestion to {rpath}:{start_line}-{end_line}");
    let range_comment = github::NewComment {
        path: &rpath,
        line: end_line,
        start_line: Some(start_line),
        side: crate::model::Side::Right,
        commit_id: &loaded.meta.head_sha,
        body: &crate::app::suggestion_body("// ghdiff automated suggestion — safe to delete"),
    };
    let mc =
        github::post_review_comment(&repo, number, &range_comment).expect("post range comment");
    assert!(mc.id != 0);
    println!("posted range comment id {}", mc.id);
    github::delete_review_comment(&repo, mc.id).expect("delete range comment");

    println!("cleaned up");
}

#[test]
fn renders_all_screens_without_panicking() {
    use crate::app::Screen;
    use crate::model::PrSummary;

    let mut app = fixture_app();

    // Picker.
    app.pr_list = vec![
        PrSummary {
            number: 1,
            title: "first".into(),
            author: None,
            head_ref: "a".into(),
            is_draft: false,
        },
        PrSummary {
            number: 2,
            title: "second".into(),
            author: None,
            head_ref: "b".into(),
            is_draft: true,
        },
    ];
    app.screen = Screen::Picker;
    let s = render(&mut app, 100, 30);
    assert!(s.contains("first") && s.contains("second"));

    // Error.
    app.screen = Screen::Error;
    app.error = "something went wrong".into();
    let s = render(&mut app, 100, 30);
    assert!(s.contains("something went wrong"));

    // Loading.
    app.screen = Screen::Loading;
    app.loading_msg = "Loading…".into();
    let _ = render(&mut app, 100, 30);

    // Main + help overlay + comment draft overlay.
    app.screen = Screen::Main;
    app.select_file(0);
    app.help = true;
    let s = render(&mut app, 120, 40);
    assert!(s.contains("help"));
    app.help = false;
    for _ in 0..4 {
        press(&mut app, 'j');
    }
    press(&mut app, 'c');
    if app.draft.is_some() {
        let s = render(&mut app, 120, 40);
        assert!(s.contains("Ctrl+S submit"));
    }
}

// ---------------------------------------------------------------------------
// Multi-line selection
// ---------------------------------------------------------------------------

use crate::app::{DraftTarget, LineMeta, RangeSel};
use crate::model::Side;
use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

fn select_and_render(app: &mut App, path: &str, w: u16, h: u16) {
    let idx = app.files.iter().position(|f| f.path == path).unwrap();
    app.select_file(idx);
    app.focus = crate::app::Focus::Diff;
    let _ = render(app, w, h);
}

#[test]
fn keyboard_range_selection_comments_multiple_lines() {
    let mut app = fixture_app();
    // usb_reset.c is a fully added file: every code line is RIGHT-side.
    select_and_render(&mut app, "cpu/rpx0xx/periph_common/usb_reset.c", 140, 40);

    // Row 0 is the hunk header; row 1 is the first code line.
    app.view.cursor = 1;
    press(&mut app, 'v'); // start the selection
    assert!(app.visual_start.is_some());
    press(&mut app, 'j');
    press(&mut app, 'j');

    let sel = app.selected_range().expect("a selection");
    assert_eq!(sel.rows, vec![1, 2, 3]);
    assert_eq!(sel.side, Side::Right);
    assert_eq!(sel.start_line, 1);
    assert_eq!(sel.line, 3);

    press(&mut app, 'c');
    match app.draft.as_ref().map(|d| &d.target) {
        Some(DraftTarget::NewLine {
            start_line,
            line,
            side,
            ..
        }) => {
            assert_eq!(*start_line, Some(1));
            assert_eq!(*line, 3);
            assert_eq!(*side, Side::Right);
        }
        other => panic!("expected a range draft, got {other:?}"),
    }
    // Starting a comment clears the selection.
    assert!(app.visual_start.is_none());
}

#[test]
fn single_line_selection_is_a_plain_comment() {
    let mut app = fixture_app();
    select_and_render(&mut app, "cpu/rpx0xx/periph_common/usb_reset.c", 140, 40);
    app.view.cursor = 1;
    press(&mut app, 'v');
    press(&mut app, 'c');
    match app.draft.as_ref().map(|d| &d.target) {
        Some(DraftTarget::NewLine {
            start_line, line, ..
        }) => {
            assert_eq!(*start_line, None, "a one-line selection is not a range");
            assert_eq!(*line, 1);
        }
        other => panic!("expected a single-line draft, got {other:?}"),
    }
}

#[test]
fn range_stops_at_a_side_change() {
    let mut app = fixture_app();
    // Makefile.dep contains a removed line immediately followed by an added one.
    select_and_render(&mut app, "Makefile.dep", 140, 40);

    let removed_row = app
        .view
        .meta
        .iter()
        .position(|m| {
            matches!(
                m,
                LineMeta::Code {
                    kind: crate::model::LineKind::Removed,
                    ..
                }
            )
        })
        .expect("a removed line");
    let added_row = app
        .view
        .meta
        .iter()
        .position(|m| {
            matches!(
                m,
                LineMeta::Code {
                    kind: crate::model::LineKind::Added,
                    ..
                }
            )
        })
        .expect("an added line");

    // Select from the removed line down to the added line.
    app.visual_start = Some(removed_row);
    app.view.cursor = added_row;

    let sel = app.selected_range().expect("a selection");
    assert_eq!(sel.side, Side::Left, "started on a removed line");
    assert_eq!(
        sel.rows,
        vec![removed_row],
        "must not cross to the other side"
    );
    assert_eq!(sel.start_line, sel.line);
}

// ---------------------------------------------------------------------------
// Mouse support
// ---------------------------------------------------------------------------

fn click(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[test]
fn mouse_click_moves_the_cursor_in_the_diff() {
    let mut app = fixture_app();
    select_and_render(&mut app, "cpu/rpx0xx/periph_common/usb_reset.c", 140, 40);

    let area = app.diff_area;
    let target_y = area.y + 5;
    app.on_mouse(click(
        MouseEventKind::Down(MouseButton::Left),
        area.x + 10,
        target_y,
    ));
    app.on_mouse(click(
        MouseEventKind::Up(MouseButton::Left),
        area.x + 10,
        target_y,
    ));

    assert_eq!(app.focus, crate::app::Focus::Diff);
    assert_eq!(app.view.cursor, app.view.scroll as usize + 5);
    // A press without a drag is not a selection.
    assert!(app.visual_start.is_none());
}

#[test]
fn mouse_click_selects_a_file_in_the_sidebar() {
    let mut app = fixture_app();
    select_and_render(&mut app, "cpu/rpx0xx/periph_common/usb_reset.c", 140, 40);
    assert_eq!(
        app.file_index,
        app.files
            .iter()
            .position(|f| f.path.ends_with("usb_reset.c"))
            .unwrap()
    );

    let side = app.sidebar_area;
    // Third list row (offset 0), inside the border.
    let y = side.y + 1 + 2;
    app.on_mouse(click(
        MouseEventKind::Down(MouseButton::Left),
        side.x + 3,
        y,
    ));
    assert_eq!(app.focus, crate::app::Focus::Files);
    assert_eq!(app.file_index, 2);
}

#[test]
fn mouse_drag_selects_a_range() {
    let mut app = fixture_app();
    select_and_render(&mut app, "cpu/rpx0xx/periph_common/usb_reset.c", 140, 40);
    let area = app.diff_area;

    let from = area.y + 1; // first code line
    let to = area.y + 3;
    app.on_mouse(click(
        MouseEventKind::Down(MouseButton::Left),
        area.x + 5,
        from,
    ));
    app.on_mouse(click(
        MouseEventKind::Drag(MouseButton::Left),
        area.x + 5,
        to,
    ));
    app.on_mouse(click(MouseEventKind::Up(MouseButton::Left), area.x + 5, to));

    let sel: RangeSel = app.selected_range().expect("a dragged selection");
    assert_eq!(sel.rows, vec![1, 2, 3]);
    assert!(app.visual_start.is_some());
}

#[test]
fn mouse_wheel_scrolls_the_diff() {
    let mut app = fixture_app();
    select_and_render(&mut app, "cpu/rpx0xx/periph_common/usb_reset.c", 140, 40);
    app.view.cursor = 10;
    let area = app.diff_area;
    app.on_mouse(click(MouseEventKind::ScrollDown, area.x + 5, area.y + 5));
    assert_eq!(app.view.cursor, 13);
    app.on_mouse(click(MouseEventKind::ScrollUp, area.x + 5, area.y + 5));
    assert_eq!(app.view.cursor, 10);
}

#[test]
fn renders_a_multiline_range_badge() {
    use crate::model::{Anchor, Comment, ReviewThread};

    let mut app = fixture_app();
    let path = "cpu/rpx0xx/periph_common/usb_reset.c";
    app.threads.push(ReviewThread {
        id: "PRRT_test".into(),
        path: path.to_string(),
        line: Some(4),
        start_line: Some(2),
        side: Side::Right,
        resolved: false,
        outdated: false,
        comments: vec![Comment {
            id: 1,
            author: "reviewer".into(),
            body: "this whole block".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            original_line: Some(4),
            diff_hunk: None,
            pending: false,
        }],
        anchor: Some(Anchor { hunk: 0, line: 3 }),
    });
    app.view_dirty = true;
    select_and_render(&mut app, path, 140, 40);

    let screen = render(&mut app, 140, 40);
    assert!(screen.contains("L2–4"), "expected a range badge:\n{screen}");
}

// ---------------------------------------------------------------------------
// Theming
// ---------------------------------------------------------------------------

use crate::theme::Theme;
use ratatui::style::Color;

fn luma(c: Color) -> f32 {
    match c {
        Color::Rgb(r, g, b) => 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32,
        _ => 128.0,
    }
}

#[test]
fn light_and_dark_themes_have_matching_tints() {
    let light = Theme::new(crate::termbg::Mode::Light, Some((255, 255, 255)));
    let dark = Theme::new(crate::termbg::Mode::Dark, Some((13, 17, 23)));

    assert_eq!(light.syntax_theme, "InspiredGitHub");
    assert_eq!(dark.syntax_theme, "base16-ocean.dark");

    // Tints must suit the background they sit on.
    assert!(
        luma(light.added_bg) > 180.0,
        "light added: {:?}",
        light.added_bg
    );
    assert!(
        luma(light.removed_bg) > 180.0,
        "light removed: {:?}",
        light.removed_bg
    );
    assert!(
        luma(dark.added_bg) < 80.0,
        "dark added: {:?}",
        dark.added_bg
    );
    assert!(
        luma(dark.removed_bg) < 80.0,
        "dark removed: {:?}",
        dark.removed_bg
    );

    // The base blends with the terminal rather than forcing a colour.
    assert_eq!(light.bg, Color::Reset);
    assert_eq!(light.text, Color::Reset);

    // Accents come from the terminal palette.
    assert_eq!(light.green, Color::Green);
    assert_eq!(light.red, Color::Red);

    // The hunk band is tinted and its accent differs per mode.
    assert!(matches!(light.hunk_bg, Color::Rgb(..)));
    assert!(matches!(dark.hunk_bg, Color::Rgb(..)));
    assert_ne!(light.hunk_fg, dark.hunk_fg);
}

#[test]
fn added_rows_are_tinted_according_to_the_theme() {
    fn bg_at_first_added_row(app: &mut App) -> Color {
        let backend = TestBackend::new(140, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let y = app.diff_area.y + 1; // row 0 is the hunk header
        terminal.backend().buffer()[(app.diff_area.x + 20, y)].bg
    }

    crate::theme::set(crate::termbg::Mode::Light, Some((255, 255, 255)));
    let mut light_app = fixture_app();
    select_and_render(
        &mut light_app,
        "cpu/rpx0xx/periph_common/usb_reset.c",
        140,
        40,
    );
    let light_bg = bg_at_first_added_row(&mut light_app);

    crate::theme::set(crate::termbg::Mode::Dark, Some((13, 17, 23)));
    let mut dark_app = fixture_app();
    select_and_render(
        &mut dark_app,
        "cpu/rpx0xx/periph_common/usb_reset.c",
        140,
        40,
    );
    let dark_bg = bg_at_first_added_row(&mut dark_app);

    // Restore the default for other tests.
    crate::theme::set(crate::termbg::Mode::Dark, None);

    assert!(
        luma(light_bg) > 180.0,
        "expected a light tint, got {light_bg:?}"
    );
    assert!(
        luma(dark_bg) < 80.0,
        "expected a dark tint, got {dark_bg:?}"
    );
}

// ---------------------------------------------------------------------------
// Suggestion mode
// ---------------------------------------------------------------------------

#[test]
fn suggestion_prefills_the_current_line() {
    let mut app = fixture_app();
    select_and_render(&mut app, "cpu/rpx0xx/periph_common/usb_reset.c", 140, 40);
    app.view.cursor = 1; // first code line, an added line

    press(&mut app, 's');
    match app.draft.as_ref().map(|d| &d.target) {
        Some(DraftTarget::Suggestion {
            start_line, line, ..
        }) => {
            assert_eq!(*start_line, None);
            assert_eq!(*line, 1);
        }
        other => panic!("expected a suggestion draft, got {other:?}"),
    }
    let text = app.draft.as_ref().unwrap().editor.text();
    assert_eq!(text, "/*", "editor should be pre-filled with the line");
}

#[test]
fn suggestion_prefills_a_selected_range() {
    let mut app = fixture_app();
    select_and_render(&mut app, "cpu/rpx0xx/periph_common/usb_reset.c", 140, 40);

    app.view.cursor = 1;
    press(&mut app, 'v');
    press(&mut app, 'j');
    press(&mut app, 'j');
    press(&mut app, 's');

    match app.draft.as_ref().map(|d| &d.target) {
        Some(DraftTarget::Suggestion {
            start_line, line, ..
        }) => {
            assert_eq!(*start_line, Some(1));
            assert_eq!(*line, 3);
        }
        other => panic!("expected a range suggestion, got {other:?}"),
    }
    let text = app.draft.as_ref().unwrap().editor.text();
    assert_eq!(text.lines().count(), 3);
    assert_eq!(
        text,
        "/*\n * SPDX-FileCopyrightText: 2026 Example University\n * SPDX-License-Identifier: LGPL-2.1-only"
    );
}

#[test]
fn suggestion_body_is_a_fenced_block() {
    let body = crate::app::suggestion_body("let x = 1;");
    assert_eq!(body, "```suggestion\nlet x = 1;\n```");
    assert!(body.starts_with("```suggestion\n"));
    assert!(body.ends_with("\n```"));
}

#[test]
fn suggestion_is_refused_on_a_removed_line() {
    let mut app = fixture_app();
    select_and_render(&mut app, "Makefile.dep", 140, 40);
    let removed_row = app
        .view
        .meta
        .iter()
        .position(|m| {
            matches!(
                m,
                LineMeta::Code {
                    kind: crate::model::LineKind::Removed,
                    ..
                }
            )
        })
        .expect("a removed line");
    app.view.cursor = removed_row;

    press(&mut app, 's');
    assert!(
        app.draft.is_none(),
        "no suggestion draft for a removed line"
    );
    assert!(app.status.contains("added or context"));
}

#[test]
fn renders_existing_suggestions_specially() {
    use crate::model::{Anchor, Comment, ReviewThread};

    let mut app = fixture_app();
    let path = "cpu/rpx0xx/periph_common/usb_reset.c";
    app.threads.push(ReviewThread {
        id: "PRRT_test".into(),
        path: path.to_string(),
        line: Some(2),
        start_line: None,
        side: Side::Right,
        resolved: false,
        outdated: false,
        comments: vec![Comment {
            id: 7,
            author: "reviewer".into(),
            body: crate::app::suggestion_body(" * SPDX-License-Identifier: MIT"),
            created_at: "2026-01-01T00:00:00Z".into(),
            original_line: Some(2),
            diff_hunk: None,
            pending: false,
        }],
        anchor: Some(Anchor { hunk: 0, line: 1 }),
    });
    app.view_dirty = true;
    select_and_render(&mut app, path, 140, 40);

    let screen = render(&mut app, 140, 40);
    assert!(screen.contains("Suggested change"), "\n{screen}");
    assert!(
        screen.contains("SPDX-License-Identifier: MIT"),
        "\n{screen}"
    );
    // The raw fence must not leak into the rendering.
    assert!(!screen.contains("```suggestion"), "\n{screen}");
}

// ---------------------------------------------------------------------------
// Review submission
// ---------------------------------------------------------------------------

use crate::github::ReviewVerdict;

fn tab(app: &mut App) {
    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
}

#[test]
fn review_composer_opens_and_cycles_the_verdict() {
    let mut app = fixture_app();
    assert!(app.review_draft.is_none());

    press(&mut app, 'S');
    let review = app.review_draft.as_ref().expect("review composer");
    assert_eq!(review.verdict, ReviewVerdict::Comment);

    tab(&mut app);
    assert_eq!(
        app.review_draft.as_ref().unwrap().verdict,
        ReviewVerdict::Approve
    );
    tab(&mut app);
    assert_eq!(
        app.review_draft.as_ref().unwrap().verdict,
        ReviewVerdict::RequestChanges
    );
    tab(&mut app);
    assert_eq!(
        app.review_draft.as_ref().unwrap().verdict,
        ReviewVerdict::Comment
    );

    // Esc closes it.
    app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.review_draft.is_none());
}

#[test]
fn review_validation_depends_on_the_verdict() {
    use crate::app::review_validation;
    assert!(review_validation(ReviewVerdict::Approve, "", false).is_ok());
    assert!(review_validation(ReviewVerdict::Approve, "   ", false).is_ok());
    assert!(review_validation(ReviewVerdict::Comment, "", false).is_err());
    assert!(review_validation(ReviewVerdict::Comment, "lgtm", false).is_ok());
    assert!(review_validation(ReviewVerdict::RequestChanges, "  ", false).is_err());
    assert!(review_validation(ReviewVerdict::RequestChanges, "please fix", false).is_ok());
}

#[test]
fn review_composer_requires_a_summary_for_comment() {
    let mut app = fixture_app();
    press(&mut app, 'S');
    press_ctrl(&mut app, 's');

    let review = app.review_draft.as_ref().expect("still open");
    assert!(review.error.is_some());
    assert!(!review.submitting);

    // Switching to Approve with an empty body is allowed, but the app is
    // offline so it is refused before any network call.
    tab(&mut app);
    press_ctrl(&mut app, 's');
    let review = app.review_draft.as_ref().expect("still open");
    assert!(
        review.error.as_deref().unwrap_or("").contains("offline"),
        "expected the offline guard, got {:?}",
        review.error
    );
    assert!(!review.submitting);
}

#[test]
fn renders_the_review_composer() {
    let mut app = fixture_app();
    app.select_file(0);
    press(&mut app, 'S');
    for c in "looks good".chars() {
        press(&mut app, c);
    }
    tab(&mut app); // Approve

    let screen = render(&mut app, 120, 40);
    assert!(screen.contains("submit review"), "\n{screen}");
    assert!(screen.contains("Comment"), "\n{screen}");
    assert!(screen.contains("Approve"), "\n{screen}");
    assert!(screen.contains("Request changes"), "\n{screen}");
    assert!(screen.contains("looks good"), "\n{screen}");
    assert!(screen.contains("Tab verdict"), "\n{screen}");
}

// ---------------------------------------------------------------------------
// Changes since the last review
// ---------------------------------------------------------------------------

#[test]
fn builds_a_file_diff_from_a_compare_entry() {
    let patch = "@@ -1,3 +1,4 @@\n context\n-old1\n-old2\n+new1\n+new2\n+new3\n";
    let fd = github::build_file_diff("src/x.c", "modified", None, Some(patch), 3, 2);
    assert_eq!(fd.path, "src/x.c");
    assert_eq!(fd.status, FileStatus::Modified);
    assert_eq!(fd.additions, 3);
    assert_eq!(fd.deletions, 2);
    assert_eq!(fd.hunks.len(), 1);
    let lines = &fd.hunks[0].lines;
    assert_eq!(lines.len(), 6);
    assert_eq!(lines[0].old_lineno, Some(1));
    assert_eq!(lines[0].new_lineno, Some(1));
    // First removed line.
    assert_eq!(lines[1].old_lineno, Some(2));
    assert_eq!(lines[1].new_lineno, None);
    // First added line.
    assert_eq!(lines[3].old_lineno, None);
    assert_eq!(lines[3].new_lineno, Some(2));
    // Last added line.
    assert_eq!(lines[5].new_lineno, Some(4));

    let added = github::build_file_diff(
        "new.c",
        "added",
        None,
        Some("@@ -0,0 +1,2 @@\n+a\n+b\n"),
        2,
        0,
    );
    assert_eq!(added.status, FileStatus::Added);
    assert!(added.old_path.is_none());
    assert_eq!(added.hunks[0].lines[0].old_lineno, None);
    assert_eq!(added.hunks[0].lines[0].new_lineno, Some(1));

    let binary = github::build_file_diff("logo.png", "modified", None, None, 0, 0);
    assert!(binary.binary);
    assert!(binary.hunks.is_empty());
}

#[test]
fn since_review_filters_files_and_restores() {
    let mut app = fixture_app();
    let full_len = app.files.len();
    assert!(full_len > 2);
    assert!(!app.since_review);

    let subset: Vec<_> = app.full_files.iter().take(2).cloned().collect();
    let review = crate::github::LastReview {
        sha: "e30a35f5e30a35f5".into(),
        submitted_at: "2026-10-17T10:00:00Z".into(),
        state: "COMMENTED".into(),
    };
    app.enter_since_review(review, subset);

    assert!(app.since_review);
    assert_eq!(app.files.len(), 2);
    let screen = render(&mut app, 120, 30);
    assert!(screen.contains("since last review"), "\n{screen}");
    assert!(app.status.contains("commented"), "{}", app.status);

    app.exit_since_review();
    assert!(!app.since_review);
    assert_eq!(app.files.len(), full_len);
    let screen = render(&mut app, 120, 30);
    assert!(!screen.contains("since last review"), "\n{screen}");
}

/// Read-only: exercises the "since last review" lookup against GitHub.
#[test]
#[ignore = "talks to GitHub"]
fn live_since_last_review() {
    let Some((repo, number)) = live_target() else {
        skip_live("live_since_last_review");
        return;
    };

    let meta = github::fetch_pr(&repo, number).expect("fetch");
    let result =
        github::diff_since_last_review(&repo, number, &meta.meta.head_sha).expect("lookup");
    match result {
        Some((review, files)) => {
            println!(
                "last review: {} at {} ({} files changed since)",
                review.state,
                review.submitted_at,
                files.len()
            );
            for f in files.iter().take(10) {
                println!("  {} +{} -{}", f.path, f.additions, f.deletions);
            }
        }
        None => println!("no review by the viewer"),
    }
}

// ---------------------------------------------------------------------------
// Drafts (pending review)
// ---------------------------------------------------------------------------

#[test]
fn comments_are_held_as_drafts_until_the_review_is_submitted() {
    let mut app = fixture_app();
    select_and_render(&mut app, "cpu/rpx0xx/periph_common/usb_reset.c", 140, 40);
    app.view.cursor = 1;

    press(&mut app, 'c');
    for c in "needs work".chars() {
        press(&mut app, c);
    }
    press_ctrl(&mut app, 's');

    assert!(app.draft.is_none(), "the composer should close");
    assert_eq!(app.pending_count(), 1);
    assert!(app.threads.iter().any(|t| t.id.starts_with("draft-")));
    assert!(app.status.contains("Draft saved"), "{}", app.status);

    // The draft is rendered inline, marked as a draft.
    let screen = render(&mut app, 140, 40);
    assert!(screen.contains("needs work"), "\n{screen}");
    assert!(screen.contains("draft"), "\n{screen}");

    // A Comment review with inline comments does not need a summary.
    assert!(crate::app::review_validation(ReviewVerdict::Comment, "", true).is_ok());
}

#[test]
fn suggestions_are_also_drafts() {
    let mut app = fixture_app();
    select_and_render(&mut app, "cpu/rpx0xx/periph_common/usb_reset.c", 140, 40);
    app.view.cursor = 1;
    press(&mut app, 's');
    press_ctrl(&mut app, 's');
    assert_eq!(app.pending_count(), 1);
    let thread = app
        .threads
        .iter()
        .find(|t| t.id.starts_with("draft-"))
        .expect("draft thread");
    assert!(thread.comments[0].body.starts_with("```suggestion"));
}

#[test]
fn a_draft_can_be_discarded() {
    let mut app = fixture_app();
    select_and_render(&mut app, "cpu/rpx0xx/periph_common/usb_reset.c", 140, 40);
    app.view.cursor = 1;
    press(&mut app, 'c');
    press(&mut app, 'x');
    press_ctrl(&mut app, 's');
    assert_eq!(app.pending_count(), 1);

    let row = app
        .view
        .meta
        .iter()
        .position(|m| matches!(m, LineMeta::Comment { .. }))
        .expect("a comment row");
    app.view.cursor = row;
    press(&mut app, 'd');
    assert_eq!(app.pending_count(), 0);
    assert!(app.status.contains("discarded"), "{}", app.status);
}

#[test]
fn replies_are_buffered_as_drafts() {
    let mut app = fixture_app();
    // usb_reset.c carries an existing thread on line 42.
    select_and_render(&mut app, "cpu/rpx0xx/periph_common/usb_reset.c", 140, 40);
    let row = app
        .view
        .meta
        .iter()
        .position(|m| matches!(m, LineMeta::Comment { .. }))
        .expect("a thread row");
    app.view.cursor = row;

    press(&mut app, 'r');
    assert!(app.draft.is_some());
    for c in "ok".chars() {
        press(&mut app, c);
    }
    press_ctrl(&mut app, 's');

    assert!(app.draft.is_none());
    assert_eq!(app.pending_count(), 1);
    // The reply lives on the existing thread, not a new draft thread.
    assert!(!app.threads.iter().any(|t| t.id.starts_with("draft-")));
}

#[test]
fn replying_to_a_draft_is_refused() {
    let mut app = fixture_app();
    select_and_render(&mut app, "cpu/rpx0xx/periph_common/usb_reset.c", 140, 40);
    app.view.cursor = 1;
    press(&mut app, 'c');
    press(&mut app, 'x');
    press_ctrl(&mut app, 's');

    // The cursor jumped to the new draft thread.
    press(&mut app, 'r');
    assert!(app.draft.is_none());
    assert!(app.status.contains("still a draft"), "{}", app.status);
}

/// Reversible live check of the review payload: creates a *pending* review
/// (which is invisible to others), verifies its comments, then deletes it.
#[test]
#[ignore = "talks to GitHub"]
fn live_pending_review_roundtrip() {
    use crate::github::PendingComment;
    use crate::model::LineKind;

    let Some((repo, number)) = live_target() else {
        skip_live("live_pending_review_roundtrip");
        return;
    };
    let loaded = github::fetch_pr(&repo, number).expect("fetch");

    // A single-line comment plus a multi-line suggestion.
    let mut single = None;
    let mut range = None;
    'outer: for f in &loaded.files {
        for hunk in &f.hunks {
            let run: Vec<u32> = hunk
                .lines
                .iter()
                .filter(|l| l.kind == LineKind::Added)
                .filter_map(|l| l.new_lineno)
                .collect();
            for w in run.windows(3) {
                if w[0] + 1 == w[1] && w[1] + 1 == w[2] {
                    single = Some((f.path.clone(), w[2]));
                    range = Some((f.path.clone(), w[0], w[2]));
                    break 'outer;
                }
            }
        }
    }
    let (path, line) = single.expect("a commentable line");
    let (rpath, start, end) = range.expect("a commentable range");

    let comments = vec![
        PendingComment {
            path: path.clone(),
            line,
            start_line: None,
            side: crate::model::Side::Right,
            body: "ghdiff pending review test — safe to delete".into(),
        },
        PendingComment {
            path: rpath,
            line: end,
            start_line: Some(start),
            side: crate::model::Side::Right,
            body: crate::app::suggestion_body("// ghdiff suggestion"),
        },
    ];

    let review_id = github::create_review(
        &repo,
        number,
        &loaded.meta.head_sha,
        None, // pending
        "ghdiff pending review test",
        &comments,
    )
    .expect("create pending review");
    println!("created pending review {review_id}");

    let count = github::review_comment_count(&repo, number, review_id).expect("count comments");
    println!("pending review has {count} comments");
    assert_eq!(count, 2, "both comments should be attached");

    github::delete_pending_review(&repo, number, review_id).expect("delete pending review");
    println!("deleted pending review {review_id}");
}

#[test]
fn footer_shows_the_current_line_length() {
    let mut app = fixture_app();
    select_and_render(&mut app, "cpu/rpx0xx/periph_common/usb_reset.c", 140, 40);

    // First code line is `/*`.
    app.view.cursor = 1;
    let screen = render(&mut app, 140, 40);
    assert!(screen.contains("Ln 1"), "\n{screen}");
    assert!(screen.contains("2 chars"), "\n{screen}");

    // A hunk header has no readout.
    app.view.cursor = 0;
    let screen = render(&mut app, 140, 40);
    assert!(!screen.contains(" chars"), "\n{screen}");

    // A long line reports its length too.
    app.view.cursor = 2;
    let screen = render(&mut app, 140, 40);
    assert!(screen.contains("Ln 2"), "\n{screen}");
    assert!(screen.contains("chars"), "\n{screen}");
}

#[test]
fn hunk_headers_stand_out_from_context() {
    let mut app = fixture_app();
    select_and_render(&mut app, "Makefile.dep", 140, 40);

    let backend = TestBackend::new(140, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let buf = terminal.backend().buffer();

    // Row 0 of the diff is the hunk header, row 1 the first context line.
    let x = app.diff_area.x + 30;
    let hunk = buf[(x, app.diff_area.y)].bg;
    let context = buf[(x, app.diff_area.y + 1)].bg;
    assert!(
        matches!(hunk, Color::Rgb(..)),
        "the hunk band should be a tinted row, got {hunk:?}"
    );
    assert!(
        (luma(hunk) - luma(context)).abs() > 20.0,
        "the hunk band should stand out: {hunk:?} vs {context:?}"
    );

    // The `⋯` marker is drawn in the hunk accent colour.
    let mut marker = None;
    for i in 0..app.diff_area.width {
        let cell = &buf[(app.diff_area.x + i, app.diff_area.y)];
        if cell.symbol() == "⋯" {
            marker = Some(cell.fg);
        }
    }
    let fg = marker.expect("expected a ⋯ marker in the hunk header");
    assert!(matches!(fg, Color::Rgb(..)), "marker colour was {fg:?}");
}

#[test]
fn hunk_header_splits_ranges_from_context() {
    use crate::app::split_hunk_header as split;
    assert_eq!(
        split("@@ -2,6 +2,7 @@ MODULE = $(APPLICATION_MODULE)"),
        ("@@ -2,6 +2,7 @@", "MODULE = $(APPLICATION_MODULE)")
    );
    assert_eq!(split("@@ -1 +1 @@"), ("@@ -1 +1 @@", ""));
    // A header without the trailing marker is returned unchanged.
    assert_eq!(split("@@ -1 +1"), ("@@ -1 +1", ""));
}
