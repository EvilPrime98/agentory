
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::Notify;

pub const STEP_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub name: String,
    pub prompt: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub skills: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepStatus {
    #[default]
    Pending,
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepRun {
    pub status: StepStatus,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub prompt: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub workflow_id: String,
    pub workflow_name: String,
    pub input: String,
    pub started: u64,
    pub steps: Vec<Step>,
    pub results: Vec<StepRun>,
}

impl Run {
    pub fn new(id: String, workflow: &Workflow, input: String) -> Self {
        Self {
            id,
            workflow_id: workflow.id.clone(),
            workflow_name: workflow.name.clone(),
            input,
            started: now_secs(),
            steps: workflow.steps.clone(),
            results: vec![StepRun::default(); workflow.steps.len()],
        }
    }

    pub fn resume_point(&self) -> Option<usize> {
        self.results
            .iter()
            .position(|r| r.status != StepStatus::Done)
    }

    pub fn finished(&self) -> bool {
        self.resume_point().is_none()
    }

    pub fn running(&self) -> bool {
        self.results.iter().any(|r| r.status == StepStatus::Running)
    }

    fn outputs_before(&self, index: usize) -> Vec<&str> {
        self.results[..index.min(self.results.len())]
            .iter()
            .map(|r| {
                if r.status == StepStatus::Done {
                    r.output.as_str()
                } else {
                    ""
                }
            })
            .collect()
    }

    pub fn prompt_for(&self, index: usize) -> String {
        let step = &self.steps[index];
        let outputs = self.outputs_before(index);
        let body = render(&step.prompt, &self.input, &outputs);
        if step.skills.is_empty() {
            body
        } else {
            format!("Use these skills: {}.\n\n{body}", step.skills.join(", "))
        }
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn render(template: &str, input: &str, outputs: &[&str]) -> String {
    let mut vars: HashMap<String, &str> = HashMap::new();
    vars.insert("input".into(), input);
    vars.insert("prev".into(), outputs.last().copied().unwrap_or(""));
    for (i, o) in outputs.iter().enumerate() {
        vars.insert(format!("step{}", i + 1), o);
    }
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let key = after[..end].trim();
                let is_step = key
                    .strip_prefix("step")
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
                match vars.get(key) {
                    Some(v) => out.push_str(v),
                    None if key == "prev" || is_step => {}
                    None => {
                        out.push_str("{{");
                        out.push_str(&after[..end]);
                        out.push_str("}}");
                    }
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn definitions_dir(project: &Path) -> PathBuf {
    project.join(".agentory").join("workflows")
}

fn runs_dir(project_id: &str) -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("Agentory")
            .join("runs")
            .join(project_id),
    )
}

pub fn load_workflows(project: &Path) -> Vec<Workflow> {
    let Ok(rd) = std::fs::read_dir(definitions_dir(project)) else {
        return Vec::new();
    };
    let mut out: Vec<Workflow> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|s| serde_json::from_str(&s).ok())
        .collect();
    out.sort_by(|a, b: &Workflow| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn safe_file_name(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn save_workflow(project: &Path, wf: &Workflow) -> std::io::Result<()> {
    if !safe_file_name(&wf.id) {
        return Err(std::io::Error::other("invalid workflow id"));
    }
    let dir = definitions_dir(project);
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_string_pretty(wf).map_err(std::io::Error::other)?;
    std::fs::write(dir.join(format!("{}.json", wf.id)), json)
}

pub fn delete_workflow(project: &Path, id: &str) -> std::io::Result<()> {
    if !safe_file_name(id) {
        return Err(std::io::Error::other("invalid workflow id"));
    }
    std::fs::remove_file(definitions_dir(project).join(format!("{id}.json")))
}

pub fn load_runs(project_id: &str) -> Vec<Run> {
    let Some(dir) = runs_dir(project_id) else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Run> = rd
        .filter_map(Result::ok)
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|s| serde_json::from_str(&s).ok())
        .collect();
    for run in &mut out {
        for r in &mut run.results {
            if r.status == StepStatus::Running {
                r.status = StepStatus::Cancelled;
            }
        }
    }
    out.sort_by(|a, b| b.started.cmp(&a.started));
    out
}

pub fn save_run(project_id: &str, run: &Run) {
    let Some(dir) = runs_dir(project_id) else {
        return;
    };
    if !safe_file_name(&run.id) {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_string_pretty(run) {
        if let Err(e) = std::fs::write(dir.join(format!("{}.json", run.id)), json) {
            tracing::warn!("could not save run {}: {e}", run.id);
        }
    }
}

pub fn delete_run(project_id: &str, run_id: &str) {
    if let Some(dir) = runs_dir(project_id) {
        if safe_file_name(run_id) {
            let _ = std::fs::remove_file(dir.join(format!("{run_id}.json")));
        }
    }
}

pub enum StepOutcome {
    Done(String),
    Failed(String),
    Cancelled,
}

fn is_script(program: &Path) -> bool {
    program
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
}

pub async fn run_step(
    program: &str,
    dir: &Path,
    model: &str,
    prompt: &str,
    cancel: Arc<Notify>,
) -> StepOutcome {
    let resolved =
        crate::platform::pty::resolve_program(program).unwrap_or_else(|| PathBuf::from(program));
    let mut cmd = if is_script(&resolved) {
        let mut c = Command::new("cmd.exe");
        c.arg("/c").arg(&resolved);
        c
    } else {
        Command::new(&resolved)
    };
    cmd.arg("-p");
    if !model.trim().is_empty() {
        cmd.arg("--model").arg(model.trim());
    }
    cmd.current_dir(dir)
        .env("PATH", crate::selfbuild::preflight::augmented_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return StepOutcome::Failed(format!("could not run {program}: {e}")),
    };
    if let Some(mut stdin) = child.stdin.take() {
        let bytes = prompt.as_bytes().to_vec();
        tokio::spawn(async move {
            let _ = stdin.write_all(&bytes).await;
            let _ = stdin.shutdown().await;
        });
    }
    let wait = child.wait_with_output();
    tokio::select! {
        _ = cancel.notified() => StepOutcome::Cancelled,
        res = tokio::time::timeout(STEP_TIMEOUT, wait) => match res {
            Err(_) => StepOutcome::Failed(format!(
                "timed out after {} minutes",
                STEP_TIMEOUT.as_secs() / 60
            )),
            Ok(Err(e)) => StepOutcome::Failed(e.to_string()),
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if out.status.success() {
                    StepOutcome::Done(stdout)
                } else {
                    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    let msg = [stderr, stdout]
                        .into_iter()
                        .find(|s| !s.is_empty())
                        .unwrap_or_else(|| format!("exited with {}", out.status));
                    StepOutcome::Failed(msg)
                }
            }
        },
    }
}

pub fn templates() -> Vec<(&'static str, Vec<Step>)> {
    let step = |name: &str, prompt: &str| Step {
        name: name.into(),
        prompt: prompt.into(),
        ..Step::default()
    };
    vec![
        (
            "Plan, implement, review",
            vec![
                step(
                    "Plan",
                    "Write a concise implementation plan for this task. Do not change any files.\n\nTask: {{input}}",
                ),
                step(
                    "Implement",
                    "Implement this plan in the working tree.\n\nTask: {{input}}\n\nPlan:\n{{step1}}",
                ),
                step(
                    "Review",
                    "Review the uncommitted changes in the working tree against the plan, fix any problems you find and summarise what you did.\n\nPlan:\n{{step1}}\n\nImplementation notes:\n{{prev}}",
                ),
            ],
        ),
        (
            "Fix a bug",
            vec![
                step(
                    "Diagnose",
                    "Find the root cause of this bug without changing files and explain it.\n\nBug: {{input}}",
                ),
                step(
                    "Fix",
                    "Fix the bug and add a regression test.\n\nBug: {{input}}\n\nDiagnosis:\n{{prev}}",
                ),
            ],
        ),
        (
            "Review changes",
            vec![
                step(
                    "Summarise",
                    "Summarise the uncommitted changes in this repository. Focus: {{input}}",
                ),
                step(
                    "Critique",
                    "Review these changes for bugs, missing tests and risky edge cases.\n\n{{prev}}",
                ),
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_substitutes_variables() {
        let out = render(
            "a {{input}} b {{prev}} c {{step1}} d {{ step2 }}",
            "IN",
            &["one", "two"],
        );
        assert_eq!(out, "a IN b two c one d two");
    }

    #[test]
    fn render_keeps_unknown_and_empties_missing_steps() {
        assert_eq!(render("{{foo}} {{prev}} {{step3}}", "", &[]), "{{foo}}  ");
        assert_eq!(render("open {{ x", "", &[]), "open {{ x");
    }

    #[test]
    fn run_resumes_from_first_unfinished_step() {
        let wf = Workflow {
            id: "w".into(),
            name: "w".into(),
            steps: vec![Step::default(), Step::default()],
        };
        let mut run = Run::new("r".into(), &wf, "x".into());
        assert_eq!(run.resume_point(), Some(0));
        run.results[0].status = StepStatus::Done;
        assert_eq!(run.resume_point(), Some(1));
        run.results[1].status = StepStatus::Done;
        assert!(run.finished());
    }

    #[test]
    fn prompt_prepends_skills_and_uses_previous_output() {
        let wf = Workflow {
            id: "w".into(),
            name: "w".into(),
            steps: vec![
                Step {
                    prompt: "p".into(),
                    ..Step::default()
                },
                Step {
                    prompt: "got {{prev}}".into(),
                    skills: vec!["s".into()],
                    ..Step::default()
                },
            ],
        };
        let mut run = Run::new("r".into(), &wf, String::new());
        run.results[0] = StepRun {
            status: StepStatus::Done,
            output: "OUT".into(),
            prompt: String::new(),
        };
        assert_eq!(run.prompt_for(1), "Use these skills: s.\n\ngot OUT");
    }

    #[test]
    fn templates_are_nonempty() {
        assert!(templates().iter().all(|(_, s)| !s.is_empty()));
    }
}
