use std::path::Path;

use serde::Deserialize;

use super::capture;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setup {
    NotInstalled,
    NotAuthenticated,
    NoRemote,
    Ready,
}

pub async fn setup(dir: &Path) -> Setup {
    if capture("gh", &["--version"], dir).await.is_err() {
        return Setup::NotInstalled;
    }
    if capture("gh", &["auth", "status"], dir).await.is_err() {
        return Setup::NotAuthenticated;
    }
    match capture("gh", &["repo", "view", "--json", "nameWithOwner"], dir).await {
        Ok(_) => Setup::Ready,
        Err(_) => Setup::NoRemote,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Author {
    #[serde(default)]
    pub login: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    #[serde(default)]
    pub author: Author,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrSummary {
    pub number: u64,
    pub title: String,
    pub state: String,
    #[serde(default)]
    pub author: Author,
    #[serde(default)]
    pub head_ref_name: String,
    #[serde(default)]
    pub is_draft: bool,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub conclusion: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub details_url: String,
    #[serde(default)]
    pub target_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Passed,
    Failed,
    Pending,
    Skipped,
}

impl Check {
    pub fn title(&self) -> &str {
        if self.name.is_empty() {
            &self.context
        } else {
            &self.name
        }
    }

    pub fn url(&self) -> &str {
        if self.details_url.is_empty() {
            &self.target_url
        } else {
            &self.details_url
        }
    }

    pub fn result(&self) -> CheckState {
        let word = if !self.state.is_empty() {
            self.state.as_str()
        } else if self.status == "COMPLETED" {
            self.conclusion.as_str()
        } else {
            return CheckState::Pending;
        };
        match word {
            "SUCCESS" => CheckState::Passed,
            "NEUTRAL" | "SKIPPED" => CheckState::Skipped,
            "PENDING" | "EXPECTED" | "" => CheckState::Pending,
            _ => CheckState::Failed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrDetail {
    pub number: u64,
    pub title: String,
    pub state: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub author: Author,
    #[serde(default)]
    pub head_ref_name: String,
    #[serde(default)]
    pub base_ref_name: String,
    #[serde(default)]
    pub is_draft: bool,
    #[serde(default)]
    pub is_cross_repository: bool,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub comments: Vec<Comment>,
    #[serde(default)]
    pub status_check_rollup: Vec<Check>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueSummary {
    pub number: u64,
    pub title: String,
    pub state: String,
    #[serde(default)]
    pub author: Author,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueDetail {
    pub number: u64,
    pub title: String,
    pub state: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub author: Author,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub comments: Vec<Comment>,
}

fn parse<T: for<'de> Deserialize<'de>>(json: &str) -> Result<T, String> {
    serde_json::from_str(json).map_err(|e| format!("unexpected gh output: {e}"))
}

pub async fn pr_list(dir: &Path) -> Result<Vec<PrSummary>, String> {
    let out = capture(
        "gh",
        &[
            "pr",
            "list",
            "--state",
            "all",
            "--limit",
            "50",
            "--json",
            "number,title,state,author,headRefName,isDraft,updatedAt",
        ],
        dir,
    )
    .await?;
    parse(&out)
}

pub async fn pr_view(dir: &Path, number: u64) -> Result<PrDetail, String> {
    let n = number.to_string();
    let out = capture(
        "gh",
        &[
            "pr",
            "view",
            &n,
            "--json",
            "number,title,state,body,author,headRefName,baseRefName,isDraft,isCrossRepository,url,comments,statusCheckRollup",
        ],
        dir,
    )
    .await?;
    parse(&out)
}

pub async fn pr_diff(dir: &Path, number: u64) -> Result<String, String> {
    capture(
        "gh",
        &["pr", "diff", &number.to_string(), "--color", "never"],
        dir,
    )
    .await
}

pub async fn issue_list(dir: &Path) -> Result<Vec<IssueSummary>, String> {
    let out = capture(
        "gh",
        &[
            "issue",
            "list",
            "--state",
            "all",
            "--limit",
            "50",
            "--json",
            "number,title,state,author,updatedAt",
        ],
        dir,
    )
    .await?;
    parse(&out)
}

pub async fn issue_view(dir: &Path, number: u64) -> Result<IssueDetail, String> {
    let n = number.to_string();
    let out = capture(
        "gh",
        &[
            "issue",
            "view",
            &n,
            "--json",
            "number,title,state,body,author,url,comments",
        ],
        dir,
    )
    .await?;
    parse(&out)
}

async fn run(dir: &Path, args: &[&str]) -> Result<(), String> {
    capture("gh", args, dir).await.map(|_| ())
}

pub async fn pr_create(dir: &Path, title: &str, body: &str) -> Result<(), String> {
    run(dir, &["pr", "create", "--title", title, "--body", body]).await
}

pub async fn pr_merge(dir: &Path, number: u64) -> Result<(), String> {
    run(dir, &["pr", "merge", &number.to_string(), "--merge"]).await
}

pub async fn pr_close(dir: &Path, number: u64) -> Result<(), String> {
    run(dir, &["pr", "close", &number.to_string()]).await
}

pub async fn pr_reopen(dir: &Path, number: u64) -> Result<(), String> {
    run(dir, &["pr", "reopen", &number.to_string()]).await
}

pub async fn branch_deletable(dir: &Path, pr: &PrDetail) -> bool {
    let head = pr.head_ref_name.as_str();
    if pr.state != "MERGED" || pr.is_cross_repository || head.is_empty() || head == pr.base_ref_name
    {
        return false;
    }
    let default = capture(
        "gh",
        &[
            "repo",
            "view",
            "--json",
            "defaultBranchRef",
            "--jq",
            ".defaultBranchRef.name",
        ],
        dir,
    )
    .await;
    match default {
        Ok(name) if name.trim() != head => {}
        _ => return false,
    }
    if capture(
        "gh",
        &["api", &format!("repos/{{owner}}/{{repo}}/branches/{head}")],
        dir,
    )
    .await
    .is_err()
    {
        return false;
    }
    match capture(
        "gh",
        &[
            "pr", "list", "--state", "open", "--base", head, "--json", "number",
        ],
        dir,
    )
    .await
    {
        Ok(out) => parse::<Vec<serde_json::Value>>(&out).is_ok_and(|v| v.is_empty()),
        Err(_) => false,
    }
}

pub async fn delete_branch(dir: &Path, branch: &str) -> Result<(), String> {
    run(
        dir,
        &[
            "api",
            "-X",
            "DELETE",
            &format!("repos/{{owner}}/{{repo}}/git/refs/heads/{branch}"),
        ],
    )
    .await
}

pub async fn pr_checkout(dir: &Path, number: u64) -> Result<(), String> {
    run(dir, &["pr", "checkout", &number.to_string()]).await
}

pub async fn pr_comment(dir: &Path, number: u64, body: &str) -> Result<(), String> {
    run(dir, &["pr", "comment", &number.to_string(), "--body", body]).await
}

pub async fn issue_create(dir: &Path, title: &str, body: &str) -> Result<(), String> {
    run(dir, &["issue", "create", "--title", title, "--body", body]).await
}

pub async fn issue_comment(dir: &Path, number: u64, body: &str) -> Result<(), String> {
    run(
        dir,
        &["issue", "comment", &number.to_string(), "--body", body],
    )
    .await
}

pub async fn issue_close(dir: &Path, number: u64) -> Result<(), String> {
    run(dir, &["issue", "close", &number.to_string()]).await
}

pub async fn issue_reopen(dir: &Path, number: u64) -> Result<(), String> {
    run(dir, &["issue", "reopen", &number.to_string()]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_pr_with_both_check_kinds() {
        let json = r#"{"number":7,"title":"T","state":"OPEN","body":"b","author":{"login":"a"},
            "headRefName":"feat","baseRefName":"main","isDraft":false,"url":"u","comments":[],
            "statusCheckRollup":[
              {"__typename":"CheckRun","name":"build","status":"COMPLETED","conclusion":"FAILURE","detailsUrl":"d"},
              {"__typename":"CheckRun","name":"lint","status":"IN_PROGRESS","conclusion":"","detailsUrl":""},
              {"__typename":"StatusContext","context":"ci/x","state":"SUCCESS","targetUrl":"t"}]}"#;
        let pr: PrDetail = parse(json).unwrap();
        let states: Vec<_> = pr.status_check_rollup.iter().map(Check::result).collect();
        assert_eq!(
            states,
            [CheckState::Failed, CheckState::Pending, CheckState::Passed]
        );
        assert_eq!(pr.status_check_rollup[2].title(), "ci/x");
        assert_eq!(pr.status_check_rollup[2].url(), "t");
    }

    #[test]
    fn parses_lists_with_missing_optional_fields() {
        let prs: Vec<PrSummary> = parse(r#"[{"number":1,"title":"x","state":"MERGED"}]"#).unwrap();
        assert_eq!(prs[0].state, "MERGED");
        let issues: Vec<IssueSummary> =
            parse(r#"[{"number":2,"title":"y","state":"OPEN","author":{"login":"z"}}]"#).unwrap();
        assert_eq!(issues[0].author.login, "z");
    }
}
