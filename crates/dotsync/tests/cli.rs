//! CLI smoke tests: argument parsing and the embedded `--llm` guide.

use assert_cmd::Command;

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
