//! Plain async functions over the outside world (`git`, `spm`). Call them only through
//! `runtime::run`.

pub mod git;
pub mod spm;

use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

/// Run `program args…` in `dir` and return its stdout, or the stderr/exit status as the error.
/// Windows `.cmd` shims (`spm`) are reached through `cmd /c`.
pub(crate) async fn capture(program: &str, args: &[&str], dir: &Path) -> Result<String, String> {
    let mut cmd = if cfg!(windows) && program != "git" {
        let mut c = Command::new("cmd");
        c.arg("/c").arg(program);
        c
    } else {
        Command::new(program)
    };
    cmd.args(args)
        .current_dir(dir)
        // Tools installed while the app runs (winget) are not on its PATH yet.
        .env("PATH", crate::selfbuild::preflight::augmented_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let out = cmd
        .output()
        .await
        .map_err(|e| format!("could not run {program}: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let msg = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        Err(if msg.is_empty() {
            format!("{program} exited with {}", out.status)
        } else {
            msg
        })
    }
}

/// Run `git args…` in `dir` with a throwaway identity, panicking on failure. For test fixtures.
#[cfg(test)]
pub(crate) async fn sh(dir: &Path, args: &[&str]) {
    let mut a = vec!["-c", "user.name=t", "-c", "user.email=t@t"];
    a.extend_from_slice(args);
    capture("git", &a, dir).await.unwrap();
}
