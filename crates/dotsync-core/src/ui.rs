//! Output discipline and a tiny ANSI palette.
//!
//! The stream split is by **result vs process**, not severity. The command's
//! *result* — the status table, the `doctor` report, a `--json` document — goes
//! to **stdout**. Everything that *narrates the run* — progress, warnings,
//! errors, and the per-item outcome lines of a mutating sweep (successes
//! included) — goes to **stderr**, as one ordered stream. So `dotsync status
//! > out` captures exactly the table, `dotsync … --json | jq` stays pipe-clean,
//! and a redirected stream never captures half a log. To keep the human sweep
//! log, redirect stderr (`2> log`); stdout is reserved for the payload.
//!
//! Colour is decided **once per stream** (cached in a `OnceLock`) and only for a
//! real terminal with `NO_COLOR` unset — keyed to the stream each string is
//! actually written to, so redirecting one stream never leaks ANSI escapes into
//! the other's capture.

use std::io::IsTerminal;
use std::sync::OnceLock;

/// The stream a painted string is destined for. Colour is gated on *that*
/// stream's terminal-ness.
#[derive(Clone, Copy)]
pub enum To {
    /// stdout — the command's result (tables, reports, `--json`).
    Out,
    /// stderr — narration: progress, warnings, errors, sweep outcome lines.
    Err,
}

/// Whether the given stream is a terminal (cached once per stream).
fn stream_is_terminal(to: To) -> bool {
    match to {
        To::Out => {
            static ENABLED: OnceLock<bool> = OnceLock::new();
            *ENABLED.get_or_init(|| std::io::stdout().is_terminal())
        }
        To::Err => {
            static ENABLED: OnceLock<bool> = OnceLock::new();
            *ENABLED.get_or_init(|| std::io::stderr().is_terminal())
        }
    }
}

fn colour_for(to: To) -> bool {
    std::env::var_os("NO_COLOR").is_none() && stream_is_terminal(to)
}

/// Wrap `text` in an ANSI colour code when its destination stream is a
/// colour-capable terminal, otherwise return it unchanged.
pub fn paint(to: To, code: &str, text: &str) -> String {
    if colour_for(to) {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// Wrap `text` as an OSC-8 hyperlink to `url` when writing to a terminal, else
/// return it plain. Gated purely on stdout being a TTY (not `NO_COLOR` — a link
/// is not colour), so piped/redirected output stays plain text; terminals that
/// don't support OSC-8 consume the well-formed escape and show just the text.
pub fn hyperlink(text: &str, url: &str) -> String {
    if stream_is_terminal(To::Out) {
        format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
    } else {
        text.to_string()
    }
}

// The palette below paints for **stdout** (`To::Out`) — the common case, since
// results (tables, reports) are what carry colour. Status helpers paint for
// stderr themselves. For a colour fragment embedded in a stderr line, call
// `paint(To::Err, …)` directly.

pub fn bold(text: &str) -> String {
    paint(To::Out, "1", text)
}
pub fn dim(text: &str) -> String {
    paint(To::Out, "2", text)
}
pub fn green(text: &str) -> String {
    paint(To::Out, "32", text)
}
pub fn yellow(text: &str) -> String {
    paint(To::Out, "33", text)
}
pub fn red(text: &str) -> String {
    paint(To::Out, "31", text)
}
pub fn cyan(text: &str) -> String {
    paint(To::Out, "36", text)
}

/// `▸` informational/progress line → stderr.
pub fn info(msg: &str) {
    eprintln!("{} {msg}", paint(To::Err, "36", "▸"));
}

/// `✓` success line → stderr.
pub fn ok(msg: &str) {
    eprintln!("{} {msg}", paint(To::Err, "32", "✓"));
}

/// `⚠` warning line → stderr.
pub fn warn(msg: &str) {
    eprintln!("{} {msg}", paint(To::Err, "33", "⚠"));
}

/// `✗` error line → stderr.
pub fn err(msg: &str) {
    eprintln!("{} {msg}", paint(To::Err, "31", "✗"));
}
