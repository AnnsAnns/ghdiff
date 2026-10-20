//! Core data model: pull request metadata, unified-diff parsing and review
//! threads.

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Diff model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
}

impl FileStatus {
    #[allow(dead_code)]
    pub fn letter(self) -> char {
        match self {
            FileStatus::Added => 'A',
            FileStatus::Deleted => 'D',
            FileStatus::Modified => 'M',
            FileStatus::Renamed => 'R',
            FileStatus::Copied => 'C',
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub additions: u32,
    pub deletions: u32,
    pub hunks: Vec<Hunk>,
    pub binary: bool,
}

impl FileDiff {
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// Directory part including trailing slash (empty if file is at root).
    pub fn dir_name(&self) -> &str {
        match self.path.rfind('/') {
            Some(idx) => &self.path[..=idx],
            None => "",
        }
    }
}

// ---------------------------------------------------------------------------
// Unified diff parsing
// ---------------------------------------------------------------------------

/// Strip a leading `a/` or `b/` prefix from a diff path, and unwrap the
/// quoting that git uses for paths with special characters.
fn clean_path(raw: &str) -> Option<String> {
    let raw = raw.trim_end_matches('\r');
    let raw = raw
        .strip_prefix('"')
        .map_or(raw, |r| r.strip_suffix('"').unwrap_or(r));
    if raw == "/dev/null" {
        return None;
    }
    let stripped = raw
        .strip_prefix("a/")
        .or_else(|| raw.strip_prefix("b/"))
        .unwrap_or(raw);
    Some(stripped.to_string())
}

/// Parse the two paths out of a `diff --git <a> <b>` line.
fn parse_diff_git_paths(rest: &str) -> (Option<String>, Option<String>) {
    if rest.starts_with('"') {
        // Quoted form: "a/foo bar" "b/foo bar"
        let mut parts = Vec::new();
        let mut chars = rest.char_indices().peekable();
        while let Some(&(start, c)) = chars.peek() {
            if c == '"' {
                chars.next();
                let mut end = rest.len();
                for (i, ch) in chars.by_ref() {
                    if ch == '"' {
                        end = i;
                        break;
                    }
                }
                parts.push(clean_path(&rest[start..=end]));
            } else {
                chars.next();
            }
            if parts.len() == 2 {
                break;
            }
        }
        let a = parts.first().cloned().flatten();
        let b = parts.get(1).cloned().flatten();
        return (a, b);
    }
    if let Some(idx) = rest.rfind(" b/") {
        let a = clean_path(&rest[..idx]);
        let b = clean_path(&rest[idx + 1..]);
        (a, b)
    } else {
        (clean_path(rest), None)
    }
}

/// Parse a hunk header such as `@@ -12,10 +12,11 @@ optional context`.
fn parse_hunk_header(line: &str) -> Option<(u32, u32, String)> {
    let rest = line.strip_prefix("@@ ")?;
    let close = rest.find(" @@")?;
    let ranges = &rest[..close];
    let after = &rest[close + 3..];
    let mut parts = ranges.split(' ');
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let old_start: u32 = old.split(',').next()?.parse().ok()?;
    let new_start: u32 = new.split(',').next()?.parse().ok()?;
    Some((old_start, new_start, after.trim().to_string()))
}

/// Parse a complete unified diff (as produced by `git diff` / `gh pr diff`)
/// into a list of per-file diffs.
pub fn parse_unified_diff(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut cur: Option<FileDiff> = None;
    let mut hunk: Option<Hunk> = None;
    let mut old_no: u32 = 0;
    let mut new_no: u32 = 0;
    let mut in_hunk = false;

    macro_rules! finish_hunk {
        () => {
            if let (Some(f), Some(h)) = (cur.as_mut(), hunk.take()) {
                f.hunks.push(h);
            }
        };
    }
    macro_rules! finish_file {
        () => {
            finish_hunk!();
            if let Some(f) = cur.take() {
                files.push(f);
            }
            in_hunk = false;
        };
    }

    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);

        if let Some(rest) = line.strip_prefix("diff --git ") {
            finish_file!();
            let (a, b) = parse_diff_git_paths(rest);
            let path = b.clone().or_else(|| a.clone()).unwrap_or_default();
            cur = Some(FileDiff {
                path,
                old_path: a,
                status: FileStatus::Modified,
                additions: 0,
                deletions: 0,
                hunks: Vec::new(),
                binary: false,
            });
            continue;
        }

        if cur.is_none() {
            continue;
        }

        if !in_hunk {
            if line.starts_with("new file mode") {
                if let Some(f) = cur.as_mut() {
                    f.status = FileStatus::Added;
                    f.old_path = None;
                }
                continue;
            }
            if line.starts_with("deleted file mode") {
                if let Some(f) = cur.as_mut() {
                    f.status = FileStatus::Deleted;
                }
                continue;
            }
            if let Some(p) = line.strip_prefix("rename from ") {
                if let Some(f) = cur.as_mut() {
                    f.status = FileStatus::Renamed;
                    f.old_path = Some(p.to_string());
                }
                continue;
            }
            if let Some(p) = line.strip_prefix("rename to ") {
                if let Some(f) = cur.as_mut() {
                    f.status = FileStatus::Renamed;
                    f.path = p.to_string();
                }
                continue;
            }
            if let Some(p) = line.strip_prefix("copy from ") {
                if let Some(f) = cur.as_mut() {
                    f.status = FileStatus::Copied;
                    f.old_path = Some(p.to_string());
                }
                continue;
            }
            if let Some(p) = line.strip_prefix("copy to ") {
                if let Some(f) = cur.as_mut() {
                    f.status = FileStatus::Copied;
                    f.path = p.to_string();
                }
                continue;
            }
            if line.starts_with("Binary files") || line.starts_with("GIT binary patch") {
                if let Some(f) = cur.as_mut() {
                    f.binary = true;
                }
                continue;
            }
            if let Some(p) = line.strip_prefix("--- ") {
                if let Some(f) = cur.as_mut() {
                    f.old_path = clean_path(p);
                }
                continue;
            }
            if let Some(p) = line.strip_prefix("+++ ") {
                if let Some(f) = cur.as_mut()
                    && let Some(path) = clean_path(p)
                {
                    f.path = path;
                }
                continue;
            }
            if line.starts_with("index ")
                || line.starts_with("similarity index")
                || line.starts_with("dissimilarity index")
                || line.starts_with("old mode")
                || line.starts_with("new mode")
            {
                continue;
            }
        }

        if let Some((old_start, new_start, _ctx)) = parse_hunk_header(line) {
            finish_hunk!();
            old_no = old_start;
            new_no = new_start;
            in_hunk = true;
            hunk = Some(Hunk {
                header: line.to_string(),
                lines: Vec::new(),
            });
            continue;
        }

        if !in_hunk {
            continue;
        }

        let (kind, content) = match line.as_bytes().first() {
            Some(b'+') => (LineKind::Added, &line[1..]),
            Some(b'-') => (LineKind::Removed, &line[1..]),
            Some(b' ') => (LineKind::Context, &line[1..]),
            Some(b'\\') => continue, // "\ No newline at end of file"
            Some(_) => (LineKind::Context, line),
            None => continue,
        };

        let (old_lineno, new_lineno) = match kind {
            LineKind::Context => {
                let o = old_no;
                let n = new_no;
                old_no += 1;
                new_no += 1;
                (Some(o), Some(n))
            }
            LineKind::Added => {
                let n = new_no;
                new_no += 1;
                (None, Some(n))
            }
            LineKind::Removed => {
                let o = old_no;
                old_no += 1;
                (Some(o), None)
            }
        };

        if let (Some(f), Some(h)) = (cur.as_mut(), hunk.as_mut()) {
            match kind {
                LineKind::Added => f.additions += 1,
                LineKind::Removed => f.deletions += 1,
                LineKind::Context => {}
            }
            h.lines.push(DiffLine {
                kind,
                old_lineno,
                new_lineno,
                content: content.to_string(),
            });
        }
    }

    finish_file!();
    let _ = in_hunk;
    files
}

// ---------------------------------------------------------------------------
// Review threads
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn as_api_str(self) -> &'static str {
        match self {
            Side::Left => "LEFT",
            Side::Right => "RIGHT",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Comment {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub created_at: String,
    pub original_line: Option<u32>,
    pub diff_hunk: Option<String>,
    /// Locally created, not yet confirmed by the server.
    pub pending: bool,
}

#[derive(Debug, Clone)]
pub struct ReviewThread {
    /// Server node id, or `draft-<n>` for a locally created, unpublished thread.
    pub id: String,
    pub path: String,
    pub line: Option<u32>,
    /// First line of a multi-line comment range, if any.
    pub start_line: Option<u32>,
    pub side: Side,
    pub resolved: bool,
    pub outdated: bool,
    pub comments: Vec<Comment>,
    /// Where this thread should be displayed in the current diff.
    pub anchor: Option<Anchor>,
}

/// A position inside a parsed file diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub hunk: usize,
    pub line: usize,
}

fn find_line(file: &FileDiff, lineno: u32, side: Side) -> Option<Anchor> {
    for (hi, hunk) in file.hunks.iter().enumerate() {
        for (li, line) in hunk.lines.iter().enumerate() {
            let matches = match side {
                Side::Right => line.new_lineno == Some(lineno),
                Side::Left => line.old_lineno == Some(lineno),
            };
            if matches {
                return Some(Anchor { hunk: hi, line: li });
            }
        }
    }
    None
}

/// The content of the line a comment was originally anchored to, derived from
/// the `diff_hunk` blob (the last line of the hunk is the commented line).
fn hunk_last_content(diff_hunk: &str) -> Option<String> {
    diff_hunk
        .lines()
        .rfind(|l| !l.starts_with("@@") && !l.is_empty())
        .map(|l| l[1.min(l.len())..].to_string())
}

/// Locate a thread inside a parsed file, falling back to content matching for
/// outdated threads whose line number no longer exists.
pub fn anchor_thread(file: &FileDiff, thread: &ReviewThread) -> Option<Anchor> {
    if let Some(line) = thread.line
        && let Some(a) = find_line(file, line, thread.side)
    {
        return Some(a);
    }
    // Fallback 1: the original line number of the first comment.
    if let Some(orig) = thread.comments.first().and_then(|c| c.original_line) {
        if let Some(a) = find_line(file, orig, thread.side) {
            return Some(a);
        }
        if let Some(a) = find_line(file, orig, other_side(thread.side)) {
            return Some(a);
        }
    }
    // Fallback 2: match the commented content inside the file diff.
    if let Some(needle) = thread
        .comments
        .first()
        .and_then(|c| c.diff_hunk.as_deref())
        .and_then(hunk_last_content)
    {
        let needle = needle.trim();
        if !needle.is_empty() {
            for (hi, hunk) in file.hunks.iter().enumerate() {
                for (li, line) in hunk.lines.iter().enumerate() {
                    if line.content.trim() == needle {
                        return Some(Anchor { hunk: hi, line: li });
                    }
                }
            }
        }
    }
    None
}

fn other_side(side: Side) -> Side {
    match side {
        Side::Left => Side::Right,
        Side::Right => Side::Left,
    }
}

pub fn reanchor_all(files: &[FileDiff], threads: &mut [ReviewThread]) {
    for thread in threads.iter_mut() {
        thread.anchor = files
            .iter()
            .find(|f| f.path == thread.path)
            .and_then(|f| anchor_thread(f, thread));
    }
}

// ---------------------------------------------------------------------------
// Pull request metadata
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PrMeta {
    pub number: u64,
    pub title: String,
    pub url: String,
    #[serde(rename = "headRefOid")]
    pub head_sha: String,
    #[serde(rename = "baseRefOid")]
    pub base_sha: String,
    #[serde(rename = "headRefName")]
    pub head_ref: String,
    #[serde(rename = "baseRefName")]
    pub base_ref: String,
    pub state: String,
    pub additions: u64,
    pub deletions: u64,
    #[serde(rename = "changedFiles")]
    pub changed_files: u64,
    pub author: Option<AuthorRef>,
    /// Filled in by the caller (not part of `gh pr view` output).
    #[serde(skip)]
    pub owner: String,
    #[serde(skip)]
    pub repo: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthorRef {
    pub login: String,
}

impl PrMeta {
    pub fn author_login(&self) -> &str {
        self.author.as_ref().map(|a| a.login.as_str()).unwrap_or("")
    }

    pub fn full_repo(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrSummary {
    pub number: u64,
    pub title: String,
    pub author: Option<AuthorRef>,
    #[serde(rename = "headRefName")]
    pub head_ref: String,
    #[serde(rename = "isDraft", default)]
    pub is_draft: bool,
}

impl PrSummary {
    pub fn author_login(&self) -> &str {
        self.author.as_ref().map(|a| a.login.as_str()).unwrap_or("")
    }
}

/// Everything needed to render a pull request.
#[derive(Debug)]
pub struct Loaded {
    pub meta: PrMeta,
    pub files: Vec<FileDiff>,
    pub threads: Vec<ReviewThread>,
}
