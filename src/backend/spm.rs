//! `spm` integration. Reads `spm.json` and `.claude/skills` directly; mutations shell out to
//! `spm`; the context-cost report is `spm doctor --json`.

use std::path::Path;

use super::capture;
use crate::model::{DoctorReport, SpmManifest};

/// What is on disk for a project.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// `None` when the project has no `spm.json`.
    pub manifest: Option<SpmManifest>,
    /// Folder names under `.claude/skills`.
    pub installed: Vec<String>,
}

pub fn read_snapshot(project: &Path) -> Snapshot {
    let manifest = std::fs::read_to_string(project.join("spm.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    let mut installed: Vec<String> = std::fs::read_dir(project.join(".claude").join("skills"))
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    installed.sort();
    Snapshot {
        manifest,
        installed,
    }
}

pub async fn install(project: &Path, skills: &[String]) -> Result<String, String> {
    let mut args = vec!["install"];
    args.extend(skills.iter().map(String::as_str));
    capture("spm", &args, project).await
}

pub async fn uninstall(project: &Path, skill: &str) -> Result<String, String> {
    capture("spm", &["uninstall", skill], project).await
}

pub async fn update(project: &Path) -> Result<String, String> {
    capture("spm", &["update"], project).await
}

pub async fn repo_add(project: &Path, repo: &str) -> Result<String, String> {
    capture("spm", &["repo", "add", repo], project).await
}

pub async fn repo_rm(project: &Path, repo: &str) -> Result<String, String> {
    capture("spm", &["repo", "rm", repo], project).await
}

pub async fn doctor(project: &Path) -> Result<DoctorReport, String> {
    let out = capture("spm", &["doctor", "--json"], project).await?;
    // The CLI may print a banner before the JSON object.
    let start = out.find('{').ok_or("spm doctor printed no JSON")?;
    serde_json::from_str(&out[start..]).map_err(|e| format!("unreadable spm doctor output: {e}"))
}

/// Is `spm` on PATH?
pub async fn available() -> bool {
    capture("spm", &["--help"], &std::env::temp_dir())
        .await
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_of_missing_folder_is_empty() {
        let s = read_snapshot(Path::new("Z:/definitely/not/here"));
        assert!(s.manifest.is_none());
        assert!(s.installed.is_empty());
    }
}
