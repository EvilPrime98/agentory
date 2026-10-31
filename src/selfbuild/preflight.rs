//! Toolchain preflight: is everything needed to build Agentory from source installed?
//!
//! Detection is split into pure parsers (unit-tested over fake command output) and thin async
//! probes that run the real commands. Tools installed by `winget` are not on the running app's
//! PATH until it restarts, so lookups and child processes also search [`extra_dirs`].

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    Rust,
    Msvc,
    Git,
    Agent,
}

impl Tool {
    pub const ALL: [Tool; 4] = [Tool::Rust, Tool::Msvc, Tool::Git, Tool::Agent];

    /// Needed to clone and build. The agent CLI is only needed later, to modify the app.
    pub fn required(self) -> bool {
        self != Tool::Agent
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Tool::Rust => "selfbuild.tool.rust",
            Tool::Msvc => "selfbuild.tool.msvc",
            Tool::Git => "selfbuild.tool.git",
            Tool::Agent => "selfbuild.tool.agent",
        }
    }

    pub fn winget_id(self) -> &'static str {
        match self {
            Tool::Rust => "Rustlang.Rustup",
            Tool::Msvc => "Microsoft.VisualStudio.2022.BuildTools",
            Tool::Git => "Git.Git",
            Tool::Agent => "Anthropic.ClaudeCode",
        }
    }

    /// Arguments for `winget`. Build Tools need the C++ workload or `link.exe` is not installed.
    pub fn install_args(self) -> Vec<String> {
        let mut args: Vec<String> = [
            "install",
            "--id",
            self.winget_id(),
            "-e",
            "--accept-source-agreements",
            "--accept-package-agreements",
        ]
        .map(String::from)
        .into();
        if self == Tool::Msvc {
            args.push("--override".into());
            args.push(
                "--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
                    .into(),
            );
        }
        args
    }

    /// The command line shown to the user before it runs.
    pub fn install_command(self) -> String {
        let args = self.install_args();
        let quoted: Vec<String> = args
            .iter()
            .map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.clone() })
            .collect();
        format!("winget {}", quoted.join(" "))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Found; the detail is a version or path for display.
    Found(String),
    Missing,
}

impl State {
    pub fn found(&self) -> bool {
        matches!(self, State::Found(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub tool: Tool,
    pub state: State,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Checklist {
    pub items: Vec<Item>,
}

impl Checklist {
    pub fn state(&self, tool: Tool) -> Option<&State> {
        self.items.iter().find(|i| i.tool == tool).map(|i| &i.state)
    }

    /// Everything required to clone and build is present.
    pub fn ready(&self) -> bool {
        !self.items.is_empty()
            && self
                .items
                .iter()
                .filter(|i| i.tool.required())
                .all(|i| i.state.found())
    }
}

// ---- pure parsing ----------------------------------------------------------------------------

/// `vswhere -property installationPath` prints one path per matching install; any line means the
/// C++ tools are there.
pub fn parse_vswhere(output: &str) -> Option<String> {
    output.lines().map(str::trim).find(|l| !l.is_empty()).map(String::from)
}

/// First non-empty line of a `--version` output, e.g. `cargo 1.85.0 (…)`.
pub fn parse_version(output: &str) -> Option<String> {
    parse_vswhere(output)
}

/// Agent CLIs we recognise, in order of preference. The configured command comes first.
pub fn agent_candidates(configured: &str) -> Vec<String> {
    let mut v = vec![configured.trim().to_string()];
    for c in ["claude", "codex", "gemini"] {
        if !v.iter().any(|x| x == c) {
            v.push(c.to_string());
        }
    }
    v.retain(|c| !c.is_empty());
    v
}

// ---- PATH handling ---------------------------------------------------------------------------

/// Where winget-installed tools land, which a running app may not have on its PATH yet.
pub fn extra_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".cargo").join("bin"));
        dirs.push(home.join(".local").join("bin"));
    }
    if let Some(local) = dirs::data_local_dir() {
        dirs.push(local.join("Microsoft").join("WinGet").join("Links"));
    }
    for var in ["ProgramFiles", "ProgramW6432"] {
        if let Some(pf) = std::env::var_os(var) {
            let pf = PathBuf::from(pf);
            dirs.push(pf.join("Git").join("cmd"));
            dirs.push(pf.join("Git").join("bin"));
        }
    }
    dirs
}

/// The current PATH with [`extra_dirs`] appended (existing entries keep priority).
pub fn augmented_path() -> OsString {
    let mut all: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for d in extra_dirs() {
        if !all.contains(&d) {
            all.push(d);
        }
    }
    std::env::join_paths(all).unwrap_or_default()
}

/// Find `name` (`.exe`/`.cmd`/`.bat` on Windows) in the augmented PATH.
pub fn which(name: &str) -> Option<PathBuf> {
    which_in(name, &augmented_path())
}

pub fn which_in(name: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    let direct = Path::new(name);
    if direct.components().count() > 1 {
        return direct.is_file().then(|| direct.to_path_buf());
    }
    let exts: &[&str] = if cfg!(windows) { &["exe", "cmd", "bat"] } else { &[""] };
    for dir in std::env::split_paths(path) {
        for ext in exts {
            let mut candidate = dir.join(name);
            if !ext.is_empty() {
                candidate.set_extension(ext);
            }
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

// ---- probes ----------------------------------------------------------------------------------

fn vswhere_path() -> Option<PathBuf> {
    let base = std::env::var_os("ProgramFiles(x86)").or_else(|| std::env::var_os("ProgramFiles"))?;
    let p = PathBuf::from(base)
        .join("Microsoft Visual Studio")
        .join("Installer")
        .join("vswhere.exe");
    p.is_file().then_some(p)
}

async fn run_capture(exe: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env("PATH", augmented_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let out = cmd.output().await.ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn probe_version(name: &str) -> State {
    let Some(exe) = which(name) else { return State::Missing };
    match run_capture(&exe, &["--version"]).await.as_deref().and_then(parse_version) {
        Some(v) => State::Found(v),
        None => State::Missing,
    }
}

async fn probe_msvc() -> State {
    let Some(vswhere) = vswhere_path() else { return State::Missing };
    let args = [
        "-latest",
        "-products",
        "*",
        "-requires",
        "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
        "-property",
        "installationPath",
    ];
    match run_capture(&vswhere, &args).await.as_deref().and_then(parse_vswhere) {
        Some(path) => State::Found(path),
        None => State::Missing,
    }
}

async fn probe_agent(configured: &str) -> State {
    for name in agent_candidates(configured) {
        if let Some(path) = which(&name) {
            return State::Found(path.display().to_string());
        }
    }
    State::Missing
}

/// Probe every tool. `agent_command` is the configured agent program (empty for the default).
pub async fn check(agent_command: &str) -> Checklist {
    let (rust, msvc, git, agent) = tokio::join!(
        probe_version("cargo"),
        probe_msvc(),
        probe_version("git"),
        probe_agent(agent_command),
    );
    Checklist {
        items: vec![
            Item { tool: Tool::Rust, state: rust },
            Item { tool: Tool::Msvc, state: msvc },
            Item { tool: Tool::Git, state: git },
            Item { tool: Tool::Agent, state: agent },
        ],
    }
}

/// Whether `winget` itself is available (it ships with current Windows 10/11).
pub fn winget_available() -> bool {
    which("winget").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vswhere_empty_output_is_missing() {
        assert_eq!(parse_vswhere(""), None);
        assert_eq!(parse_vswhere("\r\n  \n"), None);
    }

    #[test]
    fn vswhere_takes_first_path() {
        let out = "C:\\Program Files (x86)\\Microsoft Visual Studio\\2022\\BuildTools\r\nD:\\other\r\n";
        assert_eq!(
            parse_vswhere(out).as_deref(),
            Some("C:\\Program Files (x86)\\Microsoft Visual Studio\\2022\\BuildTools")
        );
    }

    #[test]
    fn version_takes_first_line() {
        assert_eq!(
            parse_version("cargo 1.85.0 (d73d2caf9 2024-12-31)\n").as_deref(),
            Some("cargo 1.85.0 (d73d2caf9 2024-12-31)")
        );
        assert_eq!(parse_version("  \n"), None);
    }

    #[test]
    fn agent_candidates_prefer_configured_and_dedupe() {
        assert_eq!(agent_candidates(""), ["claude", "codex", "gemini"]);
        assert_eq!(agent_candidates(" aider "), ["aider", "claude", "codex", "gemini"]);
        assert_eq!(agent_candidates("claude"), ["claude", "codex", "gemini"]);
    }

    #[test]
    fn install_commands_show_the_winget_id() {
        assert!(Tool::Git.install_command().starts_with("winget install --id Git.Git"));
        let msvc = Tool::Msvc.install_command();
        assert!(msvc.contains("Microsoft.VisualStudio.2022.BuildTools"));
        assert!(msvc.contains("--override \"--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools"));
    }

    fn list(rust: bool, msvc: bool, git: bool, agent: bool) -> Checklist {
        let s = |b: bool| if b { State::Found("x".into()) } else { State::Missing };
        Checklist {
            items: vec![
                Item { tool: Tool::Rust, state: s(rust) },
                Item { tool: Tool::Msvc, state: s(msvc) },
                Item { tool: Tool::Git, state: s(git) },
                Item { tool: Tool::Agent, state: s(agent) },
            ],
        }
    }

    #[test]
    fn ready_ignores_the_agent_but_not_the_build_tools() {
        assert!(list(true, true, true, false).ready());
        assert!(!list(true, false, true, true).ready());
        assert!(!list(false, true, true, true).ready());
        assert!(!Checklist::default().ready());
    }

    #[test]
    fn which_in_finds_files_in_given_dirs() {
        let dir = std::env::temp_dir().join(format!("agentory-which-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let name = if cfg!(windows) { "faketool.exe" } else { "faketool" };
        std::fs::write(dir.join(name), "").unwrap();
        let path = std::env::join_paths([&dir]).unwrap();
        assert!(which_in("faketool", &path).is_some());
        assert!(which_in("nothere", &path).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
