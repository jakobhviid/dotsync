//! Health checks and safe self-healing. `doctor` surfaces the states that need
//! attention — atomic-save clobbers, conflicts, dangling/foreign links, secret
//! mode drift, and cloud "conflicted copy" siblings — and with `fix` repairs the
//! ones that are safe to repair automatically.

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

        // Secret mode drift (independent of the link state).
        if item.mapping.mode.is_some() {
            if fix {
                if let Some(out) = apply::enforce_mode(cfg, item, false) {
                    report.fixed.push(out);
                }
            } else if let Some(bits) = item.mapping.mode_bits() {
                if let Some(cur) = crate::fsutil::mode_of(&item.source) {
                    if cur != bits {
                        report.issues.push(Issue {
                            name: item.name().to_string(),
                            level: Level::Warn,
                            message: format!(
                                "secret mode drifted to {:04o}, should be {:04o} (fixable: chmod)",
                                cur, bits
                            ),
                            fixable: true,
                        });
                    }
                }
            }
        }
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

    // Cloud "conflicted copy" siblings anywhere in the sync folder.
    let mut conflicts = Vec::new();
    scan_conflicted(&cfg.sync_dir, &cfg.sync_dir, &mut conflicts);
    for rel in conflicts {
        report.issues.push(Issue {
            name: rel,
            level: Level::Warn,
            message: "looks like a cloud sync-conflict copy — reconcile and delete it".into(),
            fixable: false,
        });
    }

    Ok(report)
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
