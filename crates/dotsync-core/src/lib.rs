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

pub mod apply;      // perform link/unlink/adopt/restore ops → typed `Outcome` records
pub mod config;     // per-machine config (sync dir + home base): load / save / git guard
pub mod discovery;  // find candidate cloud dotsync folders across known providers
pub mod doctor;     // health checks and safe `--fix` repairs → a classified `Report`
pub mod fsutil;     // filesystem primitives: symlink/tree checks, modes, secret detection
pub mod mapping;    // the shared `dotsync.toml`: mappings, groups, load / save
pub mod overview;   // render the status table (a query result → stdout)
pub mod plan;       // classify each mapping's `State` on this machine
pub mod ui;         // stdout/stderr discipline + colour palette
