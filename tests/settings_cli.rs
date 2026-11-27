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
fn folder_settings_have_app_and_project_levels() {
    let dir = tempdir("folders");
    let root = tempdir("folders-project");
    let projects = serde_json::json!([
        { "id": "p1", "name": "demo", "path": root, "defaultShell": "pwsh" }
    ]);
    std::fs::write(dir.join("projects.json"), projects.to_string()).unwrap();
    let saved = || -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join("projects.json")).unwrap()).unwrap()
    };
    let inside = root.join("src");
    let inside = inside.to_str().unwrap();
    let get = |args: &[&str]| json(&agentory(&dir, args))["value"].clone();
    let for_project = ["get", "docs-dir", "--project", inside];

    assert_eq!(get(&for_project), "docs");
    assert!(
        agentory(&dir, &["set", "docs-dir", "documentation/"])
            .status
            .success()
    );
    assert_eq!(file(&dir)["docsDir"], "documentation");
    assert_eq!(get(&for_project), "documentation");

    let out = agentory(
        &dir,
        &["set", "docs-dir", r"notes\specs", "--project", inside],
    );
    assert_eq!(json(&out)["value"], "notes/specs");
    assert_eq!(saved()[0]["docsDir"], "notes/specs");
    assert_eq!(saved()[0]["defaultShell"], "pwsh");
    assert_eq!(get(&["get", "docs-dir"]), "documentation");

    for bad in [".", "../x", "/etc", r"C:\x"] {
        for args in [
            vec!["set", "docs-dir", bad],
            vec!["set", "docs-dir", bad, "--project", inside],
        ] {
            let out = agentory(&dir, &args);
            assert_eq!(out.status.code(), Some(2), "{bad}");
            let err: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
            assert_eq!(err["error"]["code"], "invalid_value");
        }
    }
    assert_eq!(get(&for_project), "notes/specs");

    assert!(
        agentory(&dir, &["set", "docs-dir", "", "--project", inside])
            .status
            .success()
    );
    assert!(saved()[0].get("docsDir").is_none());
    assert!(
        agentory(&dir, &["set", "docs-dir", "x", "--project", inside])
            .status
            .success()
    );
    let out = agentory(&dir, &["reset", "docs-dir", "--project", inside]);
    assert_eq!(json(&out)["value"], "documentation");
    assert!(saved()[0].get("docsDir").is_none());

    assert!(agentory(&dir, &["set", "docs-dir", " "]).status.success());
    assert_eq!(get(&["get", "docs-dir"]), "docs");

    std::fs::write(dir.join("prefs.json"), r#"{"issuesDir":"../out"}"#).unwrap();
    assert_eq!(get(&["get", "issues-dir"]), "issues");
}

#[test]
fn project_flag_is_limited_to_folder_keys_and_added_projects() {
    let dir = tempdir("project-flag");
    let path = dir.to_str().unwrap();
    let code = |args: &[&str]| -> (Option<i32>, serde_json::Value) {
        let out = agentory(&dir, args);
        let err: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
        (out.status.code(), err["error"]["code"].clone())
    };
    for args in [
        vec!["get", "theme", "--project", path],
        vec!["list", "--project", path],
        vec!["get", "docs-dir", "--project"],
    ] {
        assert_eq!(code(&args), (Some(2), "usage".into()), "{args:?}");
    }
    assert_eq!(
        code(&["set", "docs-dir", "x", "--project", path]),
        (Some(2), "not_registered".into())
    );
    assert!(!dir.join("projects.json").exists());
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
