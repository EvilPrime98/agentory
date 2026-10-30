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
    let mut args = vec!["install", "--yes"];
    args.extend(skills.iter().map(String::as_str));
    capture("spm", &args, project).await
}

/// A skill in the repo pool that `spm available` lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableSkill {
    pub name: String,
    /// Empty when the pool has no usable description.
    pub description: String,
}

/// `spm available`: one skill per line, a padded name column then a truncated description.
pub async fn list_available(project: &Path) -> Result<Vec<AvailableSkill>, String> {
    let out = capture("spm", &["available"], project).await?;
    Ok(parse_available(&out))
}

fn parse_available(out: &str) -> Vec<AvailableSkill> {
    out.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (name, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            if name.is_empty() {
                return None;
            }
            let description = rest.trim();
            // Folded YAML descriptions (`>`) leave a bare marker behind.
            let description = if description == ">" { "" } else { description };
            Some(AvailableSkill {
                name: name.to_string(),
                description: description.to_string(),
            })
        })
        .collect()
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
    fn available_lines_split_on_the_name_column() {
        let out = "alpha          Does a thing\ncleaner-doctor >\nbeta\n\n";
        let got = parse_available(out);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].name, "alpha");
        assert_eq!(got[0].description, "Does a thing");
        assert_eq!(got[1].name, "cleaner-doctor");
        assert_eq!(got[1].description, "");
        assert_eq!(got[2].description, "");
    }

    #[test]
    fn snapshot_of_missing_folder_is_empty() {
        let s = read_snapshot(Path::new("Z:/definitely/not/here"));
        assert!(s.manifest.is_none());
        assert!(s.installed.is_empty());
    }
}
