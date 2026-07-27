//! The interactive terminal picker: a multiselect checklist over the cloud
//! mappings, pre-checked to match what's currently linked on this machine. You
//! toggle items on/off and apply — checking an item links it here, unchecking a
//! linked item removes it. The filesystem (the symlinks) is the source of truth,
//! so the picker simply reconciles to your selection.

use std::path::Path;

use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, MultiSelect};

use dotsync_core::apply::{self, Outcome};
use dotsync_core::overview;
use dotsync_core::plan::{Item, State};

/// Items the user can act on (everything except OS-skipped / entirely-absent).
fn actionable(items: &[Item]) -> Vec<&Item> {
    items
        .iter()
        .filter(|i| !matches!(i.state, State::Skipped | State::Missing | State::LocalOnly))
        .collect()
}

fn row_label(item: &Item, home: &Path) -> String {
    let (lab, note) = overview::label(item, home);
    let secret = if item.is_secret() { "  secret" } else { "" };
    if note.is_empty() {
        format!("{:<28}  {}{}", item.name(), lab, secret)
    } else {
        format!("{:<28}  {} — {}{}", item.name(), lab, note, secret)
    }
}

/// Run the picker over `items`, returning the outcomes of what was applied.
/// Falls back with a clear message if there is nothing to choose.
pub fn run(items: &[Item], home: &Path) -> Result<Vec<Outcome>> {
    let choices = actionable(items);
    if choices.is_empty() {
        println!("Nothing to pick — no mappings apply on this machine yet.");
        println!("Add one with `dotsync adopt <path>`.");
        return Ok(Vec::new());
    }

    let labels: Vec<String> = choices.iter().map(|i| row_label(i, home)).collect();
    let defaults: Vec<bool> = choices.iter().map(|i| i.state == State::Linked).collect();

    let selection = MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Select what to sync onto this machine (space toggles, enter applies)")
        .items(&labels)
        .defaults(&defaults)
        .interact()?;

    let mut outcomes = Vec::new();
    for (idx, item) in choices.iter().enumerate() {
        let selected = selection.contains(&idx);
        let was_linked = item.state == State::Linked;
        if selected {
            let out = apply::link_item(item, false);
            // Don't spam "already-linked" for untouched rows.
            if out.action != "already-linked" {
                outcomes.push(out);
            }
        } else if was_linked || item.state == State::DanglingSelf {
            outcomes.push(apply::unlink_item(item, false));
        }
    }
    Ok(outcomes)
}
