//! The overview: turn computed [`Item`]s into a human dashboard and a JSON
//! payload. This is the read-only face of `dotsync` (bare invocation and
//! `status`) and the data the interactive picker renders.

use std::path::Path;

use serde_json::json;

use crate::config::Config;
use crate::mapping::collapse_tilde;
use crate::plan::{Item, State};
use crate::ui;

/// A short human label + note for a state, e.g. ("linked", ""). Paths in the
/// note are collapsed against `home` for readability.
pub fn label(item: &Item, home: &Path) -> (String, String) {
    let m = &item.mapping;
    match &item.state {
        State::Skipped => ("skipped".into(), format!("{} only", m.os_display())),
        State::Linked => ("linked".into(), String::new()),
        State::Available => ("available".into(), "in cloud, not linked here".into()),
        State::LocalOnly => ("local-only".into(), "adopt to start syncing".into()),
        State::Healable => ("healable".into(), "atomic-save clobber; content matches".into()),
        State::Diverged => ("conflict".into(), "local file differs from cloud".into()),
        State::DanglingSelf => ("dangling".into(), "cloud copy missing (not downloaded?)".into()),
        State::ForeignSymlink(dest) => ("foreign".into(), format!("-> {}", collapse_tilde(dest, home))),
        State::Missing => ("missing".into(), "neither local nor cloud".into()),
    }
}

fn colorize(state: &State, s: &str) -> String {
    match state {
        State::Linked => ui::green(s),
        State::Available | State::LocalOnly => ui::cyan(s),
        State::Healable => ui::yellow(s),
        State::Diverged | State::DanglingSelf | State::ForeignSymlink(_) => ui::red(s),
        State::Skipped | State::Missing => ui::dim(s),
    }
}

/// The checkbox-style symbol shown next to a row.
fn symbol(state: &State) -> String {
    let raw = match state {
        State::Linked => "[x]",
        State::Available | State::LocalOnly => "[ ]",
        State::Healable => "[~]",
        State::Diverged | State::DanglingSelf | State::ForeignSymlink(_) => "[!]",
        State::Skipped | State::Missing => "   ",
    };
    colorize(state, raw)
}

/// (predicate on state, label, color fn) — one summary bucket.
type Bucket = (fn(&State) -> bool, &'static str, fn(&str) -> String);

/// A compact, colored count summary like "3 linked · 1 available · 1 conflict".
pub fn summarize(items: &[Item]) -> String {
    // In display order.
    let buckets: [Bucket; 7] = [
        (|s| matches!(s, State::Linked), "linked", ui::green),
        (|s| matches!(s, State::Available), "available", ui::cyan),
        (|s| matches!(s, State::Healable), "healable", ui::yellow),
        (
            |s| matches!(s, State::Diverged | State::ForeignSymlink(_)),
            "conflict",
            ui::red,
        ),
        (|s| matches!(s, State::DanglingSelf), "dangling", ui::red),
        (|s| matches!(s, State::LocalOnly), "local-only", ui::cyan),
        (|s| matches!(s, State::Skipped), "skipped", ui::dim),
    ];
    let mut parts = Vec::new();
    for (pred, label, color) in buckets {
        let n = items.iter().filter(|i| pred(&i.state)).count();
        if n > 0 {
            parts.push(color(&format!("{n} {label}")));
        }
    }
    parts.join(&ui::dim(" · "))
}

/// Print the read-only dashboard.
pub fn render(items: &[Item], cfg: &Config) {
    println!(
        "\n  {}  {}",
        ui::bold("dotsync"),
        ui::dim(&collapse_tilde(&cfg.sync_dir, &cfg.home))
    );
    if items.is_empty() {
        println!(
            "\n  {}\n",
            ui::dim("no mappings yet — `dotsync adopt <path>` to add one")
        );
        return;
    }
    println!(
        "  {}",
        ui::dim(&format!(
            "home {} · {} mapping{}",
            collapse_tilde(&cfg.home, &cfg.home),
            items.len(),
            if items.len() == 1 { "" } else { "s" }
        ))
    );
    let summary = summarize(items);
    if !summary.is_empty() {
        println!("  {summary}");
    }
    println!();

    let name_w = items.iter().map(|i| i.name().len()).max().unwrap_or(4).max(4);
    for item in items {
        let (lab, note) = label(item, &cfg.home);
        let secret = if item.is_secret() {
            ui::dim(&format!(" secret {}", item.mapping.mode.as_deref().unwrap_or("")))
        } else {
            String::new()
        };
        let note = if note.is_empty() {
            String::new()
        } else {
            format!("  {}", ui::dim(&note))
        };
        println!(
            "  {} {:<name_w$}  {}{}{}",
            symbol(&item.state),
            item.name(),
            colorize(&item.state, &lab),
            secret,
            note,
            name_w = name_w,
        );
    }
    println!();
}

/// Build the JSON payload for `--json`.
pub fn to_json(items: &[Item], cfg: &Config) -> serde_json::Value {
    let rows: Vec<_> = items
        .iter()
        .map(|item| {
            json!({
                "name": item.name(),
                "state": item.state.code(),
                "target": item.target.as_ref().map(|t| t.display().to_string()),
                "source": item.source.display().to_string(),
                "linked": item.state.is_linked(),
                "secret": item.is_secret(),
                "mode": item.mapping.mode,
            })
        })
        .collect();
    json!({
        "sync_dir": cfg.sync_dir.display().to_string(),
        "home": cfg.home.display().to_string(),
        "mappings": rows,
    })
}
