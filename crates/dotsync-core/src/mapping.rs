//! The mapping model and the `dotsync.toml` document that lives *inside* the
//! sync folder (so the mapping list itself syncs across machines).
//!
//! Layout convention: the sync folder mirrors `$HOME`. A mapping's `name` is its
//! path relative to the configured home base (e.g. `.config/zed`), which is also
//! where it lives inside the sync folder. For the common case that's all a
//! mapping needs — the `$HOME` target is derived from `name`. Per-OS overrides
//! and an explicit `target` cover the cases where the path differs.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// What to do at link time when the `$HOME` target and the sync copy both exist
/// as real, *differing* content — a genuine conflict, not an atomic-save
/// artifact (those are healed automatically; see [`crate::plan`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OnConflict {
    /// Refuse and let the user resolve it by hand. The safe default.
    #[default]
    Fail,
    /// The sync copy wins: back the local target up to `<target>.bak`, then link.
    Adopt,
}

impl OnConflict {
    pub fn as_str(&self) -> &'static str {
        match self {
            OnConflict::Fail => "fail",
            OnConflict::Adopt => "adopt",
        }
    }
}

fn is_default_conflict(c: &OnConflict) -> bool {
    *c == OnConflict::Fail
}

/// One synced path.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Mapping {
    /// Path relative to the home base, e.g. `.config/zed`. Also the item's path
    /// inside the sync folder (mirror layout).
    pub name: String,
    /// Explicit `$HOME` target for all OSes. `~` is expanded. When omitted the
    /// target is derived as `<home>/<name>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// macOS-only target override (wins over `target` on mac).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_mac: Option<String>,
    /// Linux-only target override (wins over `target` on linux).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_linux: Option<String>,
    /// Octal file/dir mode to enforce on the sync copy, e.g. `0600` for secrets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Conflict behavior. Absent = `fail`.
    #[serde(default, skip_serializing_if = "is_default_conflict")]
    pub on_conflict: OnConflict,
}

impl Mapping {
    /// A mapping with only a name; target derived from the mirror layout.
    pub fn new(name: impl Into<String>) -> Self {
        Mapping {
            name: name.into(),
            target: None,
            target_mac: None,
            target_linux: None,
            mode: None,
            on_conflict: OnConflict::Fail,
        }
    }

    fn has_os_specific(&self) -> bool {
        self.target_mac.is_some() || self.target_linux.is_some()
    }

    /// Whether this mapping should be linked on the given OS (`mac`/`linux`).
    pub fn applies_to(&self, os: &str) -> bool {
        let os_target = match os {
            "mac" => self.target_mac.is_some(),
            "linux" => self.target_linux.is_some(),
            _ => false,
        };
        // Applies if there's an OS-specific target for this OS, or a general
        // target, or no OS-specific targets exist at all (derive from name).
        os_target || self.target.is_some() || !self.has_os_specific()
    }

    /// The comma-joined list of OSes this mapping applies to, for display.
    pub fn os_display(&self) -> String {
        ["mac", "linux"]
            .into_iter()
            .filter(|o| self.applies_to(o))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Absolute path inside the sync folder.
    pub fn source(&self, sync_dir: &Path) -> PathBuf {
        sync_dir.join(&self.name)
    }

    /// The resolved absolute `$HOME` target for the given OS, or `None` when the
    /// mapping does not apply on that OS.
    pub fn target_for(&self, home: &Path, os: &str) -> Option<PathBuf> {
        let raw = match os {
            "mac" => self.target_mac.as_deref().or(self.target.as_deref()),
            "linux" => self.target_linux.as_deref().or(self.target.as_deref()),
            _ => self.target.as_deref(),
        };
        match raw {
            Some(s) => Some(expand_tilde(s, home)),
            None if self.applies_to(os) => Some(home.join(&self.name)),
            None => None,
        }
    }

    /// The target as written (for display), for the given OS.
    pub fn target_display(&self, os: &str) -> String {
        match os {
            "mac" => self.target_mac.as_deref().or(self.target.as_deref()),
            "linux" => self.target_linux.as_deref().or(self.target.as_deref()),
            _ => self.target.as_deref(),
        }
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("~/{}", self.name))
    }

    /// Parse the octal `mode` into a numeric mode, if set.
    pub fn mode_bits(&self) -> Option<u32> {
        self.mode
            .as_deref()
            .and_then(|m| u32::from_str_radix(m.trim_start_matches("0o"), 8).ok())
    }
}

/// The `dotsync.toml` document: an array-of-tables under `[[mapping]]`.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct MappingsFile {
    #[serde(default, rename = "mapping")]
    pub mappings: Vec<Mapping>,
}

const HEADER: &str = "\
# dotsync mappings — this file lives inside your cloud sync folder and syncs
# across machines. It is managed by `dotsync adopt`; you can also edit it by hand.
#
# Each [[mapping]] mirrors a path from your home base into this folder.
#   name          (required) path relative to the home base, e.g. \".config/zed\".
#   target        (optional) explicit $HOME path; defaults to ~/<name>.
#   target_mac    (optional) macOS-only path override.
#   target_linux  (optional) Linux-only path override.
#   mode          (optional) octal mode enforced on the sync copy, e.g. \"0600\".
#   on_conflict   (optional) \"fail\" (default) or \"adopt\" (sync wins, local .bak).

";

impl MappingsFile {
    /// The conventional filename inside the sync folder.
    pub const FILE_NAME: &'static str = "dotsync.toml";

    /// Parse a `dotsync.toml` from disk. A missing file yields an empty set.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(MappingsFile::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("invalid TOML in {}", path.display()))
    }

    /// Serialize back to disk with a documenting header, sorted by name.
    pub fn save(&self, path: &Path) -> Result<()> {
        let mut sorted = MappingsFile {
            mappings: self.mappings.clone(),
        };
        sorted.mappings.sort_by_key(|m| m.name.clone()); // stable order, by mirror path
        let body = toml::to_string_pretty(&sorted).context("serializing mappings")?;
        std::fs::write(path, format!("{HEADER}{body}"))
            .with_context(|| format!("could not write {}", path.display()))
    }

    pub fn find(&self, name: &str) -> Option<&Mapping> {
        self.mappings.iter().find(|m| m.name == name)
    }

    /// Insert or replace a mapping by name.
    pub fn upsert(&mut self, mapping: Mapping) {
        if let Some(slot) = self.mappings.iter_mut().find(|m| m.name == mapping.name) {
            *slot = mapping;
        } else {
            self.mappings.push(mapping);
        }
    }
}

/// The OS string dotsync uses for the current platform.
pub fn current_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "mac"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "other"
    }
}

/// Expand a leading `~` / `~/` against `home`.
pub fn expand_tilde(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        home.to_path_buf()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}
