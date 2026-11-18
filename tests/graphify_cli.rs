use std::path::PathBuf;
use std::process::{Command, Output};

fn run(project: &PathBuf, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agentory"))
        .arg("graphify")
        .args(args)
        .current_dir(project)
        .env("AGENTORY_GRAPHIFY", "agentory-no-such-graphify")
        .output()
        .unwrap()
}

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentory-gf-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".agentory")).unwrap();
    dir
}

fn json(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes)
        .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(bytes)))
}

#[test]
fn status_without_graphify_reports_not_installed() {
    let dir = project("missing");
    let out = run(&dir, &["status", "--json"]);
    assert!(out.status.success());
    let v = json(&out.stdout);
    assert_eq!(v["installed"], false);
    assert_eq!(v["graph"], false);
}

#[test]
fn update_without_graphify_fails_with_structured_error() {
    let dir = project("update-missing");
    let out = run(&dir, &["update"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json(&out.stderr)["error"]["code"], "not_installed");
}

#[test]
fn status_reads_node_count_from_graph_json() {
    let dir = project("graph");
    let out_dir = dir.join("graphify-out");
    std::fs::create_dir_all(&out_dir).unwrap();
    std::fs::write(
        out_dir.join("graph.json"),
        r#"{"nodes":[{"id":"a"},{"id":"b"},{"id":"c"}]}"#,
    )
    .unwrap();
    let out = run(&dir, &["status", "--json"]);
    let v = json(&out.stdout);
    assert_eq!(v["graph"], true);
    assert_eq!(v["nodes"], 3);
    assert_eq!(v["stale"], false);
}

#[test]
fn rejects_unknown_command_and_extra_args() {
    let dir = project("usage");
    for args in [&["bogus"][..], &["status", "extra"][..], &[][..]] {
        let out = run(&dir, args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
}
