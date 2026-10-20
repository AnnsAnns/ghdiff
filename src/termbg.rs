//! Terminal background detection.
//!
//! Terminals that support the OSC 11 query (alacritty, kitty, wezterm, foot,
//! iTerm2, …) will report their background colour when asked, which lets us
//! pick a matching palette instead of guessing. When that fails we fall back
//! to `COLORFGBG`, and finally to assuming a dark background.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

/// The detected background colour and the resulting light/dark classification.
pub fn detect() -> (Mode, Option<(u8, u8, u8)>) {
    if let Some(rgb) = query_osc11() {
        return (mode_for(rgb), Some(rgb));
    }
    if let Some(rgb) = from_colorfgbg() {
        return (mode_for(rgb), Some(rgb));
    }
    (Mode::Dark, None)
}

pub fn mode_for(rgb: (u8, u8, u8)) -> Mode {
    let (r, g, b) = rgb;
    let luma = 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
    if luma > 127.5 {
        Mode::Light
    } else {
        Mode::Dark
    }
}

/// `COLORFGBG` is set by some terminals as `foreground;background` using ANSI
/// colour indices.
fn from_colorfgbg() -> Option<(u8, u8, u8)> {
    let value = std::env::var("COLORFGBG").ok()?;
    let bg = value.rsplit(';').next()?.trim().parse::<u8>().ok()?;
    Some(match bg {
        7 | 9..=15 => (255, 255, 255),
        _ => (0, 0, 0),
    })
}

/// Parse an OSC 11 reply such as `ESC ] 11 ; rgb:1e1e/1e1e/1e1e ST` or
/// `ESC ] 11 ; #1e1e1e BEL`.
fn parse_osc11(bytes: &[u8]) -> Option<(u8, u8, u8)> {
    let text = String::from_utf8_lossy(bytes);
    let start = text.find("]11;")?;
    let rest = &text[start + 4..];
    let end = rest.find(['\x07', '\u{1b}']).unwrap_or(rest.len());
    let value = rest[..end].trim();

    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex_triplet(hex);
    }
    if let Some(spec) = value.strip_prefix("rgb:") {
        let parts: Vec<&str> = spec.split('/').collect();
        if parts.len() == 3 {
            return Some((
                parse_component(parts[0])?,
                parse_component(parts[1])?,
                parse_component(parts[2])?,
            ));
        }
    }
    None
}

/// `#rrggbb` or `#rgb`.
fn parse_hex_triplet(hex: &str) -> Option<(u8, u8, u8)> {
    match hex.len() {
        6 => Some((
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
        )),
        3 => {
            let d = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok().map(|v| v * 17);
            Some((d(0)?, d(1)?, d(2)?))
        }
        _ => None,
    }
}

/// A component of an `rgb:` spec, which may use 1–4 hex digits.
fn parse_component(part: &str) -> Option<u8> {
    let part = part.trim();
    if part.is_empty() || part.len() > 4 {
        return None;
    }
    let value = u32::from_str_radix(part, 16).ok()?;
    Some(match part.len() {
        1 => (value * 17) as u8,
        2 => value as u8,
        3 => (value >> 4) as u8,
        _ => (value >> 8) as u8,
    })
}

#[cfg(unix)]
fn query_osc11() -> Option<(u8, u8, u8)> {
    use std::io::{Read, Write};
    use std::os::unix::io::AsRawFd;
    use std::time::{Duration, Instant};

    let stdout = std::io::stdout();
    let stdin = std::io::stdin();
    let out_fd = stdout.as_raw_fd();
    let in_fd = stdin.as_raw_fd();
    // Only meaningful on a real terminal.
    if unsafe { libc::isatty(out_fd) } != 1 || unsafe { libc::isatty(in_fd) } != 1 {
        return None;
    }

    let mut original: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(in_fd, &mut original) } != 0 {
        return None;
    }

    // If the user has already typed something, don't steal it — skip the query.
    let mut fds = libc::pollfd {
        fd: in_fd,
        events: libc::POLLIN,
        revents: 0,
    };
    if unsafe { libc::poll(&mut fds, 1, 0) } > 0 && (fds.revents & libc::POLLIN) != 0 {
        return None;
    }

    let mut raw = original;
    raw.c_lflag &= !(libc::ICANON | libc::ECHO);
    raw.c_cc[libc::VMIN] = 0;
    raw.c_cc[libc::VTIME] = 1; // 100 ms
    if unsafe { libc::tcsetattr(in_fd, libc::TCSANOW, &raw) } != 0 {
        return None;
    }

    let result = (|| {
        {
            let mut out = stdout.lock();
            out.write_all(b"\x1b]11;?\x1b\\").ok()?;
            out.flush().ok()?;
        }
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 64];
        let mut stdin = stdin.lock();
        let mut empty_reads = 0;
        while Instant::now() < deadline {
            match stdin.read(&mut chunk) {
                Ok(0) => {
                    // VMIN=0/VTIME=1: a zero read means a 100 ms timeout.
                    empty_reads += 1;
                    if empty_reads >= 2 || !buf.is_empty() {
                        break;
                    }
                }
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.contains(&0x07) || buf.windows(2).any(|w| w == b"\x1b\\") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        parse_osc11(&buf)
    })();

    unsafe { libc::tcsetattr(in_fd, libc::TCSANOW, &original) };
    result
}

#[cfg(not(unix))]
fn query_osc11() -> Option<(u8, u8, u8)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rgb_osc_reply() {
        let reply = b"\x1b]11;rgb:1e1e/2b2b/3c3c\x1b\\";
        assert_eq!(parse_osc11(reply), Some((0x1e, 0x2b, 0x3c)));
    }

    #[test]
    fn parses_short_and_hash_replies() {
        assert_eq!(
            parse_osc11(b"\x1b]11;rgb:ff/00/80\x07"),
            Some((255, 0, 128))
        );
        assert_eq!(parse_osc11(b"\x1b]11;#ffffff\x07"), Some((255, 255, 255)));
        assert_eq!(parse_osc11(b"\x1b]11;#abc\x1b\\"), Some((0xaa, 0xbb, 0xcc)));
    }

    #[test]
    fn ignores_unrelated_output() {
        assert_eq!(parse_osc11(b"just some text"), None);
    }

    #[test]
    fn classifies_backgrounds_by_luminance() {
        assert_eq!(mode_for((255, 255, 255)), Mode::Light);
        assert_eq!(mode_for((0, 0, 0)), Mode::Dark);
        assert_eq!(mode_for((13, 17, 23)), Mode::Dark);
    }
}
