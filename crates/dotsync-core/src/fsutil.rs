//! Filesystem primitives shared across planning and apply: symlinks, robust
//! (cross-device) moves, recursive copy, content comparison, and mode
//! enforcement. Unix-only — dotsync targets macOS and Linux.

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// True if `path` is a symlink (whether or not its destination exists).
pub fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// True if anything (file, dir, or even a dangling symlink) exists at `path`.
pub fn path_present(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Read a symlink's destination.
pub fn read_link(path: &Path) -> Result<PathBuf> {
    fs::read_link(path).with_context(|| format!("could not read symlink {}", path.display()))
}

/// Create a symlink at `link` pointing to `original`, making parent dirs.
pub fn make_symlink(original: &Path, link: &Path) -> Result<()> {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    symlink(original, link)
        .with_context(|| format!("could not link {} -> {}", link.display(), original.display()))
}

/// Remove a symlink (not its target).
pub fn remove_symlink(link: &Path) -> Result<()> {
    fs::remove_file(link).with_context(|| format!("could not remove symlink {}", link.display()))
}

/// Remove a file or directory tree.
pub fn remove_path(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)
        .with_context(|| format!("could not stat {}", path.display()))?;
    if meta.file_type().is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
    .with_context(|| format!("could not remove {}", path.display()))
}

/// Move `src` to `dst`, falling back to copy+remove across filesystems (the
/// common case here: `$HOME` and the cloud folder are often different volumes).
pub fn move_path(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_recursive(src, dst)?;
    remove_path(src)?;
    Ok(())
}

/// Recursively copy a file, directory, or symlink, preserving symlinks.
pub fn copy_recursive(src: &Path, dst: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(src)
        .with_context(|| format!("could not stat {}", src.display()))?;
    let ft = meta.file_type();
    if ft.is_symlink() {
        let target = fs::read_link(src)?;
        symlink(&target, dst)
            .with_context(|| format!("could not copy symlink to {}", dst.display()))?;
    } else if ft.is_dir() {
        fs::create_dir_all(dst)
            .with_context(|| format!("could not create {}", dst.display()))?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        }
    } else {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src, dst)
            .with_context(|| format!("could not copy {} to {}", src.display(), dst.display()))?;
    }
    Ok(())
}

/// Choose a non-clobbering backup path: `<path>.bak`, `<path>.bak.1`, …
pub fn backup_path(path: &Path) -> PathBuf {
    let base = {
        let mut s = path.as_os_str().to_os_string();
        s.push(".bak");
        PathBuf::from(s)
    };
    if !path_present(&base) {
        return base;
    }
    for n in 1.. {
        let mut s = path.as_os_str().to_os_string();
        s.push(format!(".bak.{n}"));
        let candidate = PathBuf::from(s);
        if !path_present(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// Byte-compare two regular files. Returns `false` if either isn't a readable
/// regular file, or if their contents differ.
pub fn files_equal(a: &Path, b: &Path) -> bool {
    match (fs::read(a), fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// The current mode bits (low 12 bits) of a path, following symlinks.
pub fn mode_of(path: &Path) -> Option<u32> {
    fs::metadata(path).ok().map(|m| m.permissions().mode() & 0o7777)
}

/// Set the mode on a path (following symlinks — so calling this on a symlink
/// adjusts the pointed-to file, which is what we want for a synced source).
pub fn set_mode(path: &Path, mode: u32) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("could not chmod {} to {:o}", path.display(), mode))
}

/// Heuristic: does this home-relative path look like it holds secrets? Used to
/// auto-tag a restrictive mode at adopt time.
pub fn looks_secret(name: &str) -> Option<u32> {
    let n = name.to_ascii_lowercase();
    let dir_secret = n.starts_with(".ssh/")
        || n == ".ssh"
        || n.starts_with(".gnupg")
        || n.starts_with(".aws/")
        || n.starts_with(".config/gh/")
        || n.starts_with(".config/gcloud/")
        || n.contains("credential")
        || n.contains("secret");
    let file_secret = n.ends_with(".pem")
        || n.ends_with(".key")
        || n.ends_with("id_rsa")
        || n.ends_with("id_ed25519")
        || n.ends_with(".npmrc")
        || n.ends_with(".netrc");
    if dir_secret || file_secret {
        // Directories need execute/search (0700); lone files 0600. Caller may
        // refine, but default by whether the name ends in a slash-ish dir hint.
        Some(0o600)
    } else {
        None
    }
}
