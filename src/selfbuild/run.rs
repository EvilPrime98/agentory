//! Pipeline plumbing: stages, the log channel, the result type and a streaming process runner.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Bringing the worktree up to the branch tip. Emitted by the caller (apply flow), not here.
    Merge,
    Build,
    Gate,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Stage(Stage),
    Line(String),
}

pub type Events = UnboundedSender<Event>;

pub fn channel() -> (Events, UnboundedReceiver<Event>) {
    unbounded_channel()
}

/// Final result of the pipeline; every variant carries the full log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Passed(String),
    TestsFailed(String),
    SmokeFailed(String),
    BuildFailed(String),
}

impl Outcome {
    pub fn log(&self) -> &str {
        match self {
            Outcome::Passed(l)
            | Outcome::TestsFailed(l)
            | Outcome::SmokeFailed(l)
            | Outcome::BuildFailed(l) => l,
        }
    }
    pub fn passed(&self) -> bool {
        matches!(self, Outcome::Passed(_))
    }
}

/// Collects the log text while forwarding every line and stage to the listener. A dropped
/// receiver is fine: the log is still collected.
pub struct Log {
    tx: Events,
    text: String,
}

impl Log {
    pub fn new(tx: Events) -> Log {
        Log { tx, text: String::new() }
    }
    pub fn stage(&mut self, stage: Stage) {
        let _ = self.tx.send(Event::Stage(stage));
    }
    pub fn line(&mut self, line: impl Into<String>) {
        let line = line.into();
        self.text.push_str(&line);
        self.text.push('\n');
        let _ = self.tx.send(Event::Line(line));
    }
    pub fn into_text(self) -> String {
        self.text
    }
}

pub struct Exit {
    pub status: ExitStatus,
    pub pid: Option<u32>,
}

async fn pump(reader: impl AsyncRead + Unpin, tx: UnboundedSender<String>) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if tx.send(line).is_err() {
            break;
        }
    }
}

/// Run `program args…` in `dir`, logging stdout and stderr line by line. `Err` means it could not
/// start or exceeded `timeout` (the process is killed); a non-zero exit is an `Ok` for the caller
/// to judge.
pub async fn stream(
    program: &str,
    args: &[&str],
    dir: &Path,
    envs: &[(&str, &OsStr)],
    timeout: Duration,
    log: &mut Log,
) -> Result<Exit, String> {
    log.line(format!("$ {program} {}", args.join(" ")));
    let mut cmd = Command::new(program);
    // Tools installed while the app runs (winget) are not on its PATH yet.
    cmd.args(args)
        .current_dir(dir)
        .env("PATH", super::preflight::augmented_path())
        .envs(envs.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not run {program}: {e}"))?;
    let pid = child.id();

    let (ltx, mut lrx) = unbounded_channel();
    if let Some(out) = child.stdout.take() {
        tokio::spawn(pump(out, ltx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(pump(err, ltx.clone()));
    }
    drop(ltx);

    let work = async {
        while let Some(line) = lrx.recv().await {
            log.line(line);
        }
        child.wait().await
    };
    match tokio::time::timeout(timeout, work).await {
        Ok(Ok(status)) => Ok(Exit { status, pid }),
        Ok(Err(e)) => Err(format!("waiting for {program} failed: {e}")),
        // `child` drops with the future and `kill_on_drop` ends it.
        Err(_) => Err(format!("{program} timed out after {}s", timeout.as_secs())),
    }
}
