use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn agentory(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agentory"))
        .arg("settings")
        .args(args)
        .env("AGENTORY_CONFIG_DIR", dir)
        .output()
        .unwrap()
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

fn file(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("prefs.json")).unwrap()).unwrap()
}

fn tempdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentory-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn list_set_get_reset_round_trip() {
    let dir = tempdir("roundtrip");

    let out = agentory(&dir, &["list"]);
    assert!(out.status.success());
    assert!(
        json(&out)
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["key"] == "theme" && s["value"] == "system")
    );

    assert!(agentory(&dir, &["set", "theme", "dark"]).status.success());
    assert!(
        agentory(&dir, &["set", "terminal-font-size", "16"])
            .status
            .success()
    );
    assert_eq!(json(&agentory(&dir, &["get", "theme"]))["value"], "dark");
    assert_eq!(
        json(&agentory(&dir, &["get", "terminal-font-size"]))["value"],
        16.0
    );
    assert_eq!(file(&dir)["theme"], "dark");

    assert!(agentory(&dir, &["reset", "theme"]).status.success());
    assert_eq!(json(&agentory(&dir, &["get", "theme"]))["value"], "system");
    assert_eq!(
        json(&agentory(&dir, &["get", "terminal-font-size"]))["value"],
        16.0
    );
}

#[test]
fn invalid_input_fails_with_structured_error() {
    let dir = tempdir("errors");

    let out = agentory(&dir, &["set", "theme", "blue"]);
    assert_eq!(out.status.code(), Some(2));
    let err: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["error"]["code"], "invalid_value");
    assert_eq!(
        err["error"]["allowed"],
        serde_json::json!(["system", "light", "dark"])
    );

    assert_eq!(
        agentory(&dir, &["set", "terminal-font-size", "40"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        agentory(&dir, &["get", "last-shell"]).status.code(),
        Some(2)
    );
    assert_eq!(agentory(&dir, &["frobnicate"]).status.code(), Some(2));
    assert!(!dir.join("prefs.json").exists());
}

#[test]
fn set_keeps_unrelated_keys() {
    let dir = tempdir("keep");
    std::fs::write(
        dir.join("prefs.json"),
        r#"{"lastShell":"pwsh","notifications":false}"#,
    )
    .unwrap();
    assert!(agentory(&dir, &["set", "theme", "light"]).status.success());
    let f = file(&dir);
    assert_eq!(f["lastShell"], "pwsh");
    assert_eq!(f["notifications"], false);
    assert_eq!(f["theme"], "light");
}
