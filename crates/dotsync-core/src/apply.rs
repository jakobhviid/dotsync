//! The mutating operations: adopt (local → cloud), link/unlink (cloud ↔ home),
//! and mode enforcement. Each returns an [`Outcome`] so the CLI can render it
//! either as a human line or as JSON, and honors `dry_run` (compute, don't
//! touch the filesystem).

use std::path::Path;

use anyhow::{anyhow, bail, Result};

use crate::config::Config;
use crate::fsutil;
use crate::journal::UndoAction;
use crate::mapping::{Mapping, OnConflict};
use crate::plan::{state_of, Item, State};

/// The result of acting on one mapping.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub name: String,
    pub action: String,
    pub ok: bool,
    pub detail: String,
    /// How to reverse this action, when it is a reversible destructive op. Consumed
    /// by the undo journal ([`crate::journal`]); never rendered, so `--json` output
    /// is unaffected.
    pub undo: Option<UndoAction>,
}

impl Outcome {
    pub(crate) fn new(name: &str, action: &str, ok: bool, detail: impl Into<String>) -> Self {
        Outcome {
            name: name.to_string(),
            action: action.to_string(),
            ok,
            detail: detail.into(),
            undo: None,
        }
    }

    /// Attach the action that reverses this outcome (for the undo journal).
    pub(crate) fn with_undo(mut self, undo: UndoAction) -> Self {
        self.undo = Some(undo);
        self
    }
}

/// Adopt a real path under the home base into the sync folder and symlink it
/// back. Returns the mapping to record plus the outcome. The caller persists the
/// mapping into `dotsync.toml`.
/// The home-relative mapping name a path would adopt as (its mirror-path form,
/// e.g. `~/.config/zed` → `.config/zed`). Errors if the path isn't under the
/// home base or is the home base itself.
pub fn mapping_name_for(cfg: &Config, abs_target: &Path) -> Result<String> {
    let rel = abs_target.strip_prefix(&cfg.home).map_err(|_| {
        anyhow!(
            "{} is not under the home base {} — dotsync can only adopt paths inside it",
            abs_target.display(),
            cfg.home.display()
        )
    })?;
    if rel.as_os_str().is_empty() {
        bail!("refusing to adopt the home base itself");
    }
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

pub fn adopt(
    cfg: &Config,
    abs_target: &Path,
    os_scope: Option<&str>,
    group: Option<String>,
    existing: &[String],
    dry_run: bool,
) -> Result<(Mapping, Outcome)> {
    if !fsutil::path_present(abs_target) {
        bail!("nothing exists at {}", abs_target.display());
    }
    if abs_target.starts_with(&cfg.sync_dir) {
        bail!("{} is already inside the sync folder", abs_target.display());
    }
    let name = mapping_name_for(cfg, abs_target)?;
    let source = cfg.sync_dir.join(&name);

    // Refuse overlaps: a path that contains, or is contained by, an existing
    // mapping. (An exact match is a re-adopt/regroup, handled below.)
    for existing_name in existing {
        if *existing_name == name {
            continue;
        }
        if name.starts_with(&format!("{existing_name}/")) {
            bail!("{name} is already covered by mapping {existing_name} — nothing to adopt separately");
        }
        if existing_name.starts_with(&format!("{name}/")) {
            bail!("{name} would contain the existing mapping {existing_name} — adopt specific items, not the whole directory");
        }
    }

    // Auto-tag a restrictive mode for known-secret paths (dirs 0700, files 0600).
    let mode = if fsutil::looks_secret(&name) {
        Some(if abs_target.is_dir() { "0700" } else { "0600" }.to_string())
    } else {
        None
    };

    let mut mapping = Mapping::new(name.clone());
    mapping.mode = mode.clone();
    mapping.group = group;
    match os_scope {
        Some("mac") => mapping.target_mac = Some(format!("~/{name}")),
        Some("linux") => mapping.target_linux = Some(format!("~/{name}")),
        _ => {}
    }

    // Already adopted here (symlink already points at the sync copy): don't touch
    // the filesystem — just carry the (possibly new) group/mode into the mapping.
    if fsutil::is_symlink(abs_target) && fsutil::read_link(abs_target).ok().as_deref() == Some(source.as_path()) {
        let action = if mapping.group.is_some() { "grouped" } else { "already-adopted" };
        return Ok((mapping, Outcome::new(&name, action, true, "")));
    }

    // Refuse a path that already resolves *inside* the sync folder — e.g. a path
    // under an ancestor already symlinked into the cloud. Adopting it would
    // compare the cloud master against itself and could delete it.
    if let Ok(canon) = std::fs::canonicalize(abs_target) {
        let canon_sync =
            std::fs::canonicalize(&cfg.sync_dir).unwrap_or_else(|_| cfg.sync_dir.clone());
        if canon.starts_with(&canon_sync) {
            bail!(
                "{} already resolves inside the sync folder (it's under something already \
                 synced) — nothing to adopt; use `dotsync install` to link it here",
                abs_target.display()
            );
        }
    }

    // If the cloud copy already exists, this path was adopted from another
    // machine. Reconcile rather than clobber.
    if fsutil::path_present(&source) {
        // Content matches the existing cloud copy (file bytes or a whole
        // directory tree) → adopt from another machine: just relink, no clobber.
        if fsutil::trees_equal(abs_target, &source) {
            if dry_run {
                return Ok((mapping, Outcome::new(&name, "would-relink", true, "content matches cloud")));
            }
            fsutil::remove_path(abs_target)?;
            fsutil::make_symlink(&source, abs_target)?;
            return Ok((mapping, Outcome::new(&name, "relinked", true, "matched existing cloud copy")));
        }
        bail!(
            "{} already exists in the cloud folder and differs from the local copy — \
             resolve by hand, or delete one side",
            name
        );
    }

    if dry_run {
        return Ok((
            mapping,
            Outcome::new(&name, "would-adopt", true, "move into cloud, link back"),
        ));
    }
    fsutil::move_path(abs_target, &source)?;
    if mapping.mode.is_some() {
        fsutil::enforce_secret_tree(&source)?;
    }
    fsutil::make_symlink(&source, abs_target)?;
    let undo = UndoAction::Adopt {
        name: name.clone(),
        target: abs_target.to_path_buf(),
        source: source.clone(),
    };
    Ok((
        mapping,
        Outcome::new(&name, "adopted", true, source.display().to_string()).with_undo(undo),
    ))
}

/// Make one item linked on this machine, according to its state and conflict
/// policy. Self-heals atomic-save clobbers.
pub fn link_item(item: &Item, dry_run: bool) -> Outcome {
    let name = item.name().to_string();
    let Some(target) = item.target.clone() else {
        return Outcome::new(&name, "skipped", true, "not for this OS");
    };
    let source = &item.source;

    macro_rules! done {
        ($action:expr, $ok:expr, $detail:expr) => {{
            if dry_run {
                return Outcome::new(&name, concat!("would-", $action), $ok, $detail);
            }
        }};
    }

    let result = match &item.state {
        State::Skipped => return Outcome::new(&name, "skipped", true, "not for this OS"),
        State::Linked => return Outcome::new(&name, "already-linked", true, ""),
        State::Missing => return Outcome::new(&name, "skipped", true, "nothing to link"),
        State::LocalOnly => {
            return Outcome::new(&name, "skipped", true, "local only — run `dotsync adopt`")
        }
        State::DanglingSelf => {
            if dry_run {
                return Outcome::new(&name, "would-skip", true, "cloud copy missing — nothing to link yet");
            }
            return Outcome::new(
                &name,
                "warn",
                false,
                "cloud copy missing — wait for sync, or re-adopt",
            );
        }
        State::Available => {
            done!("link", true, "");
            fsutil::make_symlink(source, &target).map(|_| ("linked", String::new(), None))
        }
        State::Healable => {
            done!("relink", true, "content matches cloud");
            // Re-check right before deleting the real file: if an atomic save
            // changed it since planning it no longer matches the cloud copy, so
            // treat it as a conflict rather than destroying unsynced content.
            if !fsutil::files_equal(&target, source) {
                return Outcome::new(
                    &name,
                    "conflict",
                    false,
                    "changed since planning — no longer matches the cloud copy; resolve by hand",
                );
            }
            fsutil::remove_path(&target)
                .and_then(|_| fsutil::make_symlink(source, &target))
                .map(|_| ("relinked", "healed atomic-save clobber".to_string(), None))
        }
        State::Diverged => match item.mapping.on_conflict {
            OnConflict::Fail => {
                if dry_run {
                    return Outcome::new(&name, "would-skip", true, "conflict — resolve, or set on_conflict = \"adopt\"");
                }
                return Outcome::new(
                    &name,
                    "conflict",
                    false,
                    "local differs from cloud — resolve, or set on_conflict = \"adopt\"",
                );
            }
            OnConflict::Adopt => {
                done!("adopt", true, "back up local, link cloud");
                let bak = fsutil::backup_path(&target);
                // Record the exact backup path now — `backup_path` is stateful and
                // can't be re-derived after the file is in place (undo needs it).
                let undo = UndoAction::Backup {
                    name: name.clone(),
                    target: target.clone(),
                    source: source.clone(),
                    backup: bak.clone(),
                };
                fsutil::move_path(&target, &bak)
                    .and_then(|_| fsutil::make_symlink(source, &target))
                    .map(|_| ("backed-up-linked", format!("local → {}", bak.display()), Some(undo)))
            }
        },
        State::ForeignSymlink(dest) => match item.mapping.on_conflict {
            OnConflict::Fail => {
                if dry_run {
                    return Outcome::new(&name, "would-skip", true, format!("foreign symlink to {} — resolve by hand", dest.display()));
                }
                return Outcome::new(
                    &name,
                    "conflict",
                    false,
                    format!("target is a symlink to {} — resolve by hand", dest.display()),
                );
            }
            OnConflict::Adopt => {
                done!("adopt", true, "replace foreign symlink");
                let bak = fsutil::backup_path(&target);
                let undo = UndoAction::Backup {
                    name: name.clone(),
                    target: target.clone(),
                    source: source.clone(),
                    backup: bak.clone(),
                };
                fsutil::move_path(&target, &bak)
                    .and_then(|_| fsutil::make_symlink(source, &target))
                    .map(|_| ("backed-up-linked", format!("old link → {}", bak.display()), Some(undo)))
            }
        },
    };

    match result {
        Ok((action, detail, undo)) => {
            // Enforce secret perms on the freshly linked source (recursively).
            let outcome = if item.mapping.mode.is_some() && fsutil::enforce_secret_tree(source).is_err() {
                Outcome::new(&name, action, true, format!("{detail} (mode not set)"))
            } else {
                Outcome::new(&name, action, true, detail)
            };
            match undo {
                Some(undo) => outcome.with_undo(undo),
                None => outcome,
            }
        }
        Err(error) => Outcome::new(&name, "error", false, error.to_string()),
    }
}

/// Remove our symlink for one item (leaving the cloud copy in place).
pub fn unlink_item(item: &Item, dry_run: bool) -> Outcome {
    let name = item.name().to_string();
    match &item.state {
        State::Linked | State::DanglingSelf => {
            let Some(target) = item.target.clone() else {
                return Outcome::new(&name, "skipped", true, "");
            };
            if dry_run {
                return Outcome::new(&name, "would-unlink", true, "");
            }
            // Re-check we're still removing a symlink, not a real file an atomic
            // save dropped here since planning — never delete unsynced content.
            if !fsutil::is_symlink(&target) {
                return Outcome::new(
                    &name,
                    "skipped",
                    true,
                    "no longer a symlink (changed since planning) — re-run to re-check",
                );
            }
            match fsutil::remove_symlink(&target) {
                Ok(_) => Outcome::new(&name, "unlinked", true, ""),
                Err(error) => Outcome::new(&name, "error", false, error.to_string()),
            }
        }
        _ => Outcome::new(&name, "skipped", true, "no dotsync symlink here"),
    }
}

/// Restore a member to a real file/dir in `$HOME` (as part of un-managing it),
/// leaving the cloud copy untouched. Safe ordering: copy the cloud copy to a
/// temp sibling, then swap the symlink for it — so a failure mid-restore loses
/// nothing (the cloud copy is always kept, the symlink stays until the swap).
pub fn restore_item(item: &Item, dry_run: bool) -> Outcome {
    let name = item.name().to_string();
    let Some(target) = item.target.clone() else {
        return Outcome::new(&name, "skipped", true, "not for this OS");
    };
    match &item.state {
        State::Linked => {
            if !fsutil::path_present(&item.source) {
                return Outcome::new(&name, "error", false, "cloud copy missing — cannot restore");
            }
            if dry_run {
                return Outcome::new(&name, "would-restore", true, "copy cloud → $HOME, keep cloud copy");
            }
            let tmp = fsutil::temp_sibling(&target);
            let _ = fsutil::remove_path(&tmp);
            if let Err(error) = fsutil::copy_recursive(&item.source, &tmp) {
                let _ = fsutil::remove_path(&tmp);
                return Outcome::new(&name, "error", false, format!("restore copy failed: {error}"));
            }
            // Swap only after the copy succeeded: drop the symlink, move the real
            // copy into its place (a same-dir rename).
            if let Err(error) = fsutil::remove_symlink(&target).and_then(|_| fsutil::move_path(&tmp, &target)) {
                let _ = fsutil::remove_path(&tmp);
                return Outcome::new(&name, "error", false, error.to_string());
            }
            let undo = UndoAction::Restore {
                name: name.clone(),
                target: target.clone(),
                source: item.source.clone(),
                mapping: item.mapping.clone(),
            };
            Outcome::new(&name, "restored", true, "real copy in $HOME; cloud copy kept").with_undo(undo)
        }
        State::DanglingSelf => {
            if dry_run {
                return Outcome::new(&name, "would-unlink", true, "cloud copy missing");
            }
            match fsutil::remove_symlink(&target) {
                Ok(_) => Outcome::new(&name, "unlinked", true, "cloud copy was missing"),
                Err(error) => Outcome::new(&name, "error", false, error.to_string()),
            }
        }
        // Not linked on this machine: nothing to restore here; the caller still
        // drops it from dotsync.toml (that's the cross-machine "stop managing").
        _ => Outcome::new(&name, "skipped", true, "not linked here"),
    }
}

/// Re-assert secret perms on an item's sync copy (recursively). Returns an
/// outcome only when something is (or would be) tightened.
pub fn enforce_mode(cfg: &Config, item: &Item, dry_run: bool) -> Option<Outcome> {
    let _ = cfg;
    if item.mapping.mode.is_none() || !fsutil::path_present(&item.source) {
        return None;
    }
    if !fsutil::any_too_open(&item.source) {
        return None; // already tight
    }
    let name = item.name().to_string();
    if dry_run {
        return Some(Outcome::new(&name, "would-chmod", true, "tighten secret perms"));
    }
    match fsutil::enforce_secret_tree(&item.source) {
        Ok(_) => Some(Outcome::new(&name, "chmod", true, "tightened to 0700/0600")),
        Err(error) => Some(Outcome::new(&name, "error", false, error.to_string())),
    }
}

/// Convenience: recompute an item's state (used after mutation for reporting).
pub fn refresh(item: &Item, cfg: &Config, os: &str) -> Item {
    state_of(&item.mapping, cfg, os)
}
