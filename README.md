# ghdiff

A local terminal viewer for GitHub pull requests, built with Rust and
[ratatui](https://ratatui.rs). It aims to feel like the GitHub "Files changed"
tab without leaving your terminal.

```
┌ ghdiff ───────────────────────────────────────────────────────────────────────────┐
│#22910 cpu/rpxxxx: add usbdev support                                              │
└───────────────────────────────────────────────────────────────────────────────────┘
┌ 33 changed files ──────────────────────────┐ M makefiles/application.inc.mk +1 -0  │
│  M Makefile.dep                       +2 -1│▸        @@ -2,6 +2,7 @@ MODULE = ... │
│  M Makefile.include                   +3 -0│  2   2                                 │
│  A cpu/Makefile                       +6 -0│  3   3  DIRS += $(RIOTCPU)/$(CPU) ...  │
│  A cpu/Makefile.dep                   +4 -0│  4   4  DIRS += $(RIOTBASE)/boards     │
│  A cpu/Makefile.features              +1 -0│●     5+ DIRS += $(RIOTBASE)/cpu        │
│  M cpu/rp2350_common/clock.c         +28 -0│         ╭─ @reviewer-a · 2026-10-17      │
│  A cpu/rpx0xx/include/compat_layer.h +89 -0│         │ 🤨  Whats happening here      │
│  A cpu/rpx0xx/periph_common/usb_… ●1 +89 -0│         ╰─                             │
│▸ M makefiles/application.inc.mk ●1    +1 -0│  5   6  DIRS += $(RIOTBASE)/core ...    │
└────────────────────────────────────────────┘                                       │
 Tab focus · j/k move · c comment · r reply · t resolved · [ ] file · q quit         │
```

(The real thing is colourised: syntax-highlighted code, green/red added and
removed rows, and purple thread markers in the gutter. A full-size snapshot is
in [`docs/layout.txt`](docs/layout.txt).)

## Features

- **File list on the left** with status letters (`A`/`M`/`D`/`R`/`C`), per-file
  `+adds -dels`, and a `●n` badge for files that carry review threads.
- **Unified diff on the right** with old/new line numbers, `+`/`-` gutters and
  GitHub-style added/removed row tinting.
- **Syntax highlighting** for the diffed code, driven by the file type
  (via [syntect](https://crates.io/crates/syntect)).
- **Existing review comments are shown inline**, attached to the line they were
  made on, with all replies in the thread. **Resolved threads are hidden** by
  default (press `t` to reveal them).
- **Write review comments and replies** straight from the diff. They are kept
  as **drafts** and only published when you submit your review.
- **Multi-line comments** — press `v` and move to select a range (or drag with
  the mouse), then `c` to comment on the whole range.
- **Code suggestions** — press `s` to rewrite the current line (or selected
  range) in a small editor; it is posted as a GitHub `suggestion` block that
  reviewers can apply with one click. Existing suggestions from others are
  rendered as a tinted "Suggested change" block.
- **Submit a review** — press `S` to write a summary and pick a verdict
  (**Comment**, **Approve** or **Request changes**), then submit it to the pull
  request.
- **Changes since your last review** — press `L` to show only the files and
  hunks that changed since the commit of your most recent review, then press
  `L` again to go back to the full diff.
- **Theme-aware colours** — the palette is derived from your terminal's
  background (detected with the OSC 11 query), so it matches light *and* dark
  alacritty/kitty/wezterm themes. Force it with `--theme`.
- **Line readout** — the bottom-right corner shows the line number and length
  of the line under the cursor, VS Code style (`Ln 42 · 87 chars`).
- **Unmissable hunk headers** — where a patch jumps to a different part of the
  file, the `@@ … @@` line is drawn as a tinted band with an emphasised range
  and a `⋯` marker, so it is hard to read straight past.
- **Mouse support** — click to move the cursor or pick a file, drag or
  shift-click to select a range, and scroll the pane under the pointer.
- **Outdated threads** (whose line no longer exists) are matched back to the
  current diff by content, or listed at the end of the file.

## Requirements

- A recent Rust toolchain (`cargo`).
- The [GitHub CLI](https://cli.github.com/) (`gh`), authenticated
  (`gh auth login`). `ghdiff` shells out to `gh` for all API access, so it
  inherits your existing authentication and host configuration.

## Build

```sh
cargo build --release
# binary at target/release/ghdiff
```

Optionally put it on your `PATH`:

```sh
cargo install --path .
```

## Usage

Run it from inside a checkout of the repository you want to review:

```sh
cd ~/src/RIOT
ghdiff 22910          # open pull request #22910
```

`ghdiff` works out the repository from the current directory (`gh repo view`),
which also resolves forks to their upstream. You can also pass a repository
explicitly:

```sh
ghdiff 22910 --repo RIOT-OS/RIOT
```

With no pull request number, an interactive picker of open pull requests is
shown:

```sh
ghdiff
```

### Keys

| Key                | Action                                          |
| ------------------ | ----------------------------------------------- |
| `Tab`              | switch focus between the file list and the diff |
| `j` / `k`          | move down / up                                  |
| `g` / `G`          | jump to top / bottom                            |
| `Ctrl+d` / `Ctrl+u`| half page down / up                             |
| `[` / `]`          | previous / next file                            |
| `Enter`            | open the selected file (in the file list)       |
| `c`                | comment on the current line (or the selected range) |
| `s`                | suggest a code change for the line / selection  |
| `d`                | discard the draft comment under the cursor      |
| `v` / `V`          | start a range selection, then `c` to comment on it |
| `r`                | reply to the thread under the cursor            |
| `Ctrl+S`           | submit the comment being written (`Esc` cancels)|
| `t`                | toggle display of resolved threads              |
| `o`                | open the pull request in a browser              |
| `S`                | submit your review, publishing every draft      |
| `L`                | toggle "changes since your last review"         |
| `R`                | reload the pull request                         |
| `?`                | show help                                       |
| `q`                | quit                                            |

While writing a comment, `Enter` inserts a newline, `Ctrl+S` submits and `Esc`
cancels.

### Suggestions

Press `s` on a line — or on a range selected with `v`/drag — to open a
suggestion editor pre-filled with the current code. Rewrite it, then `Ctrl+S`
to keep it as a draft. `ghdiff` wraps the result in a GitHub `suggestion` fence
anchored to the same range, so once published the reviewer sees an **Apply
suggestion** button:

````markdown
```suggestion
 * SPDX-License-Identifier: MIT
```
````

Suggestions can only target added or context lines (the new side of the diff),
matching GitHub's own rules.

### Drafts and submitting a review

Nothing you write is sent to GitHub until you submit a review. `c`, `s` and `r`
all add **drafts**:

- a draft comment or suggestion appears inline in the diff, headed `@you · draft`
  and marked with a `●` in the gutter;
- the header shows how many drafts are pending (e.g. `· 2 draft comments`), and
  the review composer repeats the count in its title;
- `d` on a draft discards it.

Press `S` to open the review composer. Write a summary, use `Tab` to switch the
verdict between **Comment**, **Approve** and **Request changes**, then `Ctrl+S`
to submit. At that moment everything is published in one go: a single
`POST /pulls/{n}/reviews` carries the summary, the verdict and all draft
comments (replies to existing threads are posted alongside), and the view
reloads so they appear as real threads.

A `Request changes` review always needs a summary; a `Comment` review needs one
only when it has no inline comments; `Approve` may be empty. (GitHub will refuse
an approval on your own pull request.)

### Changes since your last review

Press `L` to narrow the view to what has happened since your most recent review
on the pull request — the same idea as GitHub's "Changes since your last
review" dropdown. `ghdiff` finds your latest submitted review, takes the commit
it was made against, and diffs that against the current head, so only the files
and hunks that changed since are shown. The header gains a `· since last review`
marker and the status bar records which review is being compared against.

Press `L` again to restore the full diff. If you have not reviewed the pull
request yet, a status message says so.

### Theming

The colour scheme follows your terminal. `ghdiff` asks the terminal for its
background colour (OSC 11), classifies it as light or dark, and builds the
palette from there:

- the base background and default text use the terminal's own colours, so the
  UI blends into whatever theme the terminal is using;
- accent colours use the terminal's ANSI palette;
- the added/removed tints are blended from the detected background, so they
  stay subtle on both light and dark themes;
- the syntax-highlighting theme switches between `InspiredGitHub` (light) and
  `base16-ocean.dark` (dark).

Override the detection with `--theme` (or the `GHDIFF_THEME` environment
variable):

```sh
ghdiff 22910 --theme light
ghdiff 22910 --theme dark
ghdiff 22910 --theme auto      # the default
```

Detection happens once at startup and waits briefly (a few tens of
milliseconds) for the terminal to answer; if it never does, `ghdiff` falls back
to `COLORFGBG` and finally to a dark theme.

### Mouse

| Action       | Effect                                            |
| ------------ | ------------------------------------------------- |
| click        | move the cursor, or select a file in the sidebar  |
| drag         | select a range of lines to comment on             |
| shift+click  | extend the selection from the cursor              |
| wheel        | scroll the pane under the pointer                 |

## Offline mode

For development or when you have no network, you can render a saved pull
request from a directory containing `meta.json`, `diff.patch` and
`threads.json`:

```sh
ghdiff --offline fixtures/pr22910
```

Offline mode is read-only: comments are not posted. `--dump` prints a single
rendered frame to stdout instead of starting the TUI, which is handy for
scripting and for inspecting the layout:

```sh
ghdiff 22910 --dump --file usb_reset.c
```

## How it works

- The pull request metadata comes from `gh pr view`.
- The diff comes from `gh pr diff` and is parsed by a small unified-diff parser
  (`src/model.rs`).
- Review threads — including their **resolved** state, which the REST API does
  not expose — come from the `reviewThreads` GraphQL connection.
- Comments and suggestions are collected as drafts and published together with
  the review: one `POST /pulls/{n}/reviews` carries `commit_id`, `event`, `body`
  and a `comments` array (`path`, `line`, `side`, and `start_line`/`start_side`
  for ranges and suggestions). Replies to existing threads use the
  `/comments/{id}/replies` endpoint.
- Reviews are submitted through the same endpoint with an `event` of
  `COMMENT`, `APPROVE` or `REQUEST_CHANGES`.
- "Changes since your last review" reads your latest review from the
  `reviews` GraphQL connection and diffs its commit against the head with the
  compare API.

## Limitations

- Very large diffs (GitHub caps the diff API at 300 files) are limited by what
  `gh pr diff` returns.
- A range selection must stay on one side of the diff (all added/context lines,
  or all removed lines); it is trimmed at the point where the side changes.

## Tests

```sh
cargo test
```

There is also an opt-in test that exercises the write path against a real pull
request. It posts a single-line comment, a reply and a multi-line suggestion,
then deletes all of them. (Review *submission* is not covered by this test
because a submitted review cannot be removed again.)

```sh
GHDIFF_TEST_REPO=owner/name GHDIFF_TEST_PR=123 \
    cargo test -- --ignored live_post_and_delete
```

There is a second, read-only live test for the "since last review" lookup:

```sh
GHDIFF_TEST_REPO=owner/name GHDIFF_TEST_PR=123 \
    cargo test -- --ignored live_since_last_review
```

And a third, reversible live test for the review payload: it creates a
*pending* review (invisible to others) carrying a comment and a multi-line
suggestion, checks both are attached, then deletes it.

```sh
GHDIFF_TEST_REPO=owner/name GHDIFF_TEST_PR=123 \
    cargo test -- --ignored live_pending_review_roundtrip
```

The live tests never pick a default target: without both `GHDIFF_TEST_REPO` and
`GHDIFF_TEST_PR` they skip, so `cargo test -- --ignored` cannot touch a real
pull request by accident.
