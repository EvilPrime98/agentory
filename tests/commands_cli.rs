use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

struct Env {
    config: PathBuf,
    project: PathBuf,
}

fn setup(name: &str) -> Env {
    let base = std::env::temp_dir().join(format!("agentory-cmd-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (config, project) = (base.join("config"), base.join("project"));
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(project.join("src")).unwrap();
    let saved = json!([{ "id": "p1", "name": "project", "path": project }]);
    std::fs::write(config.join("projects.json"), saved.to_string()).unwrap();
    Env { config, project }
}

fn run(env: &Env, dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agentory"))
        .arg("commands")
        .args(args)
        .current_dir(dir)
        .env("AGENTORY_CONFIG_DIR", &env.config)
        .output()
        .unwrap()
}

fn agentory(env: &Env, args: &[&str]) -> Output {
    run(env, &env.project, args)
}

fn json_out(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
        panic!(
            "stdout was not JSON: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn error(out: &Output) -> Value {
    let v: Value = serde_json::from_slice(&out.stderr).unwrap();
    v["error"].clone()
}

fn stored(env: &Env) -> Value {
    let text = std::fs::read_to_string(env.config.join("projects.json")).unwrap();
    serde_json::from_str::<Value>(&text).unwrap()[0].clone()
}

#[test]
fn add_set_rm_round_trip() {
    let env = setup("round");

    let out = agentory(&env, &["add", "Dev Server", "--command", "npm run dev"]);
    assert!(out.status.success());
    assert_eq!(json_out(&out)["id"], "dev-server");

    let out = agentory(&env, &["add", "Dev Server", "--command", "npm start"]);
    assert_eq!(json_out(&out)["id"], "dev-server-2");

    let out = agentory(&env, &["set", "dev-server", "--command", "pnpm dev"]);
    assert!(out.status.success());
    assert_eq!(stored(&env)["commands"][0]["command"], "pnpm dev");

    let out = agentory(&env, &["list"]);
    let items = json_out(&out);
    let custom: Vec<_> = items
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["source"] == "custom")
        .collect();
    assert_eq!(custom.len(), 2);

    let out = agentory(&env, &["rm", "dev-server"]);
    assert!(out.status.success());
    assert_eq!(stored(&env)["commands"].as_array().unwrap().len(), 1);
    assert_eq!(stored(&env)["commands"][0]["id"], "dev-server-2");
}

#[test]
fn works_from_a_subdirectory_and_with_project_flag() {
    let env = setup("lookup");
    let out = run(&env, &env.project.join("src"), &["list"]);
    assert!(out.status.success());

    let elsewhere = env.config.clone();
    let out = run(
        &env,
        &elsewhere,
        &["list", "--project", env.project.to_str().unwrap()],
    );
    assert!(out.status.success());
}

#[test]
fn unregistered_directory_fails_with_not_registered() {
    let env = setup("unreg");
    let out = run(&env, &env.config, &["list"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(error(&out)["code"], "not_registered");
}

#[test]
fn detected_commands_are_listed_and_read_only() {
    let env = setup("detected");
    std::fs::write(env.project.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

    let out = agentory(&env, &["list"]);
    let items = json_out(&out);
    assert!(
        items
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["source"] == "detected" && c["id"].is_null() && c["name"] == "cargo build")
    );

    let out = agentory(&env, &["rm", "cargo build"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(error(&out)["code"], "read_only");

    let out = agentory(&env, &["set", "nope", "--name", "x"]);
    assert_eq!(error(&out)["code"], "not_found");
}

#[test]
fn shell_validation_and_default_shell() {
    let env = setup("shell");

    let out = agentory(&env, &["shell", "no-such-shell"]);
    assert_eq!(out.status.code(), Some(2));
    let e = error(&out);
    assert_eq!(e["code"], "invalid");
    let allowed = e["allowed"][0].as_str().unwrap().to_string();

    let out = agentory(
        &env,
        &["add", "Build", "--command", "make", "--shell", "bogus"],
    );
    assert_eq!(error(&out)["code"], "invalid");

    let out = agentory(&env, &["shell", &allowed]);
    assert!(out.status.success());
    assert_eq!(json_out(&out)["shell"], allowed.as_str());
    assert_eq!(stored(&env)["defaultShell"], allowed.as_str());

    let out = agentory(&env, &["shell"]);
    assert_eq!(json_out(&out)["shell"], allowed.as_str());

    let out = agentory(&env, &["shell", "--clear"]);
    assert!(json_out(&out)["shell"].is_null());
    assert!(stored(&env).get("defaultShell").is_none());
}
