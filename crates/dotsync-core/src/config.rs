//! Per-machine configuration: where this machine's sync folder is, and the home
//! base that mapping targets are resolved against. Stored at
//! `$XDG_CONFIG_HOME/dotsync/config.toml` (falling back to `~/.config`).
//!
//! This is the *only* per-machine state. Which mappings are active on a machine
//! is recorded by the symlinks themselves, not here — the filesystem is the
//! source of truth.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::mapping::{collapse_tilde, expand_tilde};

/// Resolved per-machine configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// The dotsync folder inside the cloud provider's synced directory.
    pub sync_dir: PathBuf,
    /// The base directory mapping targets are relative to (usually `$HOME`).
    pub home: PathBuf,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct ConfigFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    sync_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    home: Option<String>,
}

/// The user's home directory from `$HOME`.
pub fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| anyhow!("$HOME is not set"))
}

/// `$XDG_CONFIG_HOME` or `~/.config`.
pub fn config_root() -> Result<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return Ok(PathBuf::from(x));
        }
    }
    Ok(home_dir()?.join(".config"))
}

/// Path to `config.toml`.
pub fn config_path() -> Result<PathBuf> {
    Ok(config_root()?.join("dotsync").join("config.toml"))
}

/// Load config, or `None` if this machine has never run `dotsync init`.
pub fn load() -> Result<Option<Config>> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let file: ConfigFile =
        toml::from_str(&text).with_context(|| format!("invalid TOML in {}", path.display()))?;
    let Some(sync_dir) = file.sync_dir else {
        return Ok(None);
    };
    // `home` may be stored as `~`; resolve it first, then the sync dir against it.
    let home = match file.home {
        Some(h) => expand_tilde(&h, &home_dir()?),
        None => home_dir()?,
    };
    Ok(Some(Config {
        sync_dir: expand_tilde(&sync_dir, &home),
        home,
    }))
}

/// Load config or fail with an actionable message.
pub fn require() -> Result<Config> {
    load()?.ok_or_else(|| {
        anyhow!("dotsync is not configured on this machine — run `dotsync init` first")
    })
}

/// Persist config to disk, creating the config directory as needed.
pub fn save(cfg: &Config) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    // Store paths collapsed to `~/…` where possible, so config.toml stays short.
    let home_base = home_dir().unwrap_or_else(|_| cfg.home.clone());
    let file = ConfigFile {
        sync_dir: Some(collapse_tilde(&cfg.sync_dir, &cfg.home)),
        home: Some(collapse_tilde(&cfg.home, &home_base)),
    };
    let body = toml::to_string_pretty(&file).context("serializing config")?;
    std::fs::write(&path, body).with_context(|| format!("could not write {}", path.display()))?;
    Ok(())
}

/// The nearest ancestor of `path` that is a git working tree (contains `.git`),
/// if any. Used to keep synced content — especially secrets — out of git.
pub fn enclosing_git_tree(path: &Path) -> Option<PathBuf> {
    let mut dir = Some(path);
    while let Some(d) = dir {
        if d.join(".git").exists() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// Guardrail: refuse a sync dir that sits inside a git working tree, so synced
/// content (including secrets) can never be committed to a repo by accident.
pub fn ensure_not_in_git(sync_dir: &Path) -> Result<()> {
    if let Some(repo) = enclosing_git_tree(sync_dir) {
        return Err(anyhow!(
            "sync dir {} is inside a git repository ({}) — dotsync refuses this so \
             synced files (and secrets) can never be committed. Point it at a plain \
             cloud folder instead.",
            sync_dir.display(),
            repo.display()
        ));
    }
    Ok(())
}
