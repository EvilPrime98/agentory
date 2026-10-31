//! The gate: `cargo test`, then a smoke launch of the built exe.

use std::time::Duration;

use super::build::Job;
use super::ready;
use super::run::{Log, stream};

#[derive(Debug)]
pub enum Failure {
    Tests(String),
    Smoke(String),
}

const TEST_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Slack on top of the smoke seconds for startup and shutdown.
const SMOKE_SLACK: Duration = Duration::from_secs(60);

pub async fn run(job: &Job, log: &mut Log) -> Result<(), Failure> {
    tests(job, log).await.map_err(Failure::Tests)?;
    smoke(job, log).await.map_err(Failure::Smoke)
}

async fn tests(job: &Job, log: &mut Log) -> Result<(), String> {
    let exit = stream(
        "cargo",
        &["test"],
        &job.src,
        &[("CARGO_TARGET_DIR", job.target_dir.as_os_str())],
        TEST_TIMEOUT,
        log,
    )
    .await?;
    if exit.status.success() {
        Ok(())
    } else {
        Err(format!("cargo test failed ({})", exit.status))
    }
}

/// Launch the built exe with `--smoke <secs>`: it must exit cleanly and leave a ready marker with
/// its own pid and this job's build id. The marker goes to a private file, never the real one.
async fn smoke(job: &Job, log: &mut Log) -> Result<(), String> {
    let marker = job.target_dir.join("smoke-ready");
    let _ = std::fs::remove_file(&marker);

    let exe = job.exe();
    let secs = job.smoke_secs.to_string();
    let exit = stream(
        &exe.to_string_lossy(),
        &["--smoke", &secs],
        &job.src,
        &[(ready::READY_ENV, marker.as_os_str())],
        Duration::from_secs(job.smoke_secs) + SMOKE_SLACK,
        log,
    )
    .await?;
    if !exit.status.success() {
        return Err(format!("smoke launch exited with {}", exit.status));
    }
    match ready::read(&marker) {
        None => Err("smoke launch wrote no ready marker".into()),
        Some((pid, _)) if Some(pid) != exit.pid => Err(format!(
            "ready marker pid {pid} is not the launched process ({:?})",
            exit.pid
        )),
        Some((_, id)) if id != job.build_id => Err(format!(
            "ready marker has build id {id}, expected {}",
            job.build_id
        )),
        Some(_) => {
            log.line("smoke launch ok");
            Ok(())
        }
    }
}
