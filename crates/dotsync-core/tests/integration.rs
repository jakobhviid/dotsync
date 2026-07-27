//! End-to-end tests over a real temp filesystem: adopt, state detection, the
//! atomic-save heal path, and conflict handling.

use std::fs;
use std::path::PathBuf;

use dotsync_core::apply;
use dotsync_core::config::Config;
use dotsync_core::fsutil;
use dotsync_core::mapping::{Mapping, MappingsFile};
use dotsync_core::plan::{state_of, State};

fn sandbox() -> (tempfile::TempDir, Config) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let sync = dir.path().join("cloud/dotsync");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&sync).unwrap();
    let cfg = Config {
        sync_dir: sync,
        home,
    };
    (dir, cfg)
}

fn write(path: &PathBuf, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn adopt_moves_into_cloud_and_links_back() {
    let (_d, cfg) = sandbox();
    let target = cfg.home.join(".config/zed/settings.json");
    write(&target, "theme=dark");
    // Adopt the containing dir.
    let dir = cfg.home.join(".config/zed");

    let (mapping, out) = apply::adopt(&cfg, &dir, None, false).unwrap();
    assert!(out.ok);
    assert_eq!(mapping.name, ".config/zed");

    // The dir is now a symlink into the cloud, and the file survived the move.
    assert!(fsutil::is_symlink(&dir));
    let source = cfg.sync_dir.join(".config/zed");
    assert_eq!(fsutil::read_link(&dir).unwrap(), source);
    assert_eq!(
        fs::read_to_string(source.join("settings.json")).unwrap(),
        "theme=dark"
    );

    let item = state_of(&mapping, &cfg, "mac");
    assert_eq!(item.state, State::Linked);
}

#[test]
fn secret_paths_are_auto_moded() {
    let (_d, cfg) = sandbox();
    let target = cfg.home.join(".ssh/config");
    write(&target, "Host x");
    let (mapping, _) = apply::adopt(&cfg, &target, None, false).unwrap();
    assert_eq!(mapping.mode.as_deref(), Some("0600"));
    let mode = fsutil::mode_of(&cfg.sync_dir.join(".ssh/config")).unwrap();
    assert_eq!(mode, 0o600);
}

#[test]
fn atomic_save_clobber_is_healable_and_heals() {
    let (_d, cfg) = sandbox();
    let target = cfg.home.join(".gitconfig");
    write(&target, "[user]\n");
    let (mapping, _) = apply::adopt(&cfg, &target, None, false).unwrap();

    // Simulate an atomic save: replace the symlink with a real, identical file.
    let source = mapping.source(&cfg.sync_dir);
    fsutil::remove_symlink(&target).unwrap();
    fs::copy(&source, &target).unwrap();

    let item = state_of(&mapping, &cfg, "mac");
    assert_eq!(item.state, State::Healable);

    let out = apply::link_item(&item, false);
    assert!(out.ok);
    assert!(fsutil::is_symlink(&target));
}

#[test]
fn real_divergence_is_a_conflict_and_fail_policy_refuses() {
    let (_d, cfg) = sandbox();
    let target = cfg.home.join(".gitconfig");
    write(&target, "original");
    let (mapping, _) = apply::adopt(&cfg, &target, None, false).unwrap();

    // Diverge: local file now differs from the cloud copy.
    fsutil::remove_symlink(&target).unwrap();
    fs::write(&target, "changed locally").unwrap();

    let item = state_of(&mapping, &cfg, "mac");
    assert_eq!(item.state, State::Diverged);

    // Default fail policy must not clobber.
    let out = apply::link_item(&item, false);
    assert!(!out.ok);
    assert_eq!(fs::read_to_string(&target).unwrap(), "changed locally");
}

#[test]
fn per_os_targets_scope_correctly() {
    let mut m = Mapping::new(".config/linearmouse");
    m.target_mac = Some("~/.config/linearmouse".into());
    assert!(m.applies_to("mac"));
    assert!(!m.applies_to("linux"));

    let mut both = Mapping::new(".config/code");
    both.target_mac = Some("~/Library/Application Support/Code".into());
    both.target_linux = Some("~/.config/Code".into());
    let home = PathBuf::from("/home/j");
    assert_eq!(
        both.target_for(&home, "linux").unwrap(),
        PathBuf::from("/home/j/.config/Code")
    );

    let plain = Mapping::new(".config/zed");
    assert!(plain.applies_to("mac") && plain.applies_to("linux"));
}

#[test]
fn mappings_file_round_trips() {
    let (_d, cfg) = sandbox();
    let path = cfg.sync_dir.join(MappingsFile::FILE_NAME);
    let mut f = MappingsFile::default();
    f.upsert(Mapping::new(".config/zed"));
    let mut secret = Mapping::new(".ssh/config");
    secret.mode = Some("0600".into());
    f.upsert(secret);
    f.save(&path).unwrap();

    let loaded = MappingsFile::load(&path).unwrap();
    assert_eq!(loaded.mappings.len(), 2);
    assert_eq!(loaded.find(".ssh/config").unwrap().mode.as_deref(), Some("0600"));
}
