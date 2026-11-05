use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn run(project: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentory"))
        .arg("workflow")
        .args(args)
        .current_dir(project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    if let Some(text) = stdin {
        pipe.write_all(text.as_bytes()).unwrap();
    }
    drop(pipe);
    child.wait_with_output().unwrap()
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
        panic!(
            "stdout was not JSON: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn error_code(out: &Output) -> String {
    let v: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    v["error"]["code"].as_str().unwrap().to_string()
}

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentory-wf-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".agentory")).unwrap();
    dir
}

fn file(dir: &Path, id: &str) -> PathBuf {
    dir.join(".agentory")
        .join("workflows")
        .join(format!("{id}.json"))
}

#[test]
fn create_suffixes_collisions_and_lists() {
    let dir = project("create");
    let out = run(
        &dir,
        &["create", "Fix Bug", "--prompt", "Look at {{input}}"],
        None,
    );
    assert!(out.status.success());
    assert_eq!(json(&out)["id"], "fix-bug");
    let out = run(&dir, &["create", "Fix Bug", "--prompt", "again"], None);
    assert_eq!(json(&out)["id"], "fix-bug-2");

    let list = json(&run(&dir, &["list"], None));
    assert_eq!(list.as_array().unwrap().len(), 2);
    let shown = json(&run(&dir, &["show", "fix-bug"], None));
    assert_eq!(shown["steps"][0]["prompt"], "Look at {{input}}");
    assert_eq!(shown["steps"][0]["name"], "Step 1");
}

#[test]
fn create_without_prompt_is_a_validation_error() {
    let dir = project("noprompt");
    let out = run(&dir, &["create", "Empty"], None);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(error_code(&out), "invalid");
    assert!(!file(&dir, "empty").exists());
}

#[test]
fn apply_replaces_definition_and_rejects_bad_input() {
    let dir = project("apply");
    run(&dir, &["create", "Flow", "--prompt", "one"], None);

    let good = r#"{"name":"Flow 2","steps":[{"name":"a","prompt":"x"},{"name":"b","prompt":"use {{step1}}"}]}"#;
    let out = run(&dir, &["apply", "flow"], Some(good));
    assert!(out.status.success());
    let shown = json(&run(&dir, &["show", "flow"], None));
    assert_eq!(shown["name"], "Flow 2");
    assert_eq!(shown["steps"].as_array().unwrap().len(), 2);

    let before = std::fs::read_to_string(file(&dir, "flow")).unwrap();
    let forward = r#"{"name":"F","steps":[{"name":"a","prompt":"{{step1}}"}]}"#;
    let out = run(&dir, &["apply", "flow"], Some(forward));
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(error_code(&out), "invalid");
    let out = run(&dir, &["apply", "flow"], Some("not json"));
    assert_eq!(error_code(&out), "invalid_json");
    let out = run(&dir, &["apply", "-"], Some(good));
    assert_eq!(error_code(&out), "invalid_json");
    assert_eq!(std::fs::read_to_string(file(&dir, "flow")).unwrap(), before);

    let named = r#"{"id":"other","name":"O","steps":[{"name":"a","prompt":"x"}]}"#;
    assert!(run(&dir, &["apply", "-"], Some(named)).status.success());
    assert!(file(&dir, "other").exists());
}

#[test]
fn step_edits_rename_and_delete() {
    let dir = project("steps");
    run(&dir, &["create", "Flow", "--prompt", "one"], None);

    let add = [
        "step",
        "add",
        "flow",
        "--name",
        "Second",
        "--prompt",
        "see {{prev}}",
    ];
    assert!(run(&dir, &add, None).status.success());
    let out = run(
        &dir,
        &[
            "step", "set", "flow", "2", "--model", "opus", "--skill", "a", "--skill", "b",
        ],
        None,
    );
    let step = &json(&out)["steps"][1];
    assert_eq!(step["model"], "opus");
    assert_eq!(step["skills"], serde_json::json!(["a", "b"]));

    let out = run(&dir, &["step", "move", "flow", "2", "1"], None);
    assert_eq!(json(&out)["steps"][0]["name"], "Second");
    let out = run(&dir, &["step", "rm", "flow", "1"], None);
    assert_eq!(json(&out)["steps"].as_array().unwrap().len(), 1);
    let out = run(&dir, &["step", "rm", "flow", "1"], None);
    assert_eq!(error_code(&out), "invalid");
    let out = run(&dir, &["step", "set", "flow", "9", "--name", "x"], None);
    assert_eq!(error_code(&out), "invalid");

    let out = run(&dir, &["rename", "flow", "Renamed"], None);
    assert_eq!(json(&out)["name"], "Renamed");
    assert_eq!(json(&out)["id"], "flow");

    let out = run(&dir, &["delete", "flow"], None);
    assert!(out.status.success());
    assert!(!file(&dir, "flow").exists());
    let out = run(&dir, &["delete", "flow"], None);
    assert_eq!(error_code(&out), "not_found");
}

#[test]
fn project_resolution() {
    let dir = project("resolve");
    run(&dir, &["create", "Flow", "--prompt", "one"], None);
    let sub = dir.join("a").join("b");
    std::fs::create_dir_all(&sub).unwrap();
    assert_eq!(
        json(&run(&sub, &["list"], None)).as_array().unwrap().len(),
        1
    );

    let bare = std::env::temp_dir().join(format!("agentory-wf-bare-{}", std::process::id()));
    std::fs::create_dir_all(&bare).unwrap();
    let dir_str = dir.to_str().unwrap();
    let out = run(&bare, &["list", "--project", dir_str], None);
    assert_eq!(json(&out).as_array().unwrap().len(), 1);
    assert_eq!(error_code(&run(&bare, &["list"], None)), "no_project");
}
