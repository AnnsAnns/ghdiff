//! GitHub access. All network work is delegated to the `gh` CLI so that the
//! user's existing authentication and host configuration are reused.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::Value;

use crate::model::{
    Comment, FileDiff, FileStatus, Hunk, Loaded, PrMeta, PrSummary, ReviewThread, Side,
};

const THREAD_QUERY: &str = r#"
query($owner: String!, $name: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100, after: $cursor) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id
          isResolved
          isOutdated
          path
          line
          startLine
          diffSide
          comments(first: 100) {
            nodes {
              id
              databaseId
              body
              createdAt
              path
              line
              originalLine
              diffHunk
              author { login }
            }
          }
        }
      }
    }
  }
}
"#;

fn run_gh(args: &[&str]) -> Result<String> {
    let output = Command::new("gh")
        .args(args)
        .output()
        .context("failed to run `gh`; is the GitHub CLI installed and on PATH?")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("gh {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run `gh` with a JSON body on stdin (used for nested payloads).
fn run_gh_input(args: &[&str], input: &str) -> Result<String> {
    use std::io::Write;

    let mut child = Command::new("gh")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to run `gh`; is the GitHub CLI installed and on PATH?")?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(input.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("gh {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Work out which repository we are operating on: either the explicit
/// `--repo owner/name` value or the repository of the current directory.
pub fn resolve_repo(explicit: Option<&str>) -> Result<String> {
    if let Some(repo) = explicit {
        if !repo.contains('/') {
            bail!("--repo expects `owner/name`, got `{repo}`");
        }
        return Ok(repo.to_string());
    }
    let out = run_gh(&["repo", "view", "--json", "nameWithOwner"])
        .context("could not detect the repository; pass --repo owner/name")?;
    let value: Value = serde_json::from_str(&out)?;
    value["nameWithOwner"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("unexpected `gh repo view` output"))
}

pub fn list_open_prs(repo: &str) -> Result<Vec<PrSummary>> {
    let out = run_gh(&[
        "pr",
        "list",
        "--repo",
        repo,
        "--limit",
        "50",
        "--state",
        "open",
        "--json",
        "number,title,author,headRefName,isDraft",
    ])?;
    let list: Vec<PrSummary> = serde_json::from_str(&out)?;
    Ok(list)
}

pub fn fetch_pr(repo: &str, number: u64) -> Result<Loaded> {
    let (owner, name) = split_repo(repo)?;

    let meta_json = run_gh(&[
        "pr",
        "view",
        &number.to_string(),
        "--repo",
        repo,
        "--json",
        "number,title,url,headRefOid,baseRefOid,headRefName,baseRefName,state,additions,deletions,changedFiles,author",
    ])?;
    let mut meta: PrMeta = serde_json::from_str(&meta_json)?;
    meta.owner = owner.to_string();
    meta.repo = name.to_string();

    let diff = run_gh(&["pr", "diff", &number.to_string(), "--repo", repo])?;
    let files = crate::model::parse_unified_diff(&diff);

    let threads = fetch_threads(owner, name, number)?;

    Ok(Loaded {
        meta,
        files,
        threads,
    })
}

fn split_repo(repo: &str) -> Result<(&str, &str)> {
    repo.split_once('/')
        .ok_or_else(|| anyhow!("invalid repo `{repo}`, expected owner/name"))
}

fn fetch_threads(owner: &str, name: &str, number: u64) -> Result<Vec<ReviewThread>> {
    let mut threads = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let number_s = number.to_string();
        let mut args: Vec<String> = vec![
            "api".into(),
            "graphql".into(),
            "-F".into(),
            format!("owner={owner}"),
            "-F".into(),
            format!("name={name}"),
            "-F".into(),
            format!("number={number_s}"),
            "-f".into(),
            format!("query={THREAD_QUERY}"),
        ];
        if let Some(c) = &cursor {
            args.push("-F".into());
            args.push(format!("cursor={c}"));
        }
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = run_gh(&arg_refs)?;
        let resp: GqlResponse = serde_json::from_str(&out)?;

        let pr = resp
            .data
            .repository
            .pull_request
            .ok_or_else(|| anyhow!("pull request #{number} not found"))?;

        for node in pr.review_threads.nodes {
            threads.push(node.into_thread());
        }

        if pr.review_threads.page_info.has_next_page {
            cursor = pr.review_threads.page_info.end_cursor;
            if cursor.is_none() {
                break;
            }
        } else {
            break;
        }
    }

    Ok(threads)
}

// ---------------------------------------------------------------------------
// Posting comments
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RestComment {
    id: u64,
    body: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    original_line: Option<u32>,
    #[serde(default)]
    diff_hunk: Option<String>,
    #[serde(default)]
    user: Option<RestUser>,
}

#[derive(Debug, Deserialize)]
struct RestUser {
    login: String,
}

impl RestComment {
    fn into_comment(self) -> Comment {
        Comment {
            id: self.id,
            author: self.user.map(|u| u.login).unwrap_or_default(),
            body: self.body,
            created_at: self.created_at,
            original_line: self.original_line,
            diff_hunk: self.diff_hunk,
            pending: false,
        }
    }
}

pub struct NewComment<'a> {
    pub path: &'a str,
    pub line: u32,
    /// First line of a multi-line range (must be `< line`).
    pub start_line: Option<u32>,
    pub side: Side,
    pub commit_id: &'a str,
    pub body: &'a str,
}

/// Post a single review comment immediately. The app now batches comments into
/// a review, but this is kept for the opt-in live test and scripted use.
#[allow(dead_code)]
pub fn post_review_comment(repo: &str, number: u64, new: &NewComment) -> Result<Comment> {
    let endpoint = format!("repos/{repo}/pulls/{number}/comments");
    let line = new.line.to_string();
    let mut args: Vec<String> = vec![
        "api".into(),
        "--method".into(),
        "POST".into(),
        endpoint,
        "-f".into(),
        format!("body={}", new.body),
        "-f".into(),
        format!("commit_id={}", new.commit_id),
        "-f".into(),
        format!("path={}", new.path),
        "-F".into(),
        format!("line={line}"),
        "-f".into(),
        format!("side={}", new.side.as_api_str()),
    ];
    if let Some(start) = new.start_line {
        args.push("-F".into());
        args.push(format!("start_line={start}"));
        args.push("-f".into());
        args.push(format!("start_side={}", new.side.as_api_str()));
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = run_gh(&arg_refs).context("failed to post review comment")?;
    let comment: RestComment = serde_json::from_str(&out)?;
    Ok(comment.into_comment())
}

pub fn reply_to_comment(repo: &str, number: u64, comment_id: u64, body: &str) -> Result<Comment> {
    let endpoint = format!("repos/{repo}/pulls/{number}/comments/{comment_id}/replies");
    let out = run_gh(&[
        "api",
        "--method",
        "POST",
        &endpoint,
        "-f",
        &format!("body={body}"),
    ])
    .context("failed to post reply")?;
    let comment: RestComment = serde_json::from_str(&out)?;
    Ok(comment.into_comment())
}

/// Delete a review comment (only the author may do so).
#[allow(dead_code)] // exercised by the opt-in live test
pub fn delete_review_comment(repo: &str, comment_id: u64) -> Result<()> {
    let endpoint = format!("repos/{repo}/pulls/comments/{comment_id}");
    run_gh(&["api", "--method", "DELETE", &endpoint]).context("failed to delete comment")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Submitting a review
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewVerdict {
    Comment,
    Approve,
    RequestChanges,
}

impl ReviewVerdict {
    pub fn label(self) -> &'static str {
        match self {
            ReviewVerdict::Comment => "Comment",
            ReviewVerdict::Approve => "Approve",
            ReviewVerdict::RequestChanges => "Request changes",
        }
    }

    pub fn next(self) -> Self {
        match self {
            ReviewVerdict::Comment => ReviewVerdict::Approve,
            ReviewVerdict::Approve => ReviewVerdict::RequestChanges,
            ReviewVerdict::RequestChanges => ReviewVerdict::Comment,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            ReviewVerdict::Comment => ReviewVerdict::RequestChanges,
            ReviewVerdict::Approve => ReviewVerdict::Comment,
            ReviewVerdict::RequestChanges => ReviewVerdict::Approve,
        }
    }

    fn event(self) -> &'static str {
        match self {
            ReviewVerdict::Comment => "COMMENT",
            ReviewVerdict::Approve => "APPROVE",
            ReviewVerdict::RequestChanges => "REQUEST_CHANGES",
        }
    }
}

/// A comment (or suggestion) waiting to be published as part of a review.
#[derive(Debug, Clone)]
pub struct PendingComment {
    pub path: String,
    pub line: u32,
    pub start_line: Option<u32>,
    pub side: Side,
    pub body: String,
}

/// Submit a review, publishing any pending comments in the same call. When
/// `comments` is empty this is just a summary plus verdict.
pub fn submit_review(
    repo: &str,
    number: u64,
    commit_id: &str,
    verdict: ReviewVerdict,
    body: &str,
    comments: &[PendingComment],
) -> Result<()> {
    create_review(repo, number, commit_id, Some(verdict), body, comments)?;
    Ok(())
}

/// Create a review. With `verdict = None` the review is left **pending** (only
/// visible to its author), which is what the tests use so they can clean up.
pub fn create_review(
    repo: &str,
    number: u64,
    commit_id: &str,
    verdict: Option<ReviewVerdict>,
    body: &str,
    comments: &[PendingComment],
) -> Result<u64> {
    let endpoint = format!("repos/{repo}/pulls/{number}/reviews");
    let mut payload = serde_json::Map::new();
    payload.insert("commit_id".into(), serde_json::json!(commit_id));
    if let Some(verdict) = verdict {
        payload.insert("event".into(), serde_json::json!(verdict.event()));
    }
    if !body.trim().is_empty() {
        payload.insert("body".into(), serde_json::json!(body));
    }
    if !comments.is_empty() {
        let items: Vec<serde_json::Value> = comments
            .iter()
            .map(|c| {
                let mut m = serde_json::Map::new();
                m.insert("path".into(), serde_json::json!(c.path));
                m.insert("line".into(), serde_json::json!(c.line));
                m.insert("side".into(), serde_json::json!(c.side.as_api_str()));
                m.insert("body".into(), serde_json::json!(c.body));
                if let Some(start) = c.start_line {
                    m.insert("start_line".into(), serde_json::json!(start));
                    m.insert("start_side".into(), serde_json::json!(c.side.as_api_str()));
                }
                serde_json::Value::Object(m)
            })
            .collect();
        payload.insert("comments".into(), serde_json::Value::Array(items));
    }
    let json = serde_json::Value::Object(payload).to_string();
    let out = run_gh_input(
        &["api", "--method", "POST", &endpoint, "--input", "-"],
        &json,
    )
    .context("failed to create review")?;
    let value: serde_json::Value = serde_json::from_str(&out)?;
    Ok(value["id"].as_u64().unwrap_or(0))
}

/// Delete a pending review (only pending reviews can be deleted).
#[allow(dead_code)] // used by the opt-in live test
pub fn delete_pending_review(repo: &str, number: u64, review_id: u64) -> Result<()> {
    let endpoint = format!("repos/{repo}/pulls/{number}/reviews/{review_id}");
    run_gh(&["api", "--method", "DELETE", &endpoint]).context("failed to delete review")?;
    Ok(())
}

/// Count the comments attached to a review.
#[allow(dead_code)] // used by the opt-in live test
pub fn review_comment_count(repo: &str, number: u64, review_id: u64) -> Result<usize> {
    let endpoint = format!("repos/{repo}/pulls/{number}/reviews/{review_id}/comments");
    let out = run_gh(&["api", &endpoint])?;
    let value: serde_json::Value = serde_json::from_str(&out)?;
    Ok(value.as_array().map(Vec::len).unwrap_or(0))
}

// ---------------------------------------------------------------------------
// "Changes since your last review"
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LastReview {
    pub sha: String,
    pub submitted_at: String,
    pub state: String,
}

const REVIEWS_QUERY: &str = r#"
query($owner: String!, $name: String!, $number: Int!) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviews(last: 100) {
        nodes {
          state
          submittedAt
          commit { oid }
          author { login }
        }
      }
    }
  }
}
"#;

/// Find the viewer's most recent submitted review and compute the diff between
/// its commit and the current head. Returns `Ok(None)` when the viewer has not
/// reviewed the pull request.
pub fn diff_since_last_review(
    repo: &str,
    number: u64,
    head_sha: &str,
) -> Result<Option<(LastReview, Vec<FileDiff>)>> {
    let (owner, name) = split_repo(repo)?;
    let out = run_gh(&[
        "api",
        "graphql",
        "-F",
        &format!("owner={owner}"),
        "-F",
        &format!("name={name}"),
        "-F",
        &format!("number={number}"),
        "-f",
        &format!("query={REVIEWS_QUERY}"),
    ])?;
    let resp: ReviewsResponse = serde_json::from_str(&out)?;
    let viewer = resp.data.viewer.login;
    let pr = resp
        .data
        .repository
        .pull_request
        .ok_or_else(|| anyhow!("pull request #{number} not found"))?;

    let last = pr
        .reviews
        .nodes
        .into_iter()
        .filter(|r| r.state != "PENDING")
        .filter(|r| {
            r.author
                .as_ref()
                .map(|a| a.login == viewer)
                .unwrap_or(false)
        })
        .filter_map(|r| Some((r.commit?.oid, r.submitted_at.unwrap_or_default(), r.state)))
        .max_by(|a, b| a.1.cmp(&b.1));

    let Some((sha, submitted_at, state)) = last else {
        return Ok(None);
    };

    let review = LastReview {
        sha: sha.clone(),
        submitted_at,
        state,
    };
    if sha == head_sha {
        // The head is still the commit that was reviewed.
        return Ok(Some((review, Vec::new())));
    }
    let files = compare_files(repo, &sha, head_sha)?;
    Ok(Some((review, files)))
}

fn compare_files(repo: &str, base: &str, head: &str) -> Result<Vec<FileDiff>> {
    let out = run_gh(&["api", &format!("repos/{repo}/compare/{base}...{head}")])?;
    let compare: CompareResponse = serde_json::from_str(&out)?;
    Ok(compare
        .files
        .into_iter()
        .map(|f| {
            build_file_diff(
                &f.filename,
                &f.status,
                f.previous_filename.as_deref(),
                f.patch.as_deref(),
                f.additions,
                f.deletions,
            )
        })
        .collect())
}

/// Build a [`FileDiff`] from a GitHub compare-API file entry.
pub fn build_file_diff(
    filename: &str,
    status: &str,
    previous_filename: Option<&str>,
    patch: Option<&str>,
    additions: u32,
    deletions: u32,
) -> FileDiff {
    let status = match status {
        "added" => FileStatus::Added,
        "removed" => FileStatus::Deleted,
        "renamed" => FileStatus::Renamed,
        "copied" => FileStatus::Copied,
        _ => FileStatus::Modified,
    };
    let (hunks, binary) = match patch {
        Some(patch) => (parse_hunks(filename, status, patch), false),
        None => (Vec::new(), true),
    };
    let old_path = match status {
        FileStatus::Added => None,
        _ => Some(previous_filename.unwrap_or(filename).to_string()),
    };
    FileDiff {
        path: filename.to_string(),
        old_path,
        status,
        additions,
        deletions,
        hunks,
        binary,
    }
}

/// Parse the hunks of a compare-API `patch` (which lacks the `diff --git` and
/// `---`/`+++` headers) by wrapping it in a synthetic unified diff.
fn parse_hunks(path: &str, status: FileStatus, patch: &str) -> Vec<Hunk> {
    let (old, new) = match status {
        FileStatus::Added => ("/dev/null".to_string(), format!("b/{path}")),
        FileStatus::Deleted => (format!("a/{path}"), "/dev/null".to_string()),
        _ => (format!("a/{path}"), format!("b/{path}")),
    };
    let text = format!("diff --git a/{path} b/{path}\n--- {old}\n+++ {new}\n{patch}\n");
    crate::model::parse_unified_diff(&text)
        .into_iter()
        .next()
        .map(|f| f.hunks)
        .unwrap_or_default()
}

#[derive(Debug, Deserialize)]
struct CompareResponse {
    files: Vec<CompareFile>,
}

#[derive(Debug, Deserialize)]
struct CompareFile {
    filename: String,
    status: String,
    #[serde(default)]
    additions: u32,
    #[serde(default)]
    deletions: u32,
    #[serde(default)]
    patch: Option<String>,
    #[serde(default)]
    previous_filename: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReviewsResponse {
    data: ReviewsData,
}

#[derive(Debug, Deserialize)]
struct ReviewsData {
    viewer: GqlAuthor,
    repository: ReviewsRepo,
}

#[derive(Debug, Deserialize)]
struct ReviewsRepo {
    #[serde(rename = "pullRequest")]
    pull_request: Option<ReviewsPr>,
}

#[derive(Debug, Deserialize)]
struct ReviewsPr {
    reviews: ReviewNodes,
}

#[derive(Debug, Deserialize)]
struct ReviewNodes {
    nodes: Vec<ReviewNode>,
}

#[derive(Debug, Deserialize)]
struct ReviewNode {
    state: String,
    #[serde(rename = "submittedAt")]
    submitted_at: Option<String>,
    commit: Option<ReviewCommit>,
    author: Option<GqlAuthor>,
}

#[derive(Debug, Deserialize)]
struct ReviewCommit {
    oid: String,
}

// ---------------------------------------------------------------------------
// Offline fixtures (handy for development and testing)
// ---------------------------------------------------------------------------

/// Load a pull request from a directory containing `meta.json`, `diff.patch`
/// and `threads.json` (the raw output of `gh pr view`, `gh pr diff` and the
/// review-thread GraphQL query respectively).
pub fn load_offline(dir: &Path) -> Result<Loaded> {
    let meta_json = std::fs::read_to_string(dir.join("meta.json"))
        .with_context(|| format!("reading {}", dir.join("meta.json").display()))?;
    let mut meta: PrMeta = serde_json::from_str(&meta_json)?;
    let diff = std::fs::read_to_string(dir.join("diff.patch"))
        .with_context(|| format!("reading {}", dir.join("diff.patch").display()))?;
    let threads_json = std::fs::read_to_string(dir.join("threads.json"))
        .with_context(|| format!("reading {}", dir.join("threads.json").display()))?;

    if meta.owner.is_empty() {
        // Recover owner/repo from the PR URL if possible.
        if let Some(rest) = meta.url.split("github.com/").nth(1) {
            let mut it = rest.split('/');
            meta.owner = it.next().unwrap_or("owner").to_string();
            meta.repo = it.next().unwrap_or("repo").to_string();
        }
    }

    let files = crate::model::parse_unified_diff(&diff);

    let resp: GqlResponse = serde_json::from_str(&threads_json)?;
    let mut threads = Vec::new();
    if let Some(pr) = resp.data.repository.pull_request {
        for node in pr.review_threads.nodes {
            threads.push(node.into_thread());
        }
    }

    Ok(Loaded {
        meta,
        files,
        threads,
    })
}

// ---------------------------------------------------------------------------
// GraphQL response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GqlResponse {
    data: GqlData,
}

#[derive(Debug, Deserialize)]
struct GqlData {
    repository: GqlRepo,
}

#[derive(Debug, Deserialize)]
struct GqlRepo {
    #[serde(rename = "pullRequest")]
    pull_request: Option<GqlPr>,
}

#[derive(Debug, Deserialize)]
struct GqlPr {
    #[serde(rename = "reviewThreads")]
    review_threads: GqlThreads,
}

#[derive(Debug, Deserialize)]
struct GqlThreads {
    #[serde(rename = "pageInfo")]
    page_info: GqlPageInfo,
    nodes: Vec<GqlThread>,
}

#[derive(Debug, Deserialize)]
struct GqlPageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GqlThread {
    id: String,
    #[serde(rename = "isResolved")]
    is_resolved: bool,
    #[serde(rename = "isOutdated")]
    is_outdated: bool,
    path: String,
    line: Option<u32>,
    #[serde(rename = "startLine")]
    start_line: Option<u32>,
    #[serde(rename = "diffSide")]
    diff_side: Option<String>,
    comments: GqlComments,
}

#[derive(Debug, Deserialize)]
struct GqlComments {
    nodes: Vec<GqlComment>,
}

#[derive(Debug, Deserialize)]
struct GqlComment {
    #[serde(rename = "databaseId")]
    database_id: Option<u64>,
    body: String,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "originalLine")]
    original_line: Option<u32>,
    #[serde(rename = "diffHunk")]
    diff_hunk: Option<String>,
    author: Option<GqlAuthor>,
}

#[derive(Debug, Deserialize)]
struct GqlAuthor {
    login: String,
}

impl GqlThread {
    fn into_thread(self) -> ReviewThread {
        let side = match self.diff_side.as_deref() {
            Some("LEFT") => Side::Left,
            _ => Side::Right,
        };
        let comments = self
            .comments
            .nodes
            .into_iter()
            .map(|c| Comment {
                id: c.database_id.unwrap_or(0),
                author: c.author.map(|a| a.login).unwrap_or_default(),
                body: c.body,
                created_at: c.created_at,
                original_line: c.original_line,
                diff_hunk: c.diff_hunk,
                pending: false,
            })
            .collect();
        ReviewThread {
            id: self.id,
            path: self.path,
            line: self.line,
            start_line: self.start_line,
            side,
            resolved: self.is_resolved,
            outdated: self.is_outdated,
            comments,
            anchor: None,
        }
    }
}
