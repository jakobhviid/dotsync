//! Auto-discovery of the cloud sync folder. Probes the common cloud-provider
//! roots for each platform, then offers two kinds of candidate:
//!
//! - **existing** `dotsync` folders (at a provider root or one level down, so
//!   `Apps/dotsync/` is found) — pick to reuse;
//! - a **proposed** `<provider>/Apps/dotsync` folder to create — following the
//!   common `<Cloud>/Apps/<app>` convention (Dropbox's App-folder pattern), so
//!   we never litter the provider root.
//!
//! Purely advisory — `setup` uses the results to build the picker; the user
//! always confirms, and can always type a path instead.

use std::path::{Path, PathBuf};

/// A discovered candidate sync folder.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// The cloud provider label (e.g. "Nextcloud").
    pub provider: String,
    /// The `dotsync` folder path (existing, or proposed for creation).
    pub path: PathBuf,
    /// Whether the folder already exists.
    pub exists: bool,
    /// Whether it already contains a `dotsync.toml` (i.e. is set up).
    pub configured: bool,
}

/// The folder name dotsync looks for / proposes inside each provider root.
const MARKER: &str = "dotsync";

/// Candidate provider roots under `$HOME`, as (label, path) pairs. Includes
/// macOS's unified `~/Library/CloudStorage/*` clients and the common
/// Linux/legacy home-root locations.
fn provider_roots(home: &Path) -> Vec<(String, PathBuf)> {
    let mut roots: Vec<(String, PathBuf)> = Vec::new();

    let cloud_storage = home.join("Library/CloudStorage");
    if let Ok(entries) = std::fs::read_dir(&cloud_storage) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let label = name.split(['-', '_']).next().unwrap_or(&name).to_string();
            roots.push((label, entry.path()));
        }
    }

    roots.push((
        "iCloud".into(),
        home.join("Library/Mobile Documents/com~apple~CloudDocs"),
    ));

    for (label, rel) in [
        ("Nextcloud", "Nextcloud"),
        ("Dropbox", "Dropbox"),
        ("OneDrive", "OneDrive"),
        ("ProtonDrive", "ProtonDrive"),
        ("ProtonDrive", "Proton Drive"),
        ("GoogleDrive", "Google Drive"),
        ("Sync", "Sync"),
    ] {
        roots.push((label.into(), home.join(rel)));
    }
    roots
}

fn push_unique(out: &mut Vec<Candidate>, seen: &mut Vec<PathBuf>, cand: Candidate) {
    let key = cand.path.canonicalize().unwrap_or_else(|_| cand.path.clone());
    if seen.contains(&key) {
        return;
    }
    seen.push(key);
    out.push(cand);
}

/// Discover existing and proposed cloud folders across all provider roots.
/// Sorted: configured first, then other existing folders, then proposals.
pub fn discover(home: &Path) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();

    for (provider, root) in provider_roots(home) {
        if !root.is_dir() {
            continue;
        }

        // Existing dotsync folders: at the root, and one level down.
        let direct = root.join(MARKER);
        let mut existing: Vec<PathBuf> = Vec::new();
        if direct.is_dir() {
            existing.push(direct.clone());
        }
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                if entry.file_type().map(|file_type| file_type.is_dir()).unwrap_or(false) {
                    let nested = entry.path().join(MARKER);
                    if nested.is_dir() {
                        existing.push(nested);
                    }
                }
            }
        }
        for path in existing {
            let configured = path.join(super::mapping::MappingsFile::FILE_NAME).exists();
            push_unique(
                &mut out,
                &mut seen,
                Candidate {
                    provider: provider.clone(),
                    path,
                    exists: true,
                    configured,
                },
            );
        }

        // Propose `<root>/Apps/dotsync` (the <Cloud>/Apps/<app> convention),
        // unless it already exists (in which case the scan above found it).
        let proposed = root.join("Apps").join(MARKER);
        if !proposed.is_dir() {
            push_unique(
                &mut out,
                &mut seen,
                Candidate {
                    provider: provider.clone(),
                    path: proposed,
                    exists: false,
                    configured: false,
                },
            );
        }
    }

    out.sort_by_key(|candidate| (!candidate.configured, !candidate.exists));
    out
}
