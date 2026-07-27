//! End-to-end tests over a real temp filesystem: adopt, state detection, the
//! atomic-save heal path, and conflict handling.

use std::fs;
use std::path::PathBuf;

use dotsync_core::apply;
use dotsync_core::config::Config;
use dotsync_core::doctor;
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

    let (mapping, out) = apply::adopt(&cfg, &dir, None, None, &[], false).unwrap();
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
    let (mapping, _) = apply::adopt(&cfg, &target, None, None, &[], false).unwrap();
    assert_eq!(mapping.mode.as_deref(), Some("0600"));
    let mode = fsutil::mode_of(&cfg.sync_dir.join(".ssh/config")).unwrap();
    assert_eq!(mode, 0o600);
}

#[test]
fn atomic_save_clobber_is_healable_and_heals() {
    let (_d, cfg) = sandbox();
    let target = cfg.home.join(".gitconfig");
    write(&target, "[user]\n");
    let (mapping, _) = apply::adopt(&cfg, &target, None, None, &[], false).unwrap();

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
    let (mapping, _) = apply::adopt(&cfg, &target, None, None, &[], false).unwrap();

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
fn adopt_refuses_path_already_inside_sync() {
    let (_d, cfg) = sandbox();
    // Adopt ~/.config as a dir → it becomes a symlink into the cloud.
    write(&cfg.home.join(".config/a.conf"), "a");
    apply::adopt(&cfg, &cfg.home.join(".config"), None, None, &[], false).unwrap();

    // A file now created "in" ~/.config actually lives in the cloud (via the
    // symlink). Adopting it must be refused, not delete the cloud master.
    let inner = cfg.home.join(".config/foo.conf");
    fs::write(&inner, "precious").unwrap();
    let res = apply::adopt(&cfg, &inner, None, None, &[], false);
    assert!(res.is_err(), "adopting a path inside the sync folder must be refused");
    assert_eq!(fs::read_to_string(&inner).unwrap(), "precious", "must not be destroyed");
}

#[test]
fn whole_secret_dir_is_tagged_and_moded_recursively() {
    let (_d, cfg) = sandbox();
    // The natural case: adopt the whole ~/.aws directory (no trailing slash).
    write(&cfg.home.join(".aws/credentials"), "key");
    let (m, _) = apply::adopt(&cfg, &cfg.home.join(".aws"), None, None, &[], false).unwrap();
    assert_eq!(m.mode.as_deref(), Some("0700"), "whole secret dir must be tagged");

    // Perms are enforced recursively: dir 0700, inner file 0600.
    assert_eq!(fsutil::mode_of(&cfg.sync_dir.join(".aws")).unwrap(), 0o700);
    assert_eq!(
        fsutil::mode_of(&cfg.sync_dir.join(".aws/credentials")).unwrap(),
        0o600
    );
}

#[test]
fn adopt_with_group_tags_and_lists() {
    let (_d, cfg) = sandbox();
    let mut file = MappingsFile::default();
    for name in [".claude/CLAUDE.md", ".claude/settings.json"] {
        let target = cfg.home.join(name);
        write(&target, "x");
        let (m, _) = apply::adopt(&cfg, &target, None, Some("claude".into()), &[], false).unwrap();
        assert_eq!(m.group.as_deref(), Some("claude"));
        file.upsert(m);
    }
    assert_eq!(file.groups(), vec!["claude".to_string()]);
}

#[test]
fn adopt_refuses_overlapping_mappings() {
    let (_d, cfg) = sandbox();
    write(&cfg.home.join(".config/zed/s"), "x");
    // Existing mapping ".config" → adopting ".config/zed" is already covered.
    let r = apply::adopt(
        &cfg,
        &cfg.home.join(".config/zed"),
        None,
        None,
        &[".config".to_string()],
        false,
    );
    assert!(r.is_err());
    // Existing ".config/zed" → adopting ".config" would contain it.
    let r2 = apply::adopt(
        &cfg,
        &cfg.home.join(".config"),
        None,
        None,
        &[".config/zed".to_string()],
        false,
    );
    assert!(r2.is_err());
}

#[test]
fn group_name_validation_and_suggestion() {
    use dotsync_core::mapping::{suggest_group_name, validate_group_name};
    assert!(validate_group_name("Claude Code").is_ok());
    assert!(validate_group_name(".hidden").is_err()); // path namespace
    assert!(validate_group_name("a/b").is_err());
    assert!(validate_group_name("").is_err());

    assert_eq!(suggest_group_name(&[".config/zed".into()]), "zed");
    assert_eq!(suggest_group_name(&[".gitconfig".into()]), "gitconfig");
    assert_eq!(
        suggest_group_name(&[".claude/CLAUDE.md".into(), ".claude/settings.json".into()]),
        "claude"
    );
}

#[test]
fn discovery_proposes_and_finds_folders() {
    use dotsync_core::discovery;

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    fs::create_dir_all(home.join("Nextcloud")).unwrap();

    // Provider root exists but no dotsync folder → propose <root>/Apps/dotsync.
    let cands = discovery::discover(home);
    let nc = cands
        .iter()
        .find(|c| c.path == home.join("Nextcloud/Apps/dotsync"))
        .expect("should propose ~/Nextcloud/Apps/dotsync");
    assert!(!nc.exists && !nc.configured);

    // An existing folder one level down (Apps/dotsync) is found and, with a
    // dotsync.toml, reported as configured.
    fs::create_dir_all(home.join("Nextcloud/Apps/dotsync")).unwrap();
    fs::write(home.join("Nextcloud/Apps/dotsync/dotsync.toml"), "").unwrap();
    let cands = discovery::discover(home);
    let nc = cands
        .iter()
        .find(|c| c.path == home.join("Nextcloud/Apps/dotsync"))
        .unwrap();
    assert!(nc.exists && nc.configured);
}

#[test]
fn restore_item_swaps_symlink_for_real_copy_and_keeps_cloud() {
    // The safety-critical `group remove` primitive: a Linked member must be
    // turned back into a real file in $HOME while the cloud copy is preserved,
    // and a dry-run must change nothing.
    let (_d, cfg) = sandbox();
    let target = cfg.home.join(".gitconfig");
    write(&target, "[user]\n  name = j\n");
    let (mapping, _) = apply::adopt(&cfg, &target, None, None, &[], false).unwrap();
    let item = state_of(&mapping, &cfg, "mac");
    assert_eq!(item.state, State::Linked);

    // Dry-run leaves the symlink in place.
    let dry = apply::restore_item(&item, true);
    assert!(dry.ok);
    assert!(fsutil::is_symlink(&target), "dry-run must not touch the symlink");

    // Real restore: the symlink becomes a real file, the cloud copy survives.
    let out = apply::restore_item(&item, false);
    assert!(out.ok, "restore failed: {}", out.detail);
    assert!(!fsutil::is_symlink(&target), "target must be a real file after restore");
    assert_eq!(fs::read_to_string(&target).unwrap(), "[user]\n  name = j\n");
    let source = mapping.source(&cfg.sync_dir);
    assert!(fsutil::path_present(&source), "cloud copy must be kept");
    assert_eq!(fs::read_to_string(&source).unwrap(), "[user]\n  name = j\n");
}

#[test]
fn restore_item_on_dangling_symlink_just_unlinks() {
    // If the cloud copy is already gone, there's nothing to restore — the
    // dangling symlink is simply removed (never leaving an empty file behind).
    let (_d, cfg) = sandbox();
    let target = cfg.home.join(".vimrc");
    write(&target, "set nocompatible");
    let (mapping, _) = apply::adopt(&cfg, &target, None, None, &[], false).unwrap();
    fsutil::remove_path(&mapping.source(&cfg.sync_dir)).unwrap();
    let item = state_of(&mapping, &cfg, "mac");
    assert_eq!(item.state, State::DanglingSelf);

    let out = apply::restore_item(&item, false);
    assert!(out.ok, "unlink failed: {}", out.detail);
    assert!(!fsutil::is_symlink(&target), "dangling symlink must be removed");
    assert!(!fsutil::path_present(&target), "no empty file left behind");
}

#[test]
fn doctor_flags_orphan_symlink_drift() {
    // Adopt a file (creating the symlink + cloud copy), then simulate its mapping
    // being removed on another machine: the symlink and cloud copy remain, but no
    // mapping references them. plan() can't see this — doctor's drift scan must.
    let (_d, cfg) = sandbox();
    let target = cfg.home.join(".gitconfig");
    write(&target, "[user]\n");
    apply::adopt(&cfg, &target, None, None, &[], false).unwrap();
    assert!(fsutil::is_symlink(&target));

    // Mappings no longer include it (as if `group remove` ran elsewhere).
    let mappings = MappingsFile::default();
    let report = doctor::run(&cfg, &mappings, "mac", false).unwrap();

    assert!(
        report
            .advisories()
            .any(|i| i.name == ".gitconfig" && i.message.contains("no mapping covers it")),
        "orphan symlink drift should be surfaced as an advisory"
    );
    // Drift is read-only and non-blocking: it must not make the machine unhealthy.
    assert!(report.healthy());
}

#[test]
fn doctor_advises_partially_linked_group() {
    // A group with one member linked here and another only available (synced in
    // but never linked) is drift worth surfacing.
    let (_d, cfg) = sandbox();
    let linked = cfg.home.join(".zshrc");
    write(&linked, "linked");
    let (ma, _) = apply::adopt(&cfg, &linked, None, Some("shell".into()), &[], false).unwrap();

    let avail = cfg.home.join(".bashrc");
    write(&avail, "avail");
    let (mb, _) = apply::adopt(&cfg, &avail, None, Some("shell".into()), &[], false).unwrap();
    fsutil::remove_symlink(&avail).unwrap(); // cloud copy stays → Available, not linked

    let mut mappings = MappingsFile::default();
    mappings.upsert(ma);
    mappings.upsert(mb);
    assert_eq!(state_of(mappings.find(".bashrc").unwrap(), &cfg, "mac").state, State::Available);

    let report = doctor::run(&cfg, &mappings, "mac", false).unwrap();
    assert!(
        report
            .advisories()
            .any(|i| i.name == "shell" && i.message.contains("partially linked")),
        "a partially-linked group should be advised"
    );
}

#[test]
fn doctor_flags_a_conflicted_mappings_file() {
    // A cloud provider forked dotsync.toml (OneDrive-style host suffix, which the
    // generic conflicted-copy scan misses). doctor must call it out specifically.
    let (_d, cfg) = sandbox();
    fs::write(
        cfg.sync_dir.join("dotsync-DESKTOP-AB12.toml"),
        "[[mapping]]\nname = \".vimrc\"\n",
    )
    .unwrap();
    let report = doctor::run(&cfg, &MappingsFile::default(), "mac", false).unwrap();
    assert!(
        report.advisories().any(|i| i.name == "dotsync-DESKTOP-AB12.toml"
            && i.message.contains("conflicted copy of dotsync.toml")),
        "a forked mappings file should be flagged specifically"
    );
    assert!(report.healthy(), "a conflicted copy is an advisory, not fatal");
}

#[test]
fn group_and_mapping_names_cannot_collide() {
    use dotsync_core::mapping::{ensure_free_of_group, ensure_free_of_mapping};
    let mut f = MappingsFile::default();
    let mut m = Mapping::new("Brewfile"); // a bare-named top-level mapping
    m.group = Some("packages".into());
    f.upsert(m);
    // A group name may not equal a mapping name (they share the `install <name>`
    // selector space) — the `/`-and-leading-`.` rule alone doesn't catch this.
    assert!(ensure_free_of_mapping("Brewfile", &f).is_err());
    assert!(ensure_free_of_mapping("claude", &f).is_ok());
    // A new mapping name may not equal an existing group name.
    assert!(ensure_free_of_group("packages", &f).is_err());
    assert!(ensure_free_of_group("Makefile", &f).is_ok());
}

#[test]
fn heal_refuses_when_target_changed_since_planning() {
    // TOCTOU: an item planned as Healable (real file matching cloud) must not be
    // deleted if it changed again before we act — that would lose unsynced edits.
    let (_d, cfg) = sandbox();
    let target = cfg.home.join(".gitconfig");
    write(&target, "orig");
    let (mapping, _) = apply::adopt(&cfg, &target, None, None, &[], false).unwrap();
    fsutil::remove_symlink(&target).unwrap();
    fs::write(&target, "orig").unwrap(); // matches cloud → Healable
    let item = state_of(&mapping, &cfg, "mac");
    assert_eq!(item.state, State::Healable);

    fs::write(&target, "CHANGED after planning").unwrap();
    let out = apply::link_item(&item, false);
    assert!(!out.ok, "heal must refuse when content no longer matches the cloud copy");
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "CHANGED after planning",
        "must not delete unsynced content"
    );
}

#[test]
fn unlink_refuses_when_target_became_a_real_file() {
    // TOCTOU: an item planned as Linked whose symlink an atomic save replaced
    // with real, unsynced content must not be deleted by unlink.
    let (_d, cfg) = sandbox();
    let target = cfg.home.join(".vimrc");
    write(&target, "set nocompatible");
    let (mapping, _) = apply::adopt(&cfg, &target, None, None, &[], false).unwrap();
    let item = state_of(&mapping, &cfg, "mac");
    assert_eq!(item.state, State::Linked);

    fsutil::remove_symlink(&target).unwrap();
    fs::write(&target, "unsynced local edit").unwrap();
    let out = apply::unlink_item(&item, false);
    assert!(out.ok);
    assert!(!fsutil::is_symlink(&target));
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "unsynced local edit",
        "must not delete a real file"
    );
}

#[test]
fn dry_run_link_does_not_fail_on_dangling() {
    // A pure preview must not taint the exit code on a transient state.
    let (_d, cfg) = sandbox();
    let target = cfg.home.join(".zshrc");
    write(&target, "z");
    let (mapping, _) = apply::adopt(&cfg, &target, None, None, &[], false).unwrap();
    fsutil::remove_path(&mapping.source(&cfg.sync_dir)).unwrap(); // cloud gone → dangling
    let item = state_of(&mapping, &cfg, "mac");
    assert_eq!(item.state, State::DanglingSelf);
    let out = apply::link_item(&item, true);
    assert!(out.ok, "a dry-run preview must not fail on a transient dangling state");
    assert_eq!(out.action, "would-skip");
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
