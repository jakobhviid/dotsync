//! Auto-discovery of the cloud sync folder. Probes the common cloud-provider
//! roots for each platform and looks for a `dotsync` folder at the root or one
//! level down (so `Apps/dotsync/` is found). Purely advisory — `init` uses the
//! results to propose a sync dir; the user always confirms.

use std::path::{Path, PathBuf};

/// A discovered candidate sync folder.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// The cloud provider label (e.g. "Nextcloud").
    pub provider: String,
    /// The `dotsync` folder path.
    pub path: PathBuf,
    /// Whether it already contains a `dotsync.toml` (i.e. is set up).
    pub configured: bool,
}

/// The folder name dotsync looks for inside each provider root.
const MARKER: &str = "dotsync";

/// Candidate provider roots under `$HOME`, as (label, relative path) pairs.
/// Includes macOS's unified `~/Library/CloudStorage/*` clients and the common
/// Linux/legacy home-root locations.
fn provider_roots(home: &Path) -> Vec<(String, PathBuf)> {
    let mut roots: Vec<(String, PathBuf)> = Vec::new();

    // macOS File Provider clients live under ~/Library/CloudStorage/<Name-...>
    let cloud_storage = home.join("Library/CloudStorage");
    if let Ok(entries) = std::fs::read_dir(&cloud_storage) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let label = name.split(['-', '_']).next().unwrap_or(&name).to_string();
            roots.push((label, e.path()));
        }
    }

    // iCloud Drive.
    roots.push((
        "iCloud".into(),
        home.join("Library/Mobile Documents/com~apple~CloudDocs"),
    ));

    // Home-root locations (Linux clients, and older macOS clients).
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

/// Search all provider roots for a `dotsync` marker folder. De-duplicated by
/// resolved path; configured folders (with a `dotsync.toml`) sort first.
pub fn discover(home: &Path) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();

    for (provider, root) in provider_roots(home) {
        if !root.is_dir() {
            continue;
        }
        // The marker at the provider root, and one level down (e.g. Apps/dotsync).
        let mut candidates = vec![root.join(MARKER)];
        if let Ok(entries) = std::fs::read_dir(&root) {
            for e in entries.flatten() {
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    candidates.push(e.path().join(MARKER));
                }
            }
        }
        for path in candidates {
            if !path.is_dir() {
                continue;
            }
            let key = path.canonicalize().unwrap_or_else(|_| path.clone());
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            let configured = path.join(super::mapping::MappingsFile::FILE_NAME).exists();
            found.push(Candidate {
                provider: provider.clone(),
                path,
                configured,
            });
        }
    }

    // Configured folders (with a dotsync.toml) first.
    found.sort_by_key(|c| std::cmp::Reverse(c.configured));
    found
}
