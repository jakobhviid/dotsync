//! Health checks and safe self-healing. `doctor` surfaces the states that need
//! attention — atomic-save clobbers, conflicts, dangling/foreign links, secret
//! mode drift, cloud "conflicted copy" siblings, and cross-machine drift (orphan
//! links whose mapping was removed elsewhere, partially-linked groups) — and with
//! `fix` repairs the ones that are safe to repair automatically. Drift is
//! surfaced read-only: an advisory, never an automatic mutation.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::Result;

use crate::apply::{self, Outcome};
use crate::config::Config;
use crate::mapping::MappingsFile;
use crate::plan::{plan, State};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Warn,
    Error,
}

/// One thing worth the user's attention.
#[derive(Debug, Clone)]
pub struct Issue {
    pub name: String,
    pub level: Level,
    pub message: String,
    /// Whether `dotsync doctor --fix` can repair it automatically.
    pub fixable: bool,
}

/// The result of a doctor run.
#[derive(Debug, Default)]
pub struct Report {
    pub issues: Vec<Issue>,
    pub fixed: Vec<Outcome>,
}

impl Report {
    /// Healthy = no *error-level* problems. Warn-level advisories are surfaced
    /// to the human but don't make a machine "unhealthy" (they're often
    /// transient, e.g. a cloud copy not downloaded yet).
    pub fn healthy(&self) -> bool {
        !self.issues.iter().any(|i| i.level == Level::Error)
    }

    pub fn errors(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.level == Level::Error)
    }

    pub fn advisories(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.level == Level::Warn)
    }
}

/// Scan for problems and, when `fix` is set, repair the safe ones.
pub fn run(cfg: &Config, mappings: &MappingsFile, os: &str, fix: bool) -> Result<Report> {
    let mut report = Report::default();
    let items = plan(mappings, cfg, os);

    for item in &items {
        match &item.state {
            State::Healable => {
                if fix {
                    report.fixed.push(apply::link_item(item, false));
                } else {
                    report.issues.push(Issue {
                        name: item.name().to_string(),
                        level: Level::Warn,
                        message: "an atomic-save replaced the symlink with a real file; content still matches the cloud copy (fixable: relink)".into(),
                        fixable: true,
                    });
                }
            }
            State::Diverged => report.issues.push(Issue {
                name: item.name().to_string(),
                level: Level::Error,
                message: "local file differs from the cloud copy — resolve by hand, or set on_conflict = \"adopt\"".into(),
                fixable: false,
            }),
            State::DanglingSelf => report.issues.push(Issue {
                name: item.name().to_string(),
                level: Level::Warn,
                message: "symlinked into the cloud, but the cloud copy is missing (not downloaded yet, or deleted)".into(),
                fixable: false,
            }),
            State::ForeignSymlink(dest) => report.issues.push(Issue {
                name: item.name().to_string(),
                level: Level::Warn,
                message: format!("target is a symlink to {} — not managed by dotsync", dest.display()),
                fixable: false,
            }),
            _ => {}
        }

        // Secret perms: recursive open-mode drift, plus the target must never
        // sit inside a git working tree (git would commit the secret).
        if item.mapping.mode.is_some() {
            if fix {
                if let Some(out) = apply::enforce_mode(cfg, item, false) {
                    report.fixed.push(out);
                }
            } else if crate::fsutil::any_too_open(&item.source) {
                report.issues.push(Issue {
                    name: item.name().to_string(),
                    level: Level::Warn,
                    message: "secret perms are group/world-accessible somewhere in the tree (fixable: chmod to 0700/0600)".into(),
                    fixable: true,
                });
            }
            if let Some(target) = &item.target {
                if let Some(repo) = crate::config::enclosing_git_tree(target) {
                    report.issues.push(Issue {
                        name: item.name().to_string(),
                        level: Level::Error,
                        message: format!(
                            "secret's target is inside a git repository ({}) — git could commit it; move it out of the repo",
                            repo.display()
                        ),
                        fixable: false,
                    });
                }
            }
        }
    }

    // The sync folder must never be inside a git tree (it can be created/nested
    // after `init`, so re-check here, not just at setup).
    if let Some(repo) = crate::config::enclosing_git_tree(&cfg.sync_dir) {
        report.issues.push(Issue {
            name: cfg.sync_dir.display().to_string(),
            level: Level::Error,
            message: format!(
                "sync folder is inside a git repository ({}) — synced files and secrets could be committed",
                repo.display()
            ),
            fixable: false,
        });
    }

    // World/group-accessible sync folder = weak protection for any secrets in it.
    if let Some(mode) = crate::fsutil::mode_of(&cfg.sync_dir) {
        if mode & 0o077 != 0 && items.iter().any(|i| i.mapping.mode.is_some()) {
            report.issues.push(Issue {
                name: cfg.sync_dir.display().to_string(),
                level: Level::Warn,
                message: format!(
                    "sync folder is group/world-accessible ({:04o}) but holds secrets — consider `chmod 700`",
                    mode
                ),
                fixable: false,
            });
        }
    }

    // The dangerous conflicted copy is one of the mappings file itself — the
    // shared mapping list forked. Catch the cloud-specific names the generic
    // scan misses (OneDrive `dotsync-HOST.toml`, iCloud `dotsync 2.toml`) by
    // matching any `dotsync*.toml` at the sync root that isn't the real one.
    let mut mapping_conflicts = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&cfg.sync_dir) {
        for entry in entries.flatten() {
            let fname = entry.file_name().to_string_lossy().into_owned();
            let lower = fname.to_ascii_lowercase();
            if lower.starts_with("dotsync")
                && lower.ends_with(".toml")
                && lower != MappingsFile::FILE_NAME
            {
                mapping_conflicts.push(fname);
            }
        }
    }
    for fname in &mapping_conflicts {
        report.issues.push(Issue {
            name: fname.clone(),
            level: Level::Warn,
            message: "a conflicted copy of dotsync.toml — two machines edited the mapping list. \
                      Union its [[mapping]] entries into dotsync.toml (by name) and delete this copy"
                .into(),
            fixable: false,
        });
    }

    // Other cloud "conflicted copy" siblings anywhere in the sync folder.
    let mut conflicts = Vec::new();
    scan_conflicted(&cfg.sync_dir, &cfg.sync_dir, &mut conflicts);
    for rel in conflicts {
        if mapping_conflicts.contains(&rel) {
            continue; // already reported with the specific mappings-file message
        }
        report.issues.push(Issue {
            name: rel,
            level: Level::Warn,
            message: "looks like a cloud sync-conflict copy — reconcile and delete it".into(),
            fixable: false,
        });
    }

    // Drift: a group you use here gained members on another machine. If some of a
    // group's members are linked and others are only Available, the newcomers
    // synced in but were never linked here. Read-only advisory.
    let mut by_group: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for item in &items {
        if let Some(g) = item.mapping.group.as_deref() {
            let counts = by_group.entry(g).or_default();
            match item.state {
                State::Linked => counts.0 += 1,
                State::Available => counts.1 += 1,
                _ => {}
            }
        }
    }
    for (group, (linked, available)) in by_group {
        if linked > 0 && available > 0 {
            report.issues.push(Issue {
                name: group.to_string(),
                level: Level::Warn,
                message: format!(
                    "group is partially linked here: {available} member(s) available but not linked \
                     (run `dotsync install {group}` to link them)"
                ),
                fixable: false,
            });
        }
    }

    // Drift: orphan symlinks. A `$HOME` symlink pointing into the sync folder for
    // which no mapping exists — typically left behind when the mapping was
    // removed on another machine (the removal synced, the symlink didn't).
    // `plan()` only walks `mappings`, so these are otherwise invisible.
    let mut orphans = Vec::new();
    scan_orphans(&cfg.sync_dir, &cfg.sync_dir, cfg, mappings, &mut orphans);
    for rel in orphans {
        report.issues.push(Issue {
            name: rel,
            level: Level::Warn,
            message: "symlinked into the cloud folder but no mapping covers it (drift from another \
                      machine) — `dotsync adopt` it to re-track, or remove the symlink"
                .into(),
            fixable: false,
        });
    }

    Ok(report)
}

/// Whether a home-relative path is covered by a mapping — the mapping itself, or
/// an ancestor directory that is mapped as a whole.
fn covered(rel: &str, mappings: &MappingsFile) -> bool {
    mappings
        .mappings
        .iter()
        .any(|m| m.name == rel || rel.starts_with(&format!("{}/", m.name)))
}

/// Walk the cloud tree looking for orphan links: a cloud path whose matching
/// `$HOME` location is a symlink pointing back at it, with no mapping covering
/// it. Prunes at mapped roots (their whole subtree is covered) and never
/// descends through symlinks.
fn scan_orphans(
    root: &Path,
    dir: &Path,
    cfg: &Config,
    mappings: &MappingsFile,
    out: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if rel == MappingsFile::FILE_NAME {
            continue;
        }
        // This path is itself a mapping → the whole subtree is covered; skip it.
        if mappings.mappings.iter().any(|m| m.name == rel) {
            continue;
        }
        let home_path = cfg.home.join(&rel);
        if crate::fsutil::is_symlink(&home_path)
            && crate::fsutil::read_link(&home_path).ok().as_deref() == Some(path.as_path())
            && !covered(&rel, mappings)
        {
            out.push(rel);
            continue; // don't descend into the orphan's own subtree
        }
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
            && !entry.file_type().map(|t| t.is_symlink()).unwrap_or(false)
        {
            scan_orphans(root, &path, cfg, mappings, out);
        }
    }
}

/// Recursively collect files whose names look like cloud sync-conflict copies.
fn scan_conflicted(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.contains("conflicted copy")
            || name.contains("sync-conflict")
            || name.contains("(case conflict)")
            || name.contains("conflicted-copy")
        {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            out.push(rel.to_string_lossy().into_owned());
        }
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
            && !entry
                .file_type()
                .map(|t| t.is_symlink())
                .unwrap_or(false)
        {
            scan_conflicted(root, &path, out);
        }
    }
}

/// Permissions of a path as an octal string (for display).
pub fn mode_string(path: &Path) -> String {
    std::fs::metadata(path)
        .map(|m| format!("{:04o}", m.permissions().mode() & 0o7777))
        .unwrap_or_else(|_| "????".into())
}
