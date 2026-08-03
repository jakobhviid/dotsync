//! The mapping model and the `dotsync.toml` document that lives *inside* the
//! sync folder (so the mapping list itself syncs across machines).
//!
//! Layout convention: the sync folder mirrors `$HOME`. A mapping's `name` is its
//! path relative to the configured home base (e.g. `.config/zed`), which is also
//! where it lives inside the sync folder. For the common case that's all a
//! mapping needs — the `$HOME` target is derived from `name`. Per-OS overrides
//! and an explicit `target` cover the cases where the path differs.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
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
    /// Optional group label, so several mappings can be managed as a unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
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
            group: None,
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

/// The `dotsync.toml` document: an array-of-tables under `[[mapping]]`, preceded
/// by a `dotsync_version` stamp.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct MappingsFile {
    /// The dotsync version that last wrote this file. Stamped on every save and
    /// used to detect cross-machine skew — a file written by a *newer* dotsync
    /// than the one now reading it. Absent in files written before the stamp
    /// existed (a lenient read: its absence is never an error). Declared first so
    /// it serialises as a top-level scalar, ahead of the `[[mapping]]` tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dotsync_version: Option<String>,
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
#
# The `dotsync_version` line at the top is stamped automatically on save; it lets
# another machine warn when this file was written by a newer dotsync than its own.

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
        let mut file: MappingsFile =
            toml::from_str(&text).with_context(|| format!("invalid TOML in {}", path.display()))?;
        // Guard against a hand-merged conflicted copy with duplicate names: keep
        // the first of each so nothing is planned or acted on twice.
        let mut seen = std::collections::HashSet::new();
        file.mappings.retain(|m| seen.insert(m.name.clone()));
        Ok(file)
    }

    /// Serialize back to disk with a documenting header, sorted by name, stamping
    /// the writing tool's version so another machine can detect skew.
    pub fn save(&self, path: &Path) -> Result<()> {
        let mut mappings = self.mappings.clone();
        mappings.sort_by_key(|m| m.name.clone()); // stable order, by mirror path
        let stamped = MappingsFile {
            dotsync_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            mappings,
        };
        let body = toml::to_string_pretty(&stamped).context("serializing mappings")?;
        std::fs::write(path, format!("{HEADER}{body}"))
            .with_context(|| format!("could not write {}", path.display()))
    }

    /// The stamped writer version when this file was written by a **newer** dotsync
    /// than the one now running — a cross-machine skew worth surfacing. `None` when
    /// there is no stamp, it is unparseable, or the running build is as new or
    /// newer. Best-effort: this only decides whether to *warn*, never gates
    /// behaviour, so an unreadable stamp simply stays quiet.
    pub fn newer_writer(&self) -> Option<&str> {
        let written = self.dotsync_version.as_deref()?;
        version_gt(written, env!("CARGO_PKG_VERSION")).then_some(written)
    }

    pub fn find(&self, name: &str) -> Option<&Mapping> {
        self.mappings.iter().find(|m| m.name == name)
    }

    /// Distinct group labels, sorted.
    pub fn groups(&self) -> Vec<String> {
        let mut g: Vec<String> = self.mappings.iter().filter_map(|m| m.group.clone()).collect();
        g.sort();
        g.dedup();
        g
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

/// Validate a group name. Group names live in a namespace disjoint from mapping
/// paths (no `/`, no leading `.`) so a selector like `dotsync install claude` is
/// never ambiguous between a group and a path.
pub fn validate_group_name(name: &str) -> Result<()> {
    let n = name.trim();
    if n.is_empty() {
        bail!("group name cannot be empty");
    }
    if n.contains('/') {
        bail!("group name cannot contain '/' (that's for paths)");
    }
    if n.starts_with('.') {
        bail!("group name cannot start with '.' (that's for paths)");
    }
    Ok(())
}

/// A group name and a mapping name share the `install <name>` selector space, so
/// they must never collide (the `/`-and-leading-`.` rule alone doesn't stop a
/// group from shadowing a bare-named mapping like `Brewfile`). Error if `name`
/// is already used by a mapping.
pub fn ensure_free_of_mapping(name: &str, mappings: &MappingsFile) -> Result<()> {
    if mappings.find(name.trim()).is_some() {
        bail!(
            "{:?} is already a mapping name — a name is either a group or a mapping, not both",
            name.trim()
        );
    }
    Ok(())
}

/// Error if `name` (a would-be mapping name) is already used by a group.
pub fn ensure_free_of_group(name: &str, mappings: &MappingsFile) -> Result<()> {
    if mappings.groups().iter().any(|g| g == name) {
        bail!("{name:?} is already a group name — a name is either a group or a mapping, not both");
    }
    Ok(())
}

/// The deepest shared parent directory of several home-relative paths, if any.
pub fn common_parent(paths: &[String]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    let parents: Vec<Vec<&str>> = paths
        .iter()
        .map(|p| {
            let mut c: Vec<&str> = p.split('/').collect();
            c.pop(); // drop the leaf, keep the directory part
            c
        })
        .collect();
    let mut common = parents[0].clone();
    for p in &parents[1..] {
        let mut i = 0;
        while i < common.len() && i < p.len() && common[i] == p[i] {
            i += 1;
        }
        common.truncate(i);
    }
    if common.is_empty() {
        None
    } else {
        Some(common.join("/"))
    }
}

/// Suggest a group name from the paths being adopted: the shared parent dir when
/// several paths cluster, else the single path's leaf — with leading dots and
/// the extension stripped, lowercased. E.g. [".claude/CLAUDE.md",
/// ".claude/settings.json"] -> "claude"; ".config/zed" -> "zed".
pub fn suggest_group_name(paths: &[String]) -> String {
    let pick = if paths.len() > 1 {
        common_parent(paths).unwrap_or_else(|| paths[0].clone())
    } else {
        paths.first().cloned().unwrap_or_default()
    };
    let base = pick.rsplit('/').next().unwrap_or(&pick);
    let base = base.trim_start_matches('.');
    let base = base.rsplit_once('.').map(|(a, _)| a).unwrap_or(base);
    base.to_lowercase()
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

/// Inverse of [`expand_tilde`]: render `path` as `~/…` when it lives under
/// `home`, so stored/displayed paths stay short and portable.
pub fn collapse_tilde(path: &Path, home: &Path) -> String {
    if path == home {
        "~".to_string()
    } else if let Ok(rel) = path.strip_prefix(home) {
        format!("~/{}", rel.to_string_lossy())
    } else {
        path.to_string_lossy().into_owned()
    }
}

/// Whether dotted-numeric version `a` is strictly greater than `b`, comparing the
/// first three components numerically (so `1.10.0 > 1.9.0`, unlike a string sort).
/// Any unparseable component counts as `0`; this only informs the skew *warning*,
/// so a version it can't read simply never triggers one.
fn version_gt(a: &str, b: &str) -> bool {
    fn parts(version: &str) -> (u64, u64, u64) {
        let mut components = version.split(['.', '-', '+']).map(|part| part.parse::<u64>().unwrap_or(0));
        (
            components.next().unwrap_or(0),
            components.next().unwrap_or(0),
            components.next().unwrap_or(0),
        )
    }
    parts(a) > parts(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_gt_compares_numerically_not_lexically() {
        assert!(version_gt("2.0.0", "1.9.9"));
        assert!(version_gt("1.10.0", "1.9.0")); // 10 > 9, not "10" < "9"
        assert!(!version_gt("1.0.0", "1.0.0"));
        assert!(!version_gt("1.0.0", "2.0.0"));
        assert!(!version_gt("garbage", "1.0.0")); // unparseable → 0.0.0, never warns
        assert!(version_gt("2.1.0-rc1", "2.0.0")); // pre-release suffix ignored
    }

    #[test]
    fn newer_writer_flags_only_a_strictly_newer_stamp() {
        let running = env!("CARGO_PKG_VERSION");
        let mut file = MappingsFile::default();
        assert_eq!(file.newer_writer(), None); // no stamp at all
        file.dotsync_version = Some(running.to_string());
        assert_eq!(file.newer_writer(), None); // same version as the running build
        file.dotsync_version = Some("999.0.0".to_string());
        assert_eq!(file.newer_writer(), Some("999.0.0"));
    }
}
