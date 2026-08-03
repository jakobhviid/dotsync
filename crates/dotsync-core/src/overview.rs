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

    // Column width in *characters*, not bytes: the `{:<name_w$}` formatter pads by
    // char count, so measuring bytes over-pads any non-ASCII name (Danish æ/ø/å).
    let name_w = items.iter().map(|i| i.name().chars().count()).max().unwrap_or(4).max(4);

    // Group rows (in first-seen order) render under a header with members
    // indented; ungrouped rows render normally.
    let mut group_order: Vec<String> = Vec::new();
    for item in items {
        if let Some(g) = &item.mapping.group {
            if !group_order.contains(g) {
                group_order.push(g.clone());
            }
        }
    }
    for g in &group_order {
        let members: Vec<&Item> = items
            .iter()
            .filter(|i| i.mapping.group.as_deref() == Some(g.as_str()))
            .collect();
        let linked = members.iter().filter(|i| i.state.is_linked()).count();
        println!(
            "  {}  {}",
            ui::bold(g),
            ui::dim(&format!("group · {}/{} linked", linked, members.len()))
        );
        for item in members {
            print_row(item, &cfg.home, name_w, 4);
        }
    }
    for item in items.iter().filter(|i| i.mapping.group.is_none()) {
        print_row(item, &cfg.home, name_w, 0);
    }
    println!();
}

/// Print one mapping row, optionally indented (for group members).
fn print_row(item: &Item, home: &Path, name_w: usize, indent: usize) {
    let (lab, note) = label(item, home);
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
    // Link the name to its cloud copy (file://…), padded by the *visible* width so
    // the invisible OSC-8 escape can't throw off column alignment.
    let name = item.name();
    let linked = ui::hyperlink(name, &format!("file://{}", item.source.display()));
    let padding = name_w.saturating_sub(name.chars().count());
    let name_cell = format!("{linked}{blank:padding$}", blank = "");
    println!(
        "  {:indent$}{} {name_cell}  {}{}{}",
        "",
        symbol(&item.state),
        colorize(&item.state, &lab),
        secret,
        note,
        indent = indent,
    );
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
