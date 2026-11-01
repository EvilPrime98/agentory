//! Build runner: `cargo build --release` into the shared target dir, then the gate.

use std::path::PathBuf;
use std::time::Duration;

use super::gate;
use super::paths::{Paths, built_exe};
use super::run::{Events, Log, Outcome, Stage, stream};
use crate::backend::capture;

/// One pipeline run over a source tree.
#[derive(Debug, Clone)]
pub struct Job {
    pub src: PathBuf,
    pub target_dir: PathBuf,
    /// Cargo binary name of the app.
    pub bin: String,
    /// Build id the exe must report in the ready marker.
    pub build_id: String,
    pub smoke_secs: u64,
}

impl Job {
    /// A job for the app source in `src`, using the shared target dir and `src`'s git sha.
    pub async fn for_tree(paths: &Paths, src: PathBuf) -> Result<Job, String> {
        let sha = capture("git", &["rev-parse", "--short=12", "HEAD"], &src).await?;
        Ok(Job {
            src,
            target_dir: paths.target_dir(),
            bin: env!("CARGO_PKG_NAME").to_string(),
            build_id: sha.trim().to_string(),
            smoke_secs: super::SMOKE_SECS,
        })
    }

    pub fn exe(&self) -> PathBuf {
        built_exe(&self.target_dir, &self.bin)
    }
}

/// A cold GPUI build takes a long time; this only catches a hung process.
const BUILD_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Build, then gate. Stages go to `events` as they start; the log is also returned in the result.
pub async fn run(job: &Job, events: Events) -> Outcome {
    let mut log = Log::new(events);

    log.stage(Stage::Build);
    if let Err(e) = build(job, &mut log).await {
        log.line(e);
        return Outcome::BuildFailed(log.into_text());
    }

    log.stage(Stage::Gate);
    match gate::run(job, &mut log).await {
        Ok(()) => {
            log.stage(Stage::Ready);
            Outcome::Passed(log.into_text())
        }
        Err(gate::Failure::Tests(e)) => {
            log.line(e);
            Outcome::TestsFailed(log.into_text())
        }
        Err(gate::Failure::Smoke(e)) => {
            log.line(e);
            Outcome::SmokeFailed(log.into_text())
        }
    }
}

pub async fn build(job: &Job, log: &mut Log) -> Result<(), String> {
    let exit = stream(
        "cargo",
        &["build", "--release"],
        &job.src,
        &[("CARGO_TARGET_DIR", job.target_dir.as_os_str())],
        BUILD_TIMEOUT,
        log,
    )
    .await?;
    if !exit.status.success() {
        return Err(format!("cargo build failed ({})", exit.status));
    }
    if !job.exe().exists() {
        return Err(format!("build produced no {}", job.exe().display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selfbuild::run::channel;
    use std::fs;
    use std::path::Path;

    const ID: &str = "fixture-id";
    const GOOD_MAIN: &str = r#"
        fn main() {
            if let Ok(p) = std::env::var("AGENTORY_READY_FILE") {
                std::fs::write(p, format!("{}\n{}\n", std::process::id(), "fixture-id")).unwrap();
            }
        }
        #[cfg(test)] mod t { #[test] fn ok() {} }
    "#;

    /// A tiny crate standing in for the app, in its own dirs so the outer cargo lock is not shared.
    fn fixture(name: &str, main: &str) -> Job {
        let root =
            std::env::temp_dir().join(format!("agentory-fixture-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n[workspace]\n",
        )
        .unwrap();
        fs::write(src.join("main.rs"), main).unwrap();
        Job {
            src: root.clone(),
            target_dir: root.join("target"),
            bin: "fixture".into(),
            build_id: ID.into(),
            smoke_secs: 0,
        }
    }

    async fn go(job: &Job) -> Outcome {
        let (tx, _rx) = channel();
        let out = run(job, tx).await;
        let _ = fs::remove_dir_all(Path::new(&job.src));
        out
    }

    #[tokio::test]
    async fn good_tree_passes_and_reports_stages() {
        let job = fixture("good", GOOD_MAIN);
        let (tx, mut rx) = channel();
        let out = run(&job, tx).await;
        let mut stages = vec![];
        while let Ok(ev) = rx.try_recv() {
            if let crate::selfbuild::run::Event::Stage(s) = ev {
                stages.push(s);
            }
        }
        let _ = fs::remove_dir_all(&job.src);
        assert!(out.passed(), "{}", out.log());
        assert_eq!(stages, [Stage::Build, Stage::Gate, Stage::Ready]);
    }

    #[tokio::test]
    async fn compile_error_is_build_failed() {
        let out = go(&fixture("broken", "fn main() { let x: u32 = \"no\"; }")).await;
        assert!(matches!(out, Outcome::BuildFailed(_)), "{out:?}");
        assert!(out.log().contains("error"));
    }

    #[tokio::test]
    async fn failing_test_is_tests_failed() {
        let main = "fn main() {}\n#[cfg(test)] mod t { #[test] fn bad() { panic!(\"boom\"); } }";
        let out = go(&fixture("badtest", main)).await;
        assert!(matches!(out, Outcome::TestsFailed(_)), "{out:?}");
        assert!(out.log().contains("boom"));
    }

    #[tokio::test]
    async fn missing_marker_is_smoke_failed() {
        let out = go(&fixture("nomarker", "fn main() {}")).await;
        assert!(matches!(out, Outcome::SmokeFailed(_)), "{out:?}");
    }

    #[tokio::test]
    async fn wrong_build_id_is_smoke_failed() {
        let main = GOOD_MAIN.replace("fixture-id", "someone-else");
        let out = go(&fixture("wrongid", &main)).await;
        assert!(matches!(out, Outcome::SmokeFailed(_)), "{out:?}");
        assert!(out.log().contains("someone-else"));
    }

    #[tokio::test]
    async fn crash_is_smoke_failed() {
        let out = go(&fixture("crash", "fn main() { std::process::exit(3); }")).await;
        assert!(matches!(out, Outcome::SmokeFailed(_)), "{out:?}");
    }

    /// Slow (release build of the real app into the shared target dir) and runs `cargo test`
    /// inside `cargo test`: `cargo test -- --ignored pipeline_on_current_repo --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn pipeline_on_current_repo() {
        let paths = Paths::system().unwrap();
        let src = std::env::current_dir().unwrap();
        let job = Job::for_tree(&paths, src).await.unwrap();
        let (tx, mut rx) = channel();
        let printer = tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                println!("{ev:?}");
            }
        });
        let out = run(&job, tx).await;
        printer.await.unwrap();
        assert!(
            out.passed(),
            "{:?}",
            &out.log()[out.log().len().saturating_sub(3000)..]
        );
    }
}
