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
