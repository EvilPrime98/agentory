use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run(project: &Path, session: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agentory"))
        .arg("tasks")
        .args(args)
        .arg("--project")
        .arg(project)
        .env("AGENTORY_SESSION", session)
        .output()
        .unwrap()
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

fn project() -> PathBuf {
    let test = std::thread::current()
        .name()
        .unwrap_or("t")
        .replace("::", "-");
    let dir =
        std::env::temp_dir().join(format!("agentory-tasks-cli-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn post_claim_close_round_trip() {
    let dir = project();
    let posted = run(&dir, "u", &["post", "Fix login", "--body", "see issue"]);
    assert!(posted.status.success());
    assert_eq!(json(&posted)["id"], "t1");

    let claimed = run(&dir, "a", &["claim"]);
    assert_eq!(json(&claimed)["assignee"], "a");
    assert_eq!(json(&run(&dir, "b", &["claim"])), serde_json::Value::Null);

    let stolen = run(&dir, "b", &["close", "t1"]);
    assert_eq!(error_code(&stolen), "not_owner");
    let closed = run(&dir, "a", &["close", "t1", "--summary", "done it"]);
    assert_eq!(json(&closed)["status"], "review");
    assert_eq!(error_code(&run(&dir, "a", &["close", "t1"])), "closed");

    let open = run(&dir, "a", &["list", "--status", "open"]);
    assert_eq!(json(&open).as_array().unwrap().len(), 0);
    assert_eq!(error_code(&run(&dir, "a", &["show", "nope"])), "not_found");
}

#[test]
fn dependencies_block_claims_and_are_validated() {
    let dir = project();
    run(&dir, "u", &["post", "first", "--accept", "builds"]);
    let second = run(&dir, "u", &["post", "second", "--depends-on", "t1"]);
    assert_eq!(json(&second)["dependsOn"][0], "t1");
    assert_eq!(
        error_code(&run(&dir, "u", &["post", "bad", "--depends-on", "t9"])),
        "invalid"
    );
    assert_eq!(error_code(&run(&dir, "b", &["claim", "t2"])), "blocked");
    assert_eq!(json(&run(&dir, "a", &["claim"]))["id"], "t1");
    assert_eq!(json(&run(&dir, "b", &["claim"])), serde_json::Value::Null);
}
