//! Launching external programs: the user's editor and a terminal emulator,
//! rooted at the checkout the pull request is being reviewed from.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};

/// A prepared external launch: the program, its arguments and the directory it
/// should be started in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    pub dir: PathBuf,
    /// Short program name, used in status messages.
    pub name: String,
}

impl Launch {
    fn new(program: &str, args: Vec<String>, dir: &Path) -> Self {
        Self {
            program: program.to_string(),
            args,
            dir: dir.to_path_buf(),
            name: base_name(program),
        }
    }
}

/// The root of the checkout ghdiff was started from. Reviews are viewed in the
/// context of that working copy, so this is where the editor and terminal open.
pub fn project_dir() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    })
    .clone()
}

// ---------------------------------------------------------------------------
// Editors
// ---------------------------------------------------------------------------

/// Editors tried, in order, when `GHDIFF_EDITOR` is not set.
const EDITORS: &[&str] = &["zeditor", "zed", "code", "codium"];

/// Prepare an editor launch. `file` is the checked-out file currently under
/// review (with the line to place the cursor on), when it exists locally.
pub fn editor_launch(dir: &Path, file: Option<(&Path, u32)>) -> Result<Launch> {
    let editor = env_value("GHDIFF_EDITOR")
        .or_else(|| {
            EDITORS
                .iter()
                .copied()
                .find(|e| on_path(e))
                .map(str::to_string)
        })
        .ok_or_else(|| {
            anyhow!("no supported editor found — install zeditor or VS Code, or set GHDIFF_EDITOR")
        })?;
    Ok(editor_launch_with(&editor, dir, file))
}

/// Build the command for a known editor. Kept separate from detection so the
/// mapping can be tested without any editor being installed.
pub fn editor_launch_with(editor: &str, dir: &Path, file: Option<(&Path, u32)>) -> Launch {
    let name = base_name(editor);
    let dir_s = dir.display().to_string();
    let target = file.map(|(path, line)| format!("{}:{}", path.display(), line.max(1)));

    let args = match name.as_str() {
        // Zed accepts `path:line:column` and treats the directory as the
        // workspace to open.
        "zeditor" | "zed" => match &target {
            Some(t) => vec!["-n".to_string(), dir_s, t.clone()],
            None => vec![dir_s],
        },
        // VS Code: open the folder, then jump to the file/line.
        "code" | "codium" | "code-insiders" => match &target {
            Some(t) => vec![dir_s, "--goto".to_string(), t.clone()],
            None => vec![dir_s],
        },
        // Unknown editor (from GHDIFF_EDITOR): plain paths, no line syntax.
        _ => {
            let mut args = vec![dir_s];
            if let Some((path, _)) = file {
                args.push(path.display().to_string());
            }
            args
        }
    };
    Launch::new(editor, args, dir)
}

// ---------------------------------------------------------------------------
// Terminals
// ---------------------------------------------------------------------------

/// Terminal emulators tried, in order, when neither `GHDIFF_TERMINAL` nor
/// `TERMINAL` is set.
const TERMINALS: &[&str] = &[
    "alacritty",
    "kitty",
    "wezterm",
    "foot",
    "ghostty",
    "konsole",
    "gnome-terminal",
    "xfce4-terminal",
    "x-terminal-emulator",
    "xterm",
];

/// Prepare a terminal launch rooted at `dir`.
pub fn terminal_launch(dir: &Path) -> Result<Launch> {
    let terminal = env_value("GHDIFF_TERMINAL")
        .or_else(|| env_value("TERMINAL"))
        .or_else(|| {
            TERMINALS
                .iter()
                .copied()
                .find(|t| on_path(t))
                .map(str::to_string)
        })
        .ok_or_else(|| {
            anyhow!("no terminal emulator found — install one or set GHDIFF_TERMINAL")
        })?;
    Ok(terminal_launch_with(&terminal, dir))
}

/// Build the command for a terminal emulator. The working directory is always
/// set on the process as well, so even an unknown terminal starts in `dir`.
pub fn terminal_launch_with(terminal: &str, dir: &Path) -> Launch {
    let name = base_name(terminal);
    let dir_s = dir.display().to_string();
    let args: Vec<String> = match name.as_str() {
        "alacritty" => vec!["--working-directory".into(), dir_s],
        "kitty" => vec!["--directory".into(), dir_s],
        "wezterm" => vec!["start".into(), "--cwd".into(), dir_s],
        "foot" => vec!["--working-directory".into(), dir_s],
        "ghostty" => vec!["--working-directory".into(), dir_s],
        "konsole" => vec!["--workdir".into(), dir_s],
        "gnome-terminal" | "kgx" => vec![format!("--working-directory={dir_s}")],
        "xfce4-terminal" | "tilix" => vec![format!("--working-directory={dir_s}")],
        _ => Vec::new(),
    };
    Launch::new(terminal, args, dir)
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

/// Start the program without blocking ghdiff on it. Stdio is detached so a
/// GUI editor or terminal never scribbles over the viewer.
pub fn run(launch: &Launch) -> Result<()> {
    let mut command = Command::new(&launch.program);
    command
        .args(&launch.args)
        .current_dir(&launch.dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to launch `{}`", launch.program))?;
    // Reap the child later so it does not linger as a zombie if it exits.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn base_name(program: &str) -> String {
    Path::new(program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.to_string())
}

/// Is `program` an executable file on `PATH` (or an explicit path)?
fn on_path(program: &str) -> bool {
    if program.contains('/') {
        return is_executable(Path::new(program));
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| is_executable(&dir.join(program)))
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = "/tmp/ghdiff-checkout";

    #[test]
    fn zed_gets_the_directory_and_file_line() {
        let file = Path::new("/tmp/ghdiff-checkout/src/main.rs");
        let launch = editor_launch_with("zeditor", Path::new(DIR), Some((file, 42)));
        assert_eq!(launch.program, "zeditor");
        assert_eq!(launch.name, "zeditor");
        assert_eq!(launch.dir, Path::new(DIR));
        assert_eq!(
            launch.args,
            vec!["-n", DIR, "/tmp/ghdiff-checkout/src/main.rs:42"]
        );
    }

    #[test]
    fn zed_without_a_local_file_opens_the_directory() {
        let launch = editor_launch_with("zed", Path::new(DIR), None);
        assert_eq!(launch.args, vec![DIR]);
    }

    #[test]
    fn vs_code_jumps_to_the_line() {
        let file = Path::new("/tmp/ghdiff-checkout/src/main.rs");
        let launch = editor_launch_with("code", Path::new(DIR), Some((file, 7)));
        assert_eq!(
            launch.args,
            vec![DIR, "--goto", "/tmp/ghdiff-checkout/src/main.rs:7"]
        );
    }

    #[test]
    fn line_zero_is_clamped() {
        let file = Path::new("/tmp/ghdiff-checkout/a.py");
        let launch = editor_launch_with("code", Path::new(DIR), Some((file, 0)));
        assert!(launch.args.iter().any(|a| a.ends_with("a.py:1")));
    }

    #[test]
    fn unknown_editors_get_plain_paths() {
        let file = Path::new("/tmp/ghdiff-checkout/a.py");
        let launch = editor_launch_with("my-editor", Path::new(DIR), Some((file, 9)));
        assert_eq!(
            launch.args,
            vec![DIR, "/tmp/ghdiff-checkout/a.py"],
            "unexpected line syntax for an unknown editor"
        );
    }

    #[test]
    fn terminals_start_in_the_checkout() {
        let kitty = terminal_launch_with("kitty", Path::new(DIR));
        assert_eq!(kitty.args, vec!["--directory", DIR]);
        assert_eq!(kitty.dir, Path::new(DIR));

        let wezterm = terminal_launch_with("/usr/bin/wezterm", Path::new(DIR));
        assert_eq!(wezterm.args, vec!["start", "--cwd", DIR]);
        assert_eq!(wezterm.name, "wezterm");

        let gnome = terminal_launch_with("gnome-terminal", Path::new(DIR));
        assert_eq!(gnome.args, vec!["--working-directory=/tmp/ghdiff-checkout"]);
    }

    #[test]
    fn unknown_terminals_only_get_the_working_directory() {
        let launch = terminal_launch_with("my-term", Path::new(DIR));
        assert!(launch.args.is_empty());
        assert_eq!(launch.dir, Path::new(DIR));
    }

    #[test]
    fn explicit_paths_are_checked_for_executability() {
        assert!(on_path("/bin/sh"));
        assert!(!on_path("/bin/definitely-not-a-real-program"));
    }
}
