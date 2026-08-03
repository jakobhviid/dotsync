//! The undo journal: a per-machine record of each destructive run so `dotsync
//! undo` can reverse the most recent one.
//!
//! **Location is per-machine state** — not config, and not the synced folder. The
//! bytes an undo restores only ever existed on the machine that ran the op, so a
//! journal that travelled would carry entries other machines can't apply; the
//! cross-machine half of an op already rides the synced `dotsync.toml`. Resolved as
//! `$DOTSYNC_STATE_DIR` → `$XDG_STATE_HOME/dotsync` → `~/.local/state/dotsync`
//! (matching the amdl/temper siblings). The directory is passed in as a parameter,
//! never read from the environment inside these functions, so tests point it at a
//! tempdir and run in parallel without racy `set_var`.
//!
//! **No separate byte-store:** dotsync already preserves originals — the cloud copy
//! (adopt / restore) or the `<path>.bak` (`install --adopt`) — so a run manifest
//! only records each action's paths and reverses using those. Every reversal is
//! guarded structurally: it acts only if the on-disk state is still exactly what
//! dotsync left, and otherwise **skips and reports, never clobbering** something the
//! user changed since.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::apply::Outcome;
use crate::config::Config;
use crate::fsutil;
use crate::mapping::{Mapping, MappingsFile};

/// How to reverse one destructive action. Recorded on the [`Outcome`] the forward
/// op returns, then persisted in a run manifest. Each variant carries the exact
/// paths the forward op used — the `.bak` path in particular **cannot** be
/// re-derived afterwards, since [`fsutil::backup_path`] is stateful.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UndoAction {
    /// `adopt` moved a real path into the cloud and symlinked it back. Reverse:
    /// drop the symlink, move the cloud copy back to `$HOME`, drop the mapping.
    Adopt {
        name: String,
        target: PathBuf,
        source: PathBuf,
    },
    /// `install --adopt` backed the local file up and linked the cloud copy.
    /// Reverse: drop the symlink, move the backup back over it. No mapping change.
    Backup {
        name: String,
        target: PathBuf,
        source: PathBuf,
        backup: PathBuf,
    },
    /// `unadopt` / `group remove` restored a real copy to `$HOME` and dropped the
    /// mapping. Reverse: drop the real copy, re-symlink the cloud copy, re-add the
    /// recorded mapping.
    Restore {
        name: String,
        target: PathBuf,
        source: PathBuf,
        mapping: Mapping,
    },
}

impl UndoAction {
    /// The mapping name this action concerns.
    pub fn name(&self) -> &str {
        match self {
            UndoAction::Adopt { name, .. }
            | UndoAction::Backup { name, .. }
            | UndoAction::Restore { name, .. } => name,
        }
    }
}

/// One recorded destructive run — a manifest on disk (`<id>.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    /// Milliseconds since the Unix epoch: the manifest filename and sort key.
    pub id: u128,
    /// The verb that produced it (`adopt`, `install`, `unadopt`, `group remove`),
    /// for `undo --list` and the confirmation prompt.
    pub command: String,
    /// The dotsync version that wrote this run.
    pub dotsync_version: String,
    /// The reversible actions, in the order they were applied.
    pub actions: Vec<UndoAction>,
}

/// How many recent runs to keep before pruning.
const KEEP_RUNS: usize = 10;

/// The default per-machine journal directory: `$DOTSYNC_STATE_DIR`, else
/// `$XDG_STATE_HOME/dotsync`, else `~/.local/state/dotsync`. `None` only when
/// `$HOME` is unset and no override is given.
pub fn default_dir() -> Option<PathBuf> {
    let non_empty = |value: std::ffi::OsString| (!value.is_empty()).then_some(value);
    if let Some(dir) = std::env::var_os("DOTSYNC_STATE_DIR").and_then(non_empty) {
        return Some(PathBuf::from(dir));
    }
    if let Some(state_home) = std::env::var_os("XDG_STATE_HOME").and_then(non_empty) {
        return Some(PathBuf::from(state_home).join("dotsync"));
    }
    std::env::var_os("HOME")
        .and_then(non_empty)
        .map(|home| PathBuf::from(home).join(".local/state/dotsync"))
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
}

/// Record a run of `actions` under `dir` as a `<id>.json` manifest, then prune to
/// the newest [`KEEP_RUNS`]. A no-op when `actions` is empty. The write is atomic
/// (temp + rename). The caller treats this as **best-effort**: a failed record must
/// never fail an already-successful mutation.
pub fn record(dir: &Path, command: &str, actions: Vec<UndoAction>) -> Result<()> {
    if actions.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    // A unique id even for two runs within the same millisecond.
    let mut id = now_millis();
    while dir.join(format!("{id}.json")).exists() {
        id += 1;
    }
    let run = Run {
        id,
        command: command.to_string(),
        dotsync_version: env!("CARGO_PKG_VERSION").to_string(),
        actions,
    };
    let body = serde_json::to_string_pretty(&run).context("serializing undo run")?;
    let path = dir.join(format!("{id}.json"));
    let tmp = fsutil::temp_sibling(&path);
    std::fs::write(&tmp, body).with_context(|| format!("could not write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("could not finalize {}", path.display()))?;
    prune(dir);
    Ok(())
}

/// Every recorded run under `dir`, newest first.
pub fn list(dir: &Path) -> Vec<Run> {
    let mut runs: Vec<Run> = read_ids(dir).into_iter().filter_map(|id| load(dir, id)).collect();
    runs.sort_by_key(|run| std::cmp::Reverse(run.id));
    runs
}

/// The most recent run, if any.
pub fn latest(dir: &Path) -> Option<Run> {
    read_ids(dir).into_iter().max().and_then(|id| load(dir, id))
}

fn read_ids(dir: &Path) -> Vec<u128> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            name.to_str()?.strip_suffix(".json")?.parse::<u128>().ok()
        })
        .collect()
}

fn load(dir: &Path, id: u128) -> Option<Run> {
    let text = std::fs::read_to_string(dir.join(format!("{id}.json"))).ok()?;
    serde_json::from_str(&text).ok()
}

fn prune(dir: &Path) {
    let mut ids = read_ids(dir);
    ids.sort_unstable();
    let excess = ids.len().saturating_sub(KEEP_RUNS);
    for id in ids.into_iter().take(excess) {
        let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
    }
}

/// Reverse the most recent run under `dir`. Loads the shared mappings once, undoes
/// each action in reverse order (skipping — never clobbering — anything whose
/// on-disk state no longer matches what dotsync left), saves the mappings once if
/// any changed, and (unless `dry_run`) consumes the manifest. `Ok(None)` when there
/// is nothing to undo. Returns the run (for its command label) and the per-action
/// outcomes for rendering.
pub fn revert(dir: &Path, cfg: &Config, dry_run: bool) -> Result<Option<(Run, Vec<Outcome>)>> {
    let Some(run) = latest(dir) else {
        return Ok(None);
    };
    let mappings_path = cfg.sync_dir.join(MappingsFile::FILE_NAME);
    let mut mappings = MappingsFile::load(&mappings_path)?;
    let mut mappings_changed = false;
    let mut outcomes = Vec::new();

    for action in run.actions.iter().rev() {
        let (outcome, changed) = revert_one(action, &mut mappings, dry_run);
        mappings_changed |= changed;
        outcomes.push(outcome);
    }

    if !dry_run {
        if mappings_changed {
            mappings.save(&mappings_path)?;
        }
        // Consume the manifest: a run is undone at most once.
        let _ = std::fs::remove_file(dir.join(format!("{}.json", run.id)));
    }
    Ok(Some((run, outcomes)))
}

/// Whether `target` is still exactly the symlink dotsync created into `source`.
fn is_our_symlink(target: &Path, source: &Path) -> bool {
    fsutil::is_symlink(target) && fsutil::read_link(target).ok().as_deref() == Some(source)
}

fn skipped(name: &str, why: &str) -> Outcome {
    Outcome::new(name, "skipped", true, why)
}

/// Reverse a single action, mutating `mappings` in memory. Returns the outcome and
/// whether the mappings file now needs saving. Guarded: if the on-disk state isn't
/// exactly what dotsync left, it skips (never clobbering) and says why.
fn revert_one(action: &UndoAction, mappings: &mut MappingsFile, dry_run: bool) -> (Outcome, bool) {
    match action {
        UndoAction::Adopt { name, target, source } => {
            if !is_our_symlink(target, source) {
                return (skipped(name, "target is no longer dotsync's symlink"), false);
            }
            if !fsutil::path_present(source) {
                return (skipped(name, "cloud copy is gone — nothing to move back"), false);
            }
            if dry_run {
                return (Outcome::new(name, "would-undo", true, "move back to $HOME, drop mapping"), true);
            }
            match fsutil::remove_symlink(target).and_then(|_| fsutil::move_path(source, target)) {
                Ok(_) => {
                    mappings.mappings.retain(|mapping| &mapping.name != name);
                    (Outcome::new(name, "un-adopted", true, "moved back to $HOME, mapping dropped"), true)
                }
                Err(error) => (Outcome::new(name, "error", false, error.to_string()), false),
            }
        }
        UndoAction::Backup { name, target, source, backup } => {
            if !is_our_symlink(target, source) {
                return (skipped(name, "target is no longer dotsync's symlink"), false);
            }
            if !fsutil::path_present(backup) {
                return (skipped(name, "backup is gone — nothing to restore"), false);
            }
            if dry_run {
                return (Outcome::new(name, "would-undo", true, format!("restore {}", backup.display())), false);
            }
            match fsutil::remove_symlink(target).and_then(|_| fsutil::move_path(backup, target)) {
                Ok(_) => (Outcome::new(name, "restored-backup", true, "put the backed-up file back"), false),
                Err(error) => (Outcome::new(name, "error", false, error.to_string()), false),
            }
        }
        UndoAction::Restore { name, target, source, mapping } => {
            if fsutil::is_symlink(target) {
                return (skipped(name, "already re-linked"), false);
            }
            if !fsutil::path_present(source) {
                return (skipped(name, "cloud copy is gone — cannot re-link"), false);
            }
            // The only clobber-risk path: refuse if the restored file changed since.
            if !fsutil::trees_equal(target, source) {
                return (skipped(name, "changed since it was restored — kept your version"), false);
            }
            if dry_run {
                return (Outcome::new(name, "would-undo", true, "re-link and restore the mapping"), true);
            }
            match fsutil::remove_path(target).and_then(|_| fsutil::make_symlink(source, target)) {
                Ok(_) => {
                    mappings.upsert(mapping.clone());
                    (Outcome::new(name, "re-linked", true, "restored the symlink and the mapping"), true)
                }
                Err(error) => (Outcome::new(name, "error", false, error.to_string()), false),
            }
        }
    }
}
