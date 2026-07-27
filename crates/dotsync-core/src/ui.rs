//! Tiny output helpers: ANSI color that respects `NO_COLOR` and non-TTY output,
//! plus status symbols. Human output goes to stdout; errors to stderr.

use std::io::{IsTerminal, Write};

/// Whether colored output should be emitted (stdout is a TTY and `NO_COLOR`
/// is unset).
pub fn color_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

fn paint(code: &str, s: &str) -> String {
    if color_enabled() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub fn bold(s: &str) -> String {
    paint("1", s)
}
pub fn dim(s: &str) -> String {
    paint("2", s)
}
pub fn green(s: &str) -> String {
    paint("32", s)
}
pub fn yellow(s: &str) -> String {
    paint("33", s)
}
pub fn red(s: &str) -> String {
    paint("31", s)
}
pub fn cyan(s: &str) -> String {
    paint("36", s)
}

/// `▸` informational line to stdout.
pub fn info(msg: &str) {
    println!("{} {}", cyan("▸"), msg);
}

/// `✓` success line to stdout.
pub fn ok(msg: &str) {
    println!("{} {}", green("✓"), msg);
}

/// `⚠` warning line to stderr.
pub fn warn(msg: &str) {
    let mut err = std::io::stderr();
    let _ = writeln!(err, "{} {}", yellow("⚠"), msg);
}

/// `✗` error line to stderr.
pub fn err(msg: &str) {
    let mut e = std::io::stderr();
    let _ = writeln!(e, "{} {}", red("✗"), msg);
}
