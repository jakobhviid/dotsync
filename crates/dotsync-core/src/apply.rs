//! The mutating operations: adopt (local → cloud), link/unlink (cloud ↔ home),
//! and mode enforcement. Each returns an [`Outcome`] so the CLI can render it
//! either as a human line or as JSON, and honors `dry_run` (compute, don't
//! touch the filesystem).

use std::path::Path;

use anyhow::{anyhow, bail, Result};

use crate::config::Config;
use crate::fsutil;
use crate::mapping::{Mapping, OnConflict};
use crate::plan::{state_of, Item, State};

/// The result of acting on one mapping.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub name: String,
    pub action: String,
    pub ok: bool,
    pub detail: String,
}

impl Outcome {
    fn new(name: &str, action: &str, ok: bool, detail: impl Into<String>) -> Self {
        Outcome {
            name: name.to_string(),
            action: action.to_string(),
            ok,
            detail: detail.into(),
        }
    }
}

/// Adopt a real path under the home base into the sync folder and symlink it
/// back. Returns the mapping to record plus the outcome. The caller persists the
/// mapping into `dotsync.toml`.
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
    let rel = abs_target
        .strip_prefix(&cfg.home)
        .map_err(|_| anyhow!(
            "{} is not under the home base {} — dotsync can only adopt paths inside it",
            abs_target.display(),
            cfg.home.display()
        ))?;
    if rel.as_os_str().is_empty() {
        bail!("refusing to adopt the home base itself");
    }
    if abs_target.starts_with(&cfg.sync_dir) {
        bail!("{} is already inside the sync folder", abs_target.display());
    }
    let name = rel.to_string_lossy().replace('\\', "/");
    let source = cfg.sync_dir.join(&name);

    // Refuse overlaps: a path that contains, or is contained by, an existing
    // mapping. (An exact match is a re-adopt/regroup, handled below.)
    for e in existing {
        if *e == name {
            continue;
        }
        if name.starts_with(&format!("{e}/")) {
            bail!("{name} is already covered by mapping {e} — nothing to adopt separately");
        }
        if e.starts_with(&format!("{name}/")) {
            bail!("{name} would contain the existing mapping {e} — adopt specific items, not the whole directory");
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
        if abs_target.is_file()
            && source.is_file()
            && fsutil::files_equal(abs_target, &source)
        {
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
    Ok((mapping, Outcome::new(&name, "adopted", true, format!("→ {}", source.display()))))
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
            return Outcome::new(
                &name,
                "warn",
                false,
                "cloud copy missing — wait for sync, or re-adopt",
            )
        }
        State::Available => {
            done!("link", true, "");
            fsutil::make_symlink(source, &target).map(|_| ("linked", String::new()))
        }
        State::Healable => {
            done!("relink", true, "content matches cloud");
            fsutil::remove_path(&target)
                .and_then(|_| fsutil::make_symlink(source, &target))
                .map(|_| ("relinked", "healed atomic-save clobber".to_string()))
        }
        State::Diverged => match item.mapping.on_conflict {
            OnConflict::Fail => {
                return Outcome::new(
                    &name,
                    "conflict",
                    false,
                    "local differs from cloud — resolve, or set on_conflict = \"adopt\"",
                )
            }
            OnConflict::Adopt => {
                done!("adopt", true, "back up local, link cloud");
                let bak = fsutil::backup_path(&target);
                fsutil::move_path(&target, &bak)
                    .and_then(|_| fsutil::make_symlink(source, &target))
                    .map(|_| ("backed-up-linked", format!("local → {}", bak.display())))
            }
        },
        State::ForeignSymlink(dest) => match item.mapping.on_conflict {
            OnConflict::Fail => {
                return Outcome::new(
                    &name,
                    "conflict",
                    false,
                    format!("target is a symlink to {} — resolve by hand", dest.display()),
                )
            }
            OnConflict::Adopt => {
                done!("adopt", true, "replace foreign symlink");
                let bak = fsutil::backup_path(&target);
                fsutil::move_path(&target, &bak)
                    .and_then(|_| fsutil::make_symlink(source, &target))
                    .map(|_| ("backed-up-linked", format!("old link → {}", bak.display())))
            }
        },
    };

    match result {
        Ok((action, detail)) => {
            // Enforce secret perms on the freshly linked source (recursively).
            if item.mapping.mode.is_some() && fsutil::enforce_secret_tree(source).is_err() {
                return Outcome::new(&name, action, true, format!("{detail} (mode not set)"));
            }
            Outcome::new(&name, action, true, detail)
        }
        Err(e) => Outcome::new(&name, "error", false, e.to_string()),
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
            match fsutil::remove_symlink(&target) {
                Ok(_) => Outcome::new(&name, "unlinked", true, ""),
                Err(e) => Outcome::new(&name, "error", false, e.to_string()),
            }
        }
        _ => Outcome::new(&name, "skipped", true, "no dotsync symlink here"),
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
        Err(e) => Some(Outcome::new(&name, "error", false, e.to_string())),
    }
}

/// Convenience: recompute an item's state (used after mutation for reporting).
pub fn refresh(item: &Item, cfg: &Config, os: &str) -> Item {
    state_of(&item.mapping, cfg, os)
}
