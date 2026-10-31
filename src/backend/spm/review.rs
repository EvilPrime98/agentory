//! Pre-install review of a skill folder: what is in it and what goes beyond plain instructions.
//! Port of `inspectSkill` / `formatReview` from the Node CLI.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::store::locale_cmp;

const CODE_EXT: &[&str] = &[
    "sh", "bash", "zsh", "ps1", "bat", "cmd", "js", "mjs", "cjs", "ts", "py", "rb", "pl", "php",
    "exe", "dll", "so", "dylib", "jar", "bin",
];
const LISTED: usize = 25;

/// Result of inspecting a skill folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Relative, `/`-separated paths with their sizes.
    pub files: Vec<(String, u64)>,
    pub size: u64,
    /// Scripts, binaries, symlinks and `allowed-tools`.
    pub flags: Vec<String>,
    pub symlinks: Vec<String>,
}

pub fn inspect_skill(dir: &Path) -> std::io::Result<Report> {
    fn walk(root: &Path, d: &Path, r: &mut Report) -> std::io::Result<()> {
        let mut entries: Vec<_> = fs::read_dir(d)?.collect::<Result<_, _>>()?;
        entries.sort_by(|a, b| {
            locale_cmp(&a.file_name().to_string_lossy(), &b.file_name().to_string_lossy())
        });
        for e in entries {
            if e.file_name() == ".git" {
                continue;
            }
            let full = e.path();
            let rel = full
                .strip_prefix(root)
                .unwrap_or(&full)
                .to_string_lossy()
                .replace('\\', "/");
            let ty = e.file_type()?;
            if ty.is_symlink() {
                r.symlinks.push(rel);
            } else if ty.is_dir() {
                walk(root, &full, r)?;
            } else {
                r.files.push((rel.clone(), fs::metadata(&full)?.len()));
                if is_executable(&full, &rel)? {
                    r.flags.push(format!("script or binary: {rel}"));
                }
            }
        }
        Ok(())
    }
    let mut r = Report::default();
    walk(dir, dir, &mut r)?;
    for rel in &r.symlinks {
        r.flags.push(format!("symlink: {rel}"));
    }
    let skill_md = fs::read_to_string(dir.join("SKILL.md")).unwrap_or_default();
    if let Some(tools) = allowed_tools(&skill_md) {
        r.flags.push(format!(
            "allowed-tools: {}",
            if tools.is_empty() { "(list)" } else { &tools }
        ));
    }
    r.size = r.files.iter().map(|f| f.1).sum();
    Ok(r)
}

/// The `allowed-tools:` line of the front matter, if any.
fn allowed_tools(skill_md: &str) -> Option<String> {
    let rest = skill_md.strip_prefix("---")?;
    let rest = rest.strip_prefix("\r\n").or_else(|| rest.strip_prefix('\n'))?;
    let end = rest.find("\n---")?;
    rest[..end]
        .lines()
        .find_map(|l| l.strip_prefix("allowed-tools:").map(|v| v.trim().to_string()))
}

fn is_executable(full: &Path, rel: &str) -> std::io::Result<bool> {
    let ext = Path::new(rel)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if CODE_EXT.contains(&ext.as_str()) || rel.split('/').any(|p| p == "scripts") {
        return Ok(true);
    }
    let mut buf = [0u8; 4096];
    let n = fs::File::open(full)?.read(&mut buf)?;
    Ok(buf[..n].contains(&0))
}

fn kb(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else {
        format!("{:.1} KB", n as f64 / 1024.0)
    }
}

/// What is about to be installed, shown to the user before anything is copied.
#[derive(Debug, Clone)]
pub struct Review {
    pub name: String,
    pub repo: String,
    pub sha: String,
    pub hash: String,
    pub market: String,
    /// Commit currently installed, when this is an update.
    pub from: Option<String>,
    /// Install into the global skills folder instead of the project.
    pub global: bool,
    pub report: Report,
    /// The skill folder inside the cached clone, checked again against `hash` on confirm.
    pub(super) src: PathBuf,
}

impl Review {
    /// The review text: source, commit, hash, file list and risk flags.
    pub fn text(&self) -> String {
        let mut l = vec![format!("source  {}", self.repo)];
        let from = self
            .from
            .as_ref()
            .map(|f| format!("{} -> ", f.chars().take(12).collect::<String>()))
            .unwrap_or_default();
        l.push(format!("commit  {from}{}", self.sha));
        l.push(format!("hash    {}", self.hash));
        l.push(format!("files   {} ({})", self.report.files.len(), kb(self.report.size)));
        for (p, s) in self.report.files.iter().take(LISTED) {
            l.push(format!("  {p}  {}", kb(*s)));
        }
        if self.report.files.len() > LISTED {
            l.push(format!("  ... {} more", self.report.files.len() - LISTED));
        }
        if !self.report.flags.is_empty() {
            l.push("! beyond plain instructions:".into());
            l.extend(self.report.flags.iter().map(|f| format!("  {f}")));
        }
        l.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_scripts_tools_and_binaries() {
        let d = std::env::temp_dir().join(format!("agentory-review-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("scripts")).unwrap();
        fs::write(d.join("SKILL.md"), "---\nname: x\nallowed-tools: Bash, Read\n---\nhi").unwrap();
        fs::write(d.join("scripts/run.txt"), "plain").unwrap();
        fs::write(d.join("tool.py"), "print(1)").unwrap();
        fs::write(d.join("blob.dat"), [1u8, 0, 2]).unwrap();
        fs::write(d.join("notes.md"), "text").unwrap();
        let r = inspect_skill(&d).unwrap();
        assert_eq!(r.files.len(), 5);
        for want in ["scripts/run.txt", "tool.py", "blob.dat"] {
            assert!(r.flags.contains(&format!("script or binary: {want}")), "{want}: {:?}", r.flags);
        }
        assert!(!r.flags.iter().any(|f| f.contains("notes.md")));
        assert!(r.flags.contains(&"allowed-tools: Bash, Read".to_string()));
        let _ = fs::remove_dir_all(&d);
    }
}
