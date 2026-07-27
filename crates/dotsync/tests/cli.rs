//! CLI smoke tests: argument parsing, the embedded `--llm` guide, and the
//! `group` CRUD verbs driven end-to-end over a configured temp environment.

use std::fs;
use std::path::PathBuf;

use assert_cmd::Command;
use dotsync_core::mapping::{Mapping, MappingsFile};

/// A fully-configured dotsync environment on a temp filesystem: a `config.toml`
/// under `$XDG_CONFIG_HOME/dotsync` pointing at a cloud `sync_dir` that holds a
/// `dotsync.toml` with the given `(name, group)` mappings. Returns the tempdir
/// (kept alive), the home dir, the XDG dir, and the sync dir.
fn configured(mappings: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    let xdg = d.path().join("xdg");
    let sync = d.path().join("cloud/dotsync");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(xdg.join("dotsync")).unwrap();
    fs::create_dir_all(&sync).unwrap();

    fs::write(
        xdg.join("dotsync/config.toml"),
        format!("sync_dir = \"{}\"\n", sync.display()),
    )
    .unwrap();

    let mut file = MappingsFile::default();
    for (name, group) in mappings {
        let mut m = Mapping::new(*name);
        m.group = Some((*group).to_string());
        file.upsert(m);
    }
    file.save(&sync.join(MappingsFile::FILE_NAME)).unwrap();

    (d, home, xdg, sync)
}

/// `dotsync` invocation wired to a configured sandbox.
fn dotsync(home: &std::path::Path, xdg: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("dotsync").unwrap();
    cmd.env("HOME", home).env("XDG_CONFIG_HOME", xdg);
    cmd
}

fn reload(sync: &std::path::Path) -> MappingsFile {
    MappingsFile::load(&sync.join(MappingsFile::FILE_NAME)).unwrap()
}

#[test]
fn group_list_reports_groups_and_members_as_json() {
    let (_d, home, xdg, _sync) = configured(&[
        (".claude/CLAUDE.md", "claude"),
        (".claude/settings.json", "claude"),
        (".config/zed/settings.json", "zed"),
    ]);
    let out = dotsync(&home, &xdg)
        .args(["group", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let groups = v["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    let claude = groups.iter().find(|g| g["name"] == "claude").unwrap();
    assert_eq!(claude["members"].as_array().unwrap().len(), 2);
}

#[test]
fn group_move_reassigns_a_single_mapping() {
    let (_d, home, xdg, sync) = configured(&[
        (".config/zed/settings.json", "zed"),
        (".config/nvim/init.lua", "nvim"),
    ]);
    dotsync(&home, &xdg)
        .args(["group", "move", ".config/zed/settings.json", "editors", "--json"])
        .assert()
        .success();
    let f = reload(&sync);
    let moved = f.find(".config/zed/settings.json").unwrap();
    assert_eq!(moved.group.as_deref(), Some("editors"));
}

#[test]
fn group_rename_relabels_all_members_and_merges_on_collision() {
    let (_d, home, xdg, sync) = configured(&[
        (".config/zed/settings.json", "zed"),
        (".config/code/settings.json", "code"),
    ]);
    // Rename `zed` onto the existing `code` group → an explicit merge.
    let out = dotsync(&home, &xdg)
        .args(["group", "rename", "zed", "code", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["merged"], serde_json::json!(true));

    let f = reload(&sync);
    assert_eq!(f.groups(), vec!["code".to_string()]);
    assert!(f
        .mappings
        .iter()
        .all(|m| m.group.as_deref() == Some("code")));
}

#[test]
fn group_remove_refuses_without_yes_in_json_mode() {
    // The all-machines blast radius must never be triggered non-interactively
    // without an explicit --yes.
    let (_d, home, xdg, sync) = configured(&[(".config/zed/settings.json", "zed")]);
    // In --json mode the refusal is a structured error on stdout, not stderr.
    dotsync(&home, &xdg)
        .args(["group", "remove", "zed", "--json"])
        .assert()
        .failure()
        .stdout(predicates::str::contains("--yes"));
    // The mapping must still be present — nothing was removed.
    assert!(reload(&sync).find(".config/zed/settings.json").is_some());
}

#[test]
fn group_name_cannot_shadow_a_mapping() {
    // A group renamed onto a bare mapping name (Brewfile) must be refused —
    // otherwise the group shadows the mapping in the `install <name>` selector.
    let (_d, home, xdg, sync) = configured(&[
        (".config/zed/settings.json", "zed"),
        ("Brewfile", "packages"),
    ]);
    dotsync(&home, &xdg)
        .args(["group", "rename", "zed", "Brewfile"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("already a mapping name"));
    assert_eq!(
        reload(&sync).find(".config/zed/settings.json").unwrap().group.as_deref(),
        Some("zed"),
        "the rename must not have taken effect"
    );
}

#[test]
fn group_move_to_a_new_group_reports_created() {
    let (_d, home, xdg, _sync) = configured(&[(".config/zed/settings.json", "zed")]);
    let out = dotsync(&home, &xdg)
        .args(["group", "move", ".config/zed/settings.json", "editors", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["created"], serde_json::json!(true));
}

#[test]
fn doctor_prints_advisories_to_stdout() {
    // A forked dotsync.toml is an advisory; it must land on stdout so `doctor`
    // output is capturable as a whole (regression guard for the stderr split).
    let (_d, home, xdg, sync) = configured(&[(".config/zed/settings.json", "zed")]);
    std::fs::write(sync.join("dotsync-HOST.toml"), "").unwrap();
    let out = dotsync(&home, &xdg)
        .arg("doctor")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(
        s.contains("conflicted copy of dotsync.toml"),
        "advisory must be on stdout; got: {s}"
    );
}

#[test]
fn group_remove_dry_run_changes_nothing() {
    let (_d, home, xdg, sync) = configured(&[(".config/zed/settings.json", "zed")]);
    dotsync(&home, &xdg)
        .args(["group", "remove", "zed", "--dry-run", "--json"])
        .assert()
        .success();
    assert!(reload(&sync).find(".config/zed/settings.json").is_some());
}

#[test]
fn version_prints() {
    Command::cargo_bin("dotsync")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicates::str::contains("dotsync"));
}

#[test]
fn llm_guide_is_self_contained() {
    Command::cargo_bin("dotsync")
        .unwrap()
        .arg("--llm")
        .assert()
        .success()
        .stdout(predicates::str::contains("COMMAND REFERENCE"))
        .stdout(predicates::str::contains("WORKFLOWS"))
        .stdout(predicates::str::contains("dotsync adopt"));
}

#[test]
fn status_without_config_errors_cleanly() {
    // Point config + home at an empty dir so no real config is found.
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("dotsync")
        .unwrap()
        .env("XDG_CONFIG_HOME", dir.path())
        .env("HOME", dir.path())
        .arg("status")
        .assert()
        .failure()
        .stderr(predicates::str::contains("not configured"));
}
