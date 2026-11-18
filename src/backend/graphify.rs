use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use serde::Serialize;

const GRAPH: &str = "graphify-out/graph.json";
const REPORT: &str = "graphify-out/GRAPH_REPORT.md";
const SKIP_DIRS: [&str; 3] = ["node_modules", "target", "dist"];
const MAX_SCANNED: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    pub installed: bool,
    pub version: Option<String>,
    pub graph: bool,
    pub nodes: Option<usize>,
    pub built_at: Option<u64>,
    pub stale: bool,
    pub report: Option<PathBuf>,
}

fn command(args: &[&str], dir: &Path) -> Command {
    let program = std::env::var("AGENTORY_GRAPHIFY").unwrap_or_else(|_| "graphify".to_string());
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(dir)
        .env("PATH", crate::selfbuild::preflight::augmented_path())
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

fn version(dir: &Path) -> Option<String> {
    let out = command(&["--version"], dir).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next().unwrap_or("").trim();
    Some(if line.is_empty() {
        "unknown".to_string()
    } else {
        line.to_string()
    })
}

fn secs(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn node_count(graph: &Path) -> Option<usize> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(graph).ok()?).ok()?;
    Some(v.get("nodes")?.as_array()?.len())
}

fn changed_since(dir: &Path, built: SystemTime) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    let mut seen = 0;
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            seen += 1;
            if seen > MAX_SCANNED {
                return false;
            }
            let name = e.file_name();
            let name = name.to_string_lossy();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if !name.starts_with('.') && !SKIP_DIRS.contains(&&*name) && name != "graphify-out"
                {
                    stack.push(e.path());
                }
            } else if e
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|m| m > built)
            {
                return true;
            }
        }
    }
    false
}

pub fn status(dir: &Path) -> Status {
    let version = version(dir);
    let graph_path = dir.join(GRAPH);
    let built = std::fs::metadata(&graph_path)
        .and_then(|m| m.modified())
        .ok();
    let report = dir.join(REPORT);
    Status {
        installed: version.is_some(),
        version,
        graph: built.is_some(),
        nodes: built.and_then(|_| node_count(&graph_path)),
        built_at: built.map(secs),
        stale: built.is_some_and(|b| changed_since(dir, b)),
        report: report.is_file().then_some(report),
    }
}

pub fn update(dir: &Path) -> Result<String, String> {
    let out = command(&["update", "."], dir).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "graphify is not installed".to_string()
        } else {
            format!("could not run graphify: {e}")
        }
    })?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(match (stderr.is_empty(), stdout.is_empty()) {
        (false, _) => stderr,
        (true, false) => stdout,
        _ => format!("graphify exited with {}", out.status),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_nodes_and_ignores_garbage() {
        let dir = std::env::temp_dir().join(format!("agentory-gf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("graph.json");
        std::fs::write(&f, r#"{"nodes":[{"id":1},{"id":2}],"links":[]}"#).unwrap();
        assert_eq!(node_count(&f), Some(2));
        std::fs::write(&f, "nope").unwrap();
        assert_eq!(node_count(&f), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
