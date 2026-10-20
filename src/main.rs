//! ghdiff — a local terminal viewer for GitHub pull requests.

mod app;
mod editor;
mod github;
mod launch;
mod model;
mod syntax;
mod termbg;
#[cfg(test)]
mod tests;
mod theme;
mod ui;
mod util;

use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::time::Duration;

use anyhow::{Result, bail};
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
};
use ratatui::crossterm::execute;

use app::App;

struct Options {
    number: Option<u64>,
    repo: Option<String>,
    offline: Option<PathBuf>,
    dump: bool,
    file: Option<String>,
    theme: Option<String>,
}

fn parse_args() -> Result<Options> {
    let mut opts = Options {
        number: None,
        repo: None,
        offline: None,
        dump: false,
        file: None,
        theme: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("ghdiff {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--dump" => opts.dump = true,
            "-t" | "--theme" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--theme requires a value"))?;
                opts.theme = Some(value);
            }
            "-f" | "--file" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--file requires a value"))?;
                opts.file = Some(value);
            }
            "-R" | "--repo" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--repo requires a value"))?;
                opts.repo = Some(value);
            }
            "--offline" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--offline requires a directory"))?;
                opts.offline = Some(PathBuf::from(value));
            }
            other => {
                if let Some(rest) = other.strip_prefix("--repo=") {
                    opts.repo = Some(rest.to_string());
                } else if let Some(rest) = other.strip_prefix("--offline=") {
                    opts.offline = Some(PathBuf::from(rest));
                } else if let Some(rest) = other.strip_prefix("--theme=") {
                    opts.theme = Some(rest.to_string());
                } else if other.starts_with('-') {
                    bail!("unknown option `{other}` (try --help)");
                } else {
                    let number: u64 = other
                        .trim_start_matches('#')
                        .parse()
                        .map_err(|_| anyhow::anyhow!("`{other}` is not a pull request number"))?;
                    opts.number = Some(number);
                }
            }
        }
    }
    Ok(opts)
}

fn print_help() {
    println!(
        "\
ghdiff — browse a GitHub pull request in your terminal

USAGE:
    ghdiff [PR_NUMBER] [OPTIONS]

Run it from inside a git checkout to review that repository's pull requests.
With no PR number, an interactive picker of open pull requests is shown.

OPTIONS:
    -R, --repo <OWNER/NAME>   Repository to use (defaults to the current one)
        --offline <DIR>       Load from a fixture directory instead of GitHub
        --dump                Print one rendered frame and exit (no TUI)
    -t, --theme <MODE>        Colour scheme: auto (default), dark or light
    -h, --help                Print help
    -V, --version             Print version

KEYS:
    Tab            switch focus between the file list and the diff
    j / k          move down / up
    c              write a review comment on the current line
    s              suggest a code change on the line or selection
    d              discard the draft comment under the cursor
    v / V          select a range of lines, then c to comment on it
    r              reply to the comment thread under the cursor
    S              submit your review, publishing every draft
    Ctrl+S         save the comment being written (Esc cancels)
    t              toggle display of resolved threads
    [ / ]          previous / next file
    o              open the pull request in a browser
    e              open the current file in zeditor / VS Code
                   (offers `gh pr checkout` first if the PR is not checked out)
    T              open a terminal in the checkout
    L              show only changes since your last review
    R              reload
    q              quit

Comments and suggestions are kept as drafts and are only published to GitHub
when you submit your review with S.

MOUSE:
    click          move the cursor / select a file
    drag           select a range of lines to comment on
    shift+click    extend the selection from the cursor
    wheel          scroll the pane under the pointer
"
    );
}

fn main() -> Result<()> {
    let opts = parse_args()?;

    // Resolve the colour scheme before anything renders.
    let requested = opts
        .theme
        .clone()
        .or_else(|| std::env::var("GHDIFF_THEME").ok());
    let (mode, bg) = match requested.as_deref() {
        None => termbg::detect(),
        Some(value) => match theme::parse_mode(value) {
            Some(None) => termbg::detect(),
            Some(Some(mode)) => (mode, None),
            None => bail!("unknown theme `{value}` (expected auto, dark or light)"),
        },
    };
    theme::set(mode, bg);

    let repo = match &opts.offline {
        Some(_) => opts.repo.clone().unwrap_or_default(),
        None => github::resolve_repo(opts.repo.as_deref())?,
    };

    let (tx, rx) = channel();
    let mut app = App::new(repo, tx, rx);
    if opts.offline.is_some() {
        app.offline = true;
    }

    if opts.dump {
        return dump(&mut app, &opts);
    }

    if let Some(dir) = &opts.offline {
        match github::load_offline(dir) {
            Ok(loaded) => {
                if app.repo.is_empty() {
                    app.repo = loaded.meta.full_repo();
                }
                app.set_loaded(loaded);
            }
            Err(e) => {
                app.error = format!("{e:#}");
                app.screen = app::Screen::Error;
            }
        }
    } else if let Some(number) = opts.number {
        app.start_load(number);
    } else {
        app.start_pr_list();
    }

    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let result = run(&mut terminal, &mut app);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;

        if event::poll(Duration::from_millis(120))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.on_key(key),
                Event::Mouse(mouse) => app.on_mouse(mouse),
                Event::Resize(..) => app.view_dirty = true,
                _ => {}
            }
        }

        app.drain_messages();

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

/// Load the pull request synchronously and print a rendered frame. Handy for
/// scripting and for eyeballing the layout without a terminal.
fn dump(app: &mut App, opts: &Options) -> Result<()> {
    if let Some(dir) = &opts.offline {
        let loaded = github::load_offline(dir)?;
        if app.repo.is_empty() {
            app.repo = loaded.meta.full_repo();
        }
        app.set_loaded(loaded);
    } else if let Some(number) = opts.number {
        let loaded = github::fetch_pr(&app.repo, number)?;
        app.set_loaded(loaded);
    } else {
        bail!("--dump needs a PR number or --offline DIR");
    }

    if let Some(needle) = &opts.file {
        match app
            .files
            .iter()
            .position(|f| f.path.contains(needle.as_str()))
        {
            Some(idx) => app.select_file(idx),
            None => bail!("no changed file matches `{needle}`"),
        }
    }

    let width = std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse().ok())
        .unwrap_or(160);
    let height = std::env::var("LINES")
        .ok()
        .and_then(|c| c.parse().ok())
        .unwrap_or(50);
    print!("{}", ui::render_to_string(app, width, height));
    Ok(())
}
