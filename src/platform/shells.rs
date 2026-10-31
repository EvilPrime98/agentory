//! Plain shells a project terminal can run, detected on this machine.

use std::path::PathBuf;

use crate::platform::pty::resolve_program;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shell {
    /// Stable key stored in the prefs.
    pub id: &'static str,
    pub label: &'static str,
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// Shells found on this machine, in menu order.
pub fn detect() -> Vec<Shell> {
    let mut found = Vec::new();
    let mut add = |id, label, program: Option<PathBuf>, args: &[&str]| {
        if let Some(program) = program {
            found.push(Shell {
                id,
                label,
                program,
                args: args.iter().map(|a| a.to_string()).collect(),
            });
        }
    };
    if cfg!(windows) {
        add("pwsh", "PowerShell", resolve_program("pwsh"), &[]);
        add(
            "powershell",
            "Windows PowerShell",
            resolve_program("powershell"),
            &[],
        );
        add("cmd", "Command Prompt", resolve_program("cmd"), &[]);
        add("git-bash", "Git Bash", git_bash(), &["--login", "-i"]);
        add("wsl", "WSL", resolve_program("wsl"), &[]);
    } else {
        let login = std::env::var_os("SHELL").map(PathBuf::from);
        add(
            "shell",
            "Shell",
            login.or_else(|| resolve_program("sh")),
            &[],
        );
    }
    found
}

/// Git for Windows' own `bash.exe`. `bash` on PATH is often the WSL launcher, so it is not used.
fn git_bash() -> Option<PathBuf> {
    [
        "ProgramFiles",
        "ProgramW6432",
        "ProgramFiles(x86)",
        "LocalAppData",
    ]
    .iter()
    .filter_map(std::env::var_os)
    .flat_map(|root| {
        let root = PathBuf::from(root);
        [
            root.join("Git").join("bin").join("bash.exe"),
            root.join("Programs")
                .join("Git")
                .join("bin")
                .join("bash.exe"),
        ]
    })
    .find(|p| p.is_file())
}

/// The remembered shell if it is still installed, else the first one found.
pub fn pick<'a>(shells: &'a [Shell], remembered: &str) -> Option<&'a Shell> {
    shells
        .iter()
        .find(|s| s.id == remembered)
        .or_else(|| shells.first())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(id: &'static str) -> Shell {
        Shell {
            id,
            label: id,
            program: PathBuf::from(id),
            args: Vec::new(),
        }
    }

    #[test]
    fn remembered_shell_wins_when_installed() {
        let shells = [shell("pwsh"), shell("cmd")];
        assert_eq!(pick(&shells, "cmd").unwrap().id, "cmd");
    }

    #[test]
    fn falls_back_to_the_first_shell() {
        let shells = [shell("pwsh"), shell("cmd")];
        assert_eq!(pick(&shells, "wsl").unwrap().id, "pwsh");
        assert!(pick(&[], "wsl").is_none());
    }
}
