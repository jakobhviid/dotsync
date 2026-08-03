//! Computing the state of each mapping on this machine — the pure analysis that
//! `status`, the picker, `install`, and `doctor` all build on. No mutation here.

use std::fs;
use std::path::PathBuf;

use crate::config::Config;
use crate::fsutil;
use crate::mapping::{Mapping, MappingsFile};

/// The state of one mapping on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Does not apply on this OS.
    Skipped,
    /// Correctly symlinked into the sync folder. Active here.
    Linked,
    /// Present in the cloud, not linked here yet — available to adopt.
    Available,
    /// A real file/dir exists locally but not in the cloud — an adopt candidate.
    LocalOnly,
    /// A real file sits where our symlink should be, but its content matches the
    /// sync copy (classic atomic-save clobber). Safe to silently relink.
    Healable,
    /// A real local file/dir and the sync copy both exist and differ. A conflict.
    Diverged,
    /// Symlinked into the sync folder, but the sync copy is missing (cloud not
    /// downloaded yet, or the source was deleted).
    DanglingSelf,
    /// A symlink pointing somewhere other than our sync copy.
    ForeignSymlink(PathBuf),
    /// Neither a local target nor a sync copy exists.
    Missing,
}

impl State {
    /// A short machine-readable status token.
    pub fn code(&self) -> &'static str {
        match self {
            State::Skipped => "skipped",
            State::Linked => "linked",
            State::Available => "available",
            State::LocalOnly => "local-only",
            State::Healable => "healable",
            State::Diverged => "diverged",
            State::DanglingSelf => "dangling",
            State::ForeignSymlink(_) => "foreign-symlink",
            State::Missing => "missing",
        }
    }

    /// Whether this mapping is currently active (linked) on this machine.
    pub fn is_linked(&self) -> bool {
        matches!(self, State::Linked)
    }

    /// Whether this state represents a problem worth surfacing in `doctor`.
    pub fn is_problem(&self) -> bool {
        matches!(
            self,
            State::Diverged | State::DanglingSelf | State::ForeignSymlink(_)
        )
    }
}

/// A mapping paired with its resolved paths and computed state.
#[derive(Debug, Clone)]
pub struct Item {
    pub mapping: Mapping,
    /// `$HOME` target on this OS (None when skipped on this OS).
    pub target: Option<PathBuf>,
    /// Path inside the sync folder.
    pub source: PathBuf,
    pub state: State,
}

impl Item {
    pub fn name(&self) -> &str {
        &self.mapping.name
    }

    pub fn is_secret(&self) -> bool {
        self.mapping.mode.is_some()
    }
}

/// Compute the state of one mapping.
pub fn state_of(mapping: &Mapping, cfg: &Config, os: &str) -> Item {
    let source = mapping.source(&cfg.sync_dir);
    let source_present = fsutil::path_present(&source);

    let Some(target) = mapping.target_for(&cfg.home, os) else {
        return Item {
            mapping: mapping.clone(),
            target: None,
            source,
            state: State::Skipped,
        };
    };

    let state = match fs::symlink_metadata(&target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let dest = fsutil::read_link(&target).unwrap_or_default();
            if dest == source {
                if source_present {
                    State::Linked
                } else {
                    State::DanglingSelf
                }
            } else if source_present && fsutil::same_location(&dest, &source) {
                // The target string differs but resolves to our cloud copy (a
                // symlinked/differently-normalized sync path) — still ours.
                State::Linked
            } else {
                State::ForeignSymlink(dest)
            }
        }
        Ok(_) => {
            // A real file or dir sits at the target.
            if source_present {
                // Identical content — matching file bytes, or a whole directory
                // tree — is a healable atomic-save clobber; else a real conflict.
                if fsutil::trees_equal(&target, &source) {
                    State::Healable
                } else {
                    State::Diverged
                }
            } else {
                State::LocalOnly
            }
        }
        Err(_) => {
            if source_present {
                State::Available
            } else {
                State::Missing
            }
        }
    };

    Item {
        mapping: mapping.clone(),
        target: Some(target),
        source,
        state,
    }
}

/// Compute the state of every mapping in the file.
pub fn plan(mappings: &MappingsFile, cfg: &Config, os: &str) -> Vec<Item> {
    mappings
        .mappings
        .iter()
        .map(|mapping| state_of(mapping, cfg, os))
        .collect()
}
