//! dotsync-core — the library behind the `dotsync` CLI.
//!
//! dotsync wires paths in your `$HOME` into a cloud-synced folder (Nextcloud,
//! OneDrive, Dropbox, …) via symlinks, so the cloud provider — not git — keeps
//! your config in sync across machines automatically. Every capability lives
//! here as a reusable function; the CLI is a thin `--json`-emitting layer plus
//! an interactive picker on top.
//!
//! Design rules that hold across the crate:
//! - Never destroy data without a backup (`.bak`).
//! - Prefer idempotent, self-healing operations: an atomic-save that clobbers a
//!   symlink is silently relinked when the content still matches the sync copy.
//! - Mirror the `$HOME` tree inside the sync folder, so a mapping's cloud path
//!   equals its home-relative path — no collisions, "original location" is
//!   self-evident.
//! - Keep secrets out of git by construction (the sync folder is never a repo)
//!   and enforce restrictive file modes on known-secret paths.

pub mod apply;
pub mod config;
pub mod discovery;
pub mod doctor;
pub mod fsutil;
pub mod mapping;
pub mod overview;
pub mod plan;
pub mod ui;
