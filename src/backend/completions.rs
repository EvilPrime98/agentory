
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

const MAX_FILES: usize = 20_000;
const MAX_DEPTH: usize = 8;
const SKIP_DIRS: [&str; 5] = [".git", "node_modules", "target", ".venv", "__pycache__"];

const BUILTIN: [(&str, &str); 34] = [
    ("add-dir", "Add a working directory"),
    ("agents", "Manage subagents"),
    ("clear", "Clear the conversation"),
    ("compact", "Compact the conversation"),
    ("config", "Open the settings"),
    ("context", "Show context usage"),
    ("cost", "Show token usage and cost"),
    ("doctor", "Check the installation"),
    ("exit", "Exit Claude Code"),
    ("export", "Export the conversation"),
    ("help", "Show help"),
    ("hooks", "Manage hooks"),
    ("ide", "Manage IDE integrations"),
    ("init", "Create a CLAUDE.md for this project"),
    ("install-github-app", "Set up the GitHub app"),
    ("login", "Sign in"),
    ("logout", "Sign out"),
    ("mcp", "Manage MCP servers"),
    ("memory", "Edit memory files"),
    ("model", "Choose the model"),
    ("output-style", "Choose the output style"),
    ("permissions", "Manage tool permissions"),
    ("plugin", "Manage plugins"),
    ("pr-comments", "Show pull request comments"),
    ("release-notes", "Show the release notes"),
    ("resume", "Resume a conversation"),
    ("review", "Review a pull request"),
    ("rewind", "Rewind the conversation"),
    ("security-review", "Review pending changes for security"),
    ("status", "Show the status"),
    ("statusline", "Set up the status line"),
    ("terminal-setup", "Install the Shift+Enter binding"),
    ("usage", "Show plan usage"),
    ("vim", "Toggle vim editing mode"),
];

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub value: String,
    pub detail: String,
    pub dir: bool,
}

#[derive(Debug, Default)]
pub struct Index {
    files: Vec<Candidate>,
    commands: Vec<Candidate>,
}

impl Index {
    pub fn build(cwd: &Path) -> Self {
        Self {
            files: file_candidates(cwd),
            commands: command_candidates(cwd),
        }
    }

    pub fn files(&self, query: &str) -> Vec<Candidate> {
        rank(&self.files, query, true)
    }

    pub fn commands(&self, query: &str) -> Vec<Candidate> {
        rank(&self.commands, query, false)
    }
}

fn list_files(cwd: &Path) -> Vec<String> {
    let mut cmd = Command::new("git");
    cmd.args(["ls-files", "-co", "--exclude-standard"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000);
    if let Ok(out) = cmd.output()
        && out.status.success()
    {
        return String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.replace('\\', "/"))
            .filter(|l| !l.is_empty())
            .take(MAX_FILES)
            .collect();
    }
    let mut out = Vec::new();
    walk(cwd, "", 0, &mut out);
    out
}

fn walk(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<String>) {
    if depth > MAX_DEPTH {
        return;
    }
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        if out.len() >= MAX_FILES {
            return;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = format!("{prefix}{name}");
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk(&entry.path(), &format!("{rel}/"), depth + 1, out);
            }
        } else {
            out.push(rel);
        }
    }
}

fn file_candidates(cwd: &Path) -> Vec<Candidate> {
    let files = list_files(cwd);
    let mut dirs = BTreeSet::new();
    for f in &files {
        let mut at = 0;
        while let Some(i) = f[at..].find('/') {
            at += i + 1;
            dirs.insert(f[..at].to_string());
        }
    }
    dirs.into_iter()
        .map(|d| Candidate {
            value: d,
            detail: String::new(),
            dir: true,
        })
        .chain(files.into_iter().map(|f| Candidate {
            value: f,
            detail: String::new(),
            dir: false,
        }))
        .collect()
}

fn description(path: &Path) -> String {
    let Ok(text) = fs::read_to_string(path) else {
        return String::new();
    };
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return String::new();
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some(v) = line.strip_prefix("description:") {
            return v.trim().trim_matches(['"', '\'']).to_string();
        }
    }
    String::new()
}

fn skills_in(dir: &Path, out: &mut Vec<Candidate>) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let skill = entry.path().join("SKILL.md");
        if skill.is_file() {
            out.push(Candidate {
                value: entry.file_name().to_string_lossy().into_owned(),
                detail: description(&skill),
                dir: false,
            });
        }
    }
}

fn commands_in(dir: &Path, prefix: &str, out: &mut Vec<Candidate>) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            commands_in(&path, &format!("{prefix}{name}:"), out);
        } else if let Some(stem) = name.strip_suffix(".md") {
            out.push(Candidate {
                value: format!("{prefix}{stem}"),
                detail: description(&path),
                dir: false,
            });
        }
    }
}

fn command_candidates(cwd: &Path) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut roots = vec![cwd.join(".claude")];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".claude"));
    }
    for root in &roots {
        skills_in(&root.join("skills"), &mut out);
        commands_in(&root.join("commands"), "", &mut out);
    }
    out.extend(BUILTIN.iter().map(|(name, detail)| Candidate {
        value: (*name).to_string(),
        detail: (*detail).to_string(),
        dir: false,
    }));
    out.sort_by(|a, b| a.value.cmp(&b.value));
    out.dedup_by(|a, b| a.value == b.value);
    out
}

pub const MAX_RESULTS: usize = 8;

fn score(value: &str, query: &str, path: bool) -> Option<usize> {
    let v = value.to_lowercase();
    let q = query.to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    let name = if path {
        v.trim_end_matches('/').rsplit('/').next().unwrap_or(&v)
    } else {
        &v
    };
    if name.starts_with(&q) {
        return Some(v.len());
    }
    if v.starts_with(&q) {
        return Some(v.len() + 1);
    }
    if let Some(i) = name.find(&q) {
        return Some(1_000 + i * 10 + v.len());
    }
    if let Some(i) = v.find(&q) {
        return Some(2_000 + i * 10 + v.len());
    }
    let mut chars = v.chars();
    q.chars()
        .all(|c| chars.any(|x| x == c))
        .then(|| 3_000 + v.len())
}

fn rank(items: &[Candidate], query: &str, path: bool) -> Vec<Candidate> {
    let mut hits: Vec<(usize, &Candidate)> = items
        .iter()
        .filter_map(|c| score(&c.value, query, path).map(|s| (s, c)))
        .collect();
    hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.value.cmp(&b.1.value)));
    hits.into_iter()
        .take(MAX_RESULTS)
        .map(|(_, c)| c.clone())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    File(String),
    Command(String),
}

impl Trigger {
    pub fn query(&self) -> &str {
        match self {
            Self::File(q) | Self::Command(q) => q,
        }
    }

    pub fn token_len(&self) -> usize {
        1 + self.query().chars().count()
    }

    pub fn sigil(&self) -> char {
        match self {
            Self::File(_) => '@',
            Self::Command(_) => '/',
        }
    }
}

pub fn trigger(before_caret: &str) -> Option<Trigger> {
    let word_start = before_caret.rfind(char::is_whitespace).map_or(0, |i| {
        i + before_caret[i..].chars().next().map_or(1, char::len_utf8)
    });
    let word = &before_caret[word_start..];
    if let Some(q) = word.strip_prefix('@') {
        return Some(Trigger::File(q.to_string()));
    }
    if word_start == 0
        && let Some(q) = word.strip_prefix('/')
    {
        return Some(Trigger::Command(q.to_string()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggers() {
        assert_eq!(
            trigger("look at @src/m"),
            Some(Trigger::File("src/m".into()))
        );
        assert_eq!(trigger("@"), Some(Trigger::File(String::new())));
        assert_eq!(trigger("/com"), Some(Trigger::Command("com".into())));
        assert_eq!(trigger("a /com"), None);
        assert_eq!(trigger("mail me@x.com now"), None);
        assert_eq!(trigger("fix\n@a"), Some(Trigger::File("a".into())));
        assert_eq!(trigger("/compact "), None);
        assert_eq!(trigger("héllo @é"), Some(Trigger::File("é".into())));
    }

    #[test]
    fn ranking_prefers_names() {
        let c = |v: &str| Candidate {
            value: v.into(),
            detail: String::new(),
            dir: false,
        };
        let items = [
            c("docs/main.rs"),
            c("src/main.rs"),
            c("src/ui/mod.rs"),
            c("README.md"),
        ];
        let top = rank(&items, "main", true);
        assert_eq!(top[0].value, "src/main.rs");
        assert_eq!(top.len(), 2);
        assert_eq!(rank(&items, "smr", true)[0].value, "src/main.rs");
        assert_eq!(rank(&items, "", true).len(), 4);
        assert!(rank(&items, "zzz", true).is_empty());
    }

    #[test]
    fn builtin_and_dedup() {
        let index = Index {
            files: Vec::new(),
            commands: command_candidates(Path::new("does-not-exist")),
        };
        assert_eq!(index.commands("compa")[0].value, "compact");
    }
}
