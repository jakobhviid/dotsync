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

/// One selectable line: a whole group (toggle all members) or a single mapping.
enum Row<'a> {
    Group(String, Vec<&'a Item>),
    Single(&'a Item),
}

/// Partition actionable items into group rows (in first-seen order) and singles.
fn rows<'a>(choices: &[&'a Item]) -> Vec<Row<'a>> {
    let mut groups: Vec<(String, Vec<&Item>)> = Vec::new();
    let mut singles: Vec<&Item> = Vec::new();
    for &it in choices {
        match &it.mapping.group {
            Some(g) => match groups.iter_mut().find(|(n, _)| n == g) {
                Some((_, members)) => members.push(it),
                None => groups.push((g.clone(), vec![it])),
            },
            None => singles.push(it),
        }
    }
    let mut out: Vec<Row> = groups
        .into_iter()
        .map(|(g, m)| Row::Group(g, m))
        .collect();
    out.extend(singles.into_iter().map(Row::Single));
    out
}

fn linked_count(members: &[&Item]) -> usize {
    members.iter().filter(|i| i.state.is_linked()).count()
}

/// Run the picker over `items`, returning the outcomes of what was applied.
/// Groups collapse to a single toggle-all row. Falls back with a clear message
/// if there is nothing to choose.
pub fn run(items: &[Item], home: &Path) -> Result<Vec<Outcome>> {
    let choices = actionable(items);
    if choices.is_empty() {
        println!("Nothing to pick — no mappings apply on this machine yet.");
        println!("Add one with `dotsync adopt <path>`.");
        return Ok(Vec::new());
    }

    let rows = rows(&choices);
    let labels: Vec<String> = rows
        .iter()
        .map(|r| match r {
            Row::Group(name, members) => {
                let n = linked_count(members);
                format!("{:<26}  group · {}/{} linked", name, n, members.len())
            }
            Row::Single(item) => row_label(item, home),
        })
        .collect();
    let defaults: Vec<bool> = rows
        .iter()
        .map(|r| match r {
            Row::Group(_, members) => linked_count(members) == members.len(),
            Row::Single(item) => item.state == State::Linked,
        })
        .collect();

    let selection = MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Select what to sync onto this machine (space toggles, enter applies)")
        .items(&labels)
        .defaults(&defaults)
        .interact()?;

    let mut outcomes = Vec::new();
    let mut apply_to = |item: &Item, selected: bool| {
        if selected {
            let out = apply::link_item(item, false);
            if out.action != "already-linked" {
                outcomes.push(out);
            }
        } else if item.state == State::Linked || item.state == State::DanglingSelf {
            outcomes.push(apply::unlink_item(item, false));
        }
    };

    for (idx, row) in rows.iter().enumerate() {
        let selected = selection.contains(&idx);
        match row {
            Row::Group(_, members) => {
                for m in members {
                    apply_to(m, selected);
                }
            }
            Row::Single(item) => apply_to(item, selected),
        }
    }
    Ok(outcomes)
}
