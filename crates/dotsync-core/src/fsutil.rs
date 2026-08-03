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
        .map(|meta| meta.file_type().is_symlink())
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
/// The cross-device path copies to a temp sibling and renames into place, so a
/// crash never leaves a half-written file at the final `dst`.
pub fn move_path(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    let tmp = temp_sibling(dst);
    let _ = remove_path(&tmp); // clear any stale temp
    copy_recursive(src, &tmp)?;
    fs::rename(&tmp, dst)
        .with_context(|| format!("could not finalize move to {}", dst.display()))?;
    remove_path(src)?;
    Ok(())
}

/// A temp path next to `path` (same directory, so a rename into place is atomic).
pub fn temp_sibling(path: &Path) -> PathBuf {
    let mut os_string = path.as_os_str().to_os_string();
    os_string.push(".dotsync-tmp");
    PathBuf::from(os_string)
}

/// Recursively copy a file, directory, or symlink, preserving symlinks and the
/// source's permission bits (so a secret dir isn't silently widened to 0755 on
/// a cross-device copy).
pub fn copy_recursive(src: &Path, dst: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(src)
        .with_context(|| format!("could not stat {}", src.display()))?;
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        let target = fs::read_link(src)?;
        symlink(&target, dst)
            .with_context(|| format!("could not copy symlink to {}", dst.display()))?;
    } else if file_type.is_dir() {
        fs::create_dir_all(dst)
            .with_context(|| format!("could not create {}", dst.display()))?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        }
        // Mirror the source mode *after* writing children, so a read-only source
        // dir (e.g. 0500) doesn't make its own children un-writable mid-copy.
        fs::set_permissions(dst, meta.permissions()).ok();
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
        let mut os_string = path.as_os_str().to_os_string();
        os_string.push(".bak");
        PathBuf::from(os_string)
    };
    if !path_present(&base) {
        return base;
    }
    for counter in 1.. {
        let mut os_string = path.as_os_str().to_os_string();
        os_string.push(format!(".bak.{counter}"));
        let candidate = PathBuf::from(os_string);
        if !path_present(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// Recursively compare two paths for equal content and structure: files by
/// bytes, directories by their (name-matched) entries, symlinks by link target.
/// Returns `false` if either is missing or their types differ.
pub fn trees_equal(left: &Path, right: &Path) -> bool {
    let (left_meta, right_meta) = match (fs::symlink_metadata(left), fs::symlink_metadata(right)) {
        (Ok(left_meta), Ok(right_meta)) => (left_meta, right_meta),
        _ => return false,
    };
    let (left_type, right_type) = (left_meta.file_type(), right_meta.file_type());
    if left_type.is_symlink() || right_type.is_symlink() {
        left_type.is_symlink() && right_type.is_symlink() && fs::read_link(left).ok() == fs::read_link(right).ok()
    } else if left_type.is_dir() && right_type.is_dir() {
        let entries = |dir: &Path| -> Option<Vec<std::ffi::OsString>> {
            let mut names: Vec<_> = fs::read_dir(dir).ok()?.flatten().map(|entry| entry.file_name()).collect();
            names.sort();
            Some(names)
        };
        match (entries(left), entries(right)) {
            (Some(left_names), Some(right_names)) if left_names == right_names => {
                left_names.iter().all(|name| trees_equal(&left.join(name), &right.join(name)))
            }
            _ => false,
        }
    } else if left_type.is_file() && right_type.is_file() {
        files_equal(left, right)
    } else {
        false
    }
}

/// Whether two paths refer to the same real location once symlinks and path
/// normalization are resolved. Returns `false` if either can't be resolved
/// (e.g. a missing target), so it never masks a dangling link.
pub fn same_location(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left_real), Ok(right_real)) => left_real == right_real,
        _ => false,
    }
}

/// Byte-compare two regular files. Returns `false` if either isn't a readable
/// regular file, or if their contents differ.
pub fn files_equal(left: &Path, right: &Path) -> bool {
    match (fs::read(left), fs::read(right)) {
        (Ok(left_bytes), Ok(right_bytes)) => left_bytes == right_bytes,
        _ => false,
    }
}

/// The current mode bits (low 12 bits) of a path, following symlinks.
pub fn mode_of(path: &Path) -> Option<u32> {
    fs::metadata(path).ok().map(|meta| meta.permissions().mode() & 0o7777)
}

/// Set the mode on a path (following symlinks — so calling this on a symlink
/// adjusts the pointed-to file, which is what we want for a synced source).
pub fn set_mode(path: &Path, mode: u32) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("could not chmod {} to {:o}", path.display(), mode))
}

/// Home-relative path prefixes (whole segments) that hold secrets.
const SECRET_DIRS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".kube",
    ".docker",
    ".password-store",
    ".config/gh",
    ".config/gcloud",
    ".config/op",
    ".config/rclone",
];

/// Heuristic: does this home-relative path look like it holds secrets? Used to
/// auto-tag a restrictive mode at adopt time. Matches whole path segments (so
/// adopting the directory `~/.aws` itself is caught, not just `~/.aws/...`).
/// Best-effort only — the user can always set `mode`/`secret` explicitly.
pub fn looks_secret(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let base = lower.rsplit('/').next().unwrap_or(&lower);

    let dir_secret = SECRET_DIRS
        .iter()
        .any(|secret_dir| lower == *secret_dir || lower.starts_with(&format!("{secret_dir}/")));

    let file_secret = base.ends_with(".pem")
        || base.ends_with(".key")
        || base.starts_with(".env")
        || base.contains("id_rsa")
        || base.contains("id_ed25519")
        || matches!(
            base,
            ".netrc" | ".npmrc" | ".pgpass" | ".authinfo" | ".git-credentials"
                | ".terraformrc" | ".msmtprc" | "credentials"
        )
        || lower.contains("credential")
        || lower.contains("secret");

    dir_secret || file_secret
}

/// Recursively enforce secret permissions on the sync copy: `0700` on
/// directories, `0600` on files. Symlinks inside the tree are left untouched
/// (we never chmod through a link to something outside the sync set).
pub fn enforce_secret_tree(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)
        .with_context(|| format!("could not stat {}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        set_mode(path, 0o700)?;
        for entry in fs::read_dir(path)? {
            enforce_secret_tree(&entry?.path())?;
        }
    } else {
        set_mode(path, 0o600)?;
    }
    Ok(())
}

/// True if any (non-symlink) file or directory in the tree is group/world
/// accessible — i.e. a secret whose mode has drifted open.
pub fn any_too_open(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => false,
        Ok(meta) => {
            if mode_of(path).map(|mode| mode & 0o077 != 0).unwrap_or(false) {
                return true;
            }
            if meta.is_dir() {
                if let Ok(entries) = fs::read_dir(path) {
                    return entries.flatten().any(|entry| any_too_open(&entry.path()));
                }
            }
            false
        }
        Err(_) => false,
    }
}
