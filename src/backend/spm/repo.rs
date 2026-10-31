//! Repo specs, the `~/.cache/spm` clones (through `git`) and the pool of skills they hold.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

use regex::Regex;
use sha1::{Digest, Sha1};

use super::store::{self, find_skills, re};

/// A parsed repo spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSpec {
    pub url: String,
    pub r#ref: Option<String>,
    pub subdir: Option<String>,
}

/// `owner/repo`, `owner/repo/sub/dir`, a GitHub `/tree/<ref>/<subdir>` URL or any git URL, each
/// optionally followed by `#ref`.
pub fn parse_repo(spec: &str) -> Result<RepoSpec, String> {
    static TREE: OnceLock<Regex> = OnceLock::new();
    static SHORT: OnceLock<Regex> = OnceLock::new();
    static PLAIN: OnceLock<Regex> = OnceLock::new();
    let (mut base, mut r#ref) = match spec.rfind('#') {
        Some(i) => (
            spec[..i].to_string(),
            Some(spec[i + 1..].to_string()).filter(|r| !r.is_empty()),
        ),
        None => (spec.to_string(), None),
    };
    let mut subdir: Option<String> = None;

    let tree = re(
        &TREE,
        r"^https?://github\.com/([\w.-]+/[\w.-]+?)(?:\.git)?/tree/([^/]+)(?:/(.+?))?/?$",
    );
    if let Some(c) = tree.captures(&base) {
        let (repo, tree_ref, dir) = (
            c[1].to_string(),
            c[2].to_string(),
            c.get(3).map(|m| m.as_str().to_string()),
        );
        base = repo;
        r#ref = r#ref.or(Some(tree_ref));
        subdir = dir;
    } else if let Some(c) = re(&SHORT, r"^([\w.-]+/[\w.-]+)/([\w./-]+?)/?$").captures(&base)
        && !Path::new(&base).is_absolute()
        && !base.starts_with('.')
        && !Path::new(&base).exists()
    {
        let (repo, dir) = (c[1].to_string(), c[2].to_string());
        base = repo;
        subdir = Some(dir);
    }

    if subdir
        .as_deref()
        .is_some_and(|d| d.split('/').any(|p| p == ".."))
    {
        return Err(format!(
            "invalid subdirectory \"{}\": \"..\" is not allowed",
            subdir.unwrap_or_default()
        ));
    }
    let url = if re(&PLAIN, r"^[\w.-]+/[\w.-]+$").is_match(&base) {
        format!("https://github.com/{base}.git")
    } else {
        base
    };
    Ok(RepoSpec { url, r#ref, subdir })
}

/// A stable identity so equivalent spellings of a spec compare equal.
pub fn repo_key(spec: &str) -> String {
    match parse_repo(spec) {
        Ok(r) => format!(
            "{}#{}/{}",
            r.url,
            r.r#ref.unwrap_or_default(),
            r.subdir.unwrap_or_default()
        ),
        Err(_) => spec.to_string(),
    }
}

/// Where clones live: `SPM_CACHE_DIR` or `~/.cache/spm`.
pub fn cache_dir() -> PathBuf {
    std::env::var_os("SPM_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".cache").join("spm")))
        .unwrap_or_else(|| PathBuf::from(".spm-cache"))
}

fn cache_path(repo: &RepoSpec) -> PathBuf {
    let digest = Sha1::digest(format!(
        "{}#{}",
        repo.url,
        repo.r#ref.as_deref().unwrap_or("")
    ));
    let id = &store::hex(&digest)[..8];
    let base = repo.url.trim_end_matches('/');
    let base = base.rsplit(['/', '\\']).next().unwrap_or(base);
    let base = base.strip_suffix(".git").unwrap_or(base);
    let name: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    cache_dir().join(format!("{name}-{id}"))
}

/// Run `git args…` in `cwd` (or the current dir) and return trimmed stdout.
fn git(args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    let mut cmd = Command::new("git");
    // The cache is spm's own; on Windows it can end up owned by Administrators, which git rejects
    // as "dubious ownership".
    if let Some(cwd) = cwd {
        cmd.arg("-c").arg(format!(
            "safe.directory={}",
            cwd.to_string_lossy().replace('\\', "/")
        ));
        cmd.current_dir(cwd);
    }
    cmd.args(args)
        .env("PATH", crate::selfbuild::preflight::augmented_path())
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "git is required but was not found in PATH".to_string()
        } else {
            format!("could not run git: {e}")
        }
    })?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = if err.trim().is_empty() {
            out.status.to_string()
        } else {
            err.trim().to_string()
        };
        Err(format!(
            "git {} failed: {err}",
            args.first().copied().unwrap_or("")
        ))
    }
}

fn is_sha(r: Option<&str>) -> bool {
    r.is_some_and(|r| r.len() == 40 && r.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Specs sharing a clone (same repo and ref, different subdir) must not clone concurrently.
fn lock_for(dir: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let mut map = LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.entry(dir.to_path_buf()).or_default().clone()
}

/// A fetched repo: the directory to scan and the commit it is at.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub root: PathBuf,
    pub sha: String,
}

/// Clone the repo into the cache or update the cached clone. If the update fails the cached copy
/// is used.
pub fn fetch_repo(spec: &str) -> Result<Fetched, String> {
    let repo = parse_repo(spec)?;
    let dir = cache_path(&repo);
    let lock = lock_for(&dir);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let pinned = is_sha(repo.r#ref.as_deref());
    if dir.join(".git").exists() {
        let update = || -> Result<(), String> {
            let want = repo.r#ref.as_deref().unwrap_or("HEAD");
            if !pinned || git(&["rev-parse", "HEAD"], Some(&dir))? != want.to_lowercase() {
                git(&["fetch", "--depth", "1", "origin", want], Some(&dir))?;
                git(&["reset", "--hard", "FETCH_HEAD"], Some(&dir))?;
            }
            Ok(())
        };
        if let Err(e) = update() {
            tracing::warn!("could not update {spec}, using cached copy ({e})");
        }
    } else {
        std::fs::create_dir_all(dir.parent().unwrap_or(&dir)).map_err(|e| e.to_string())?;
        if pinned {
            clone_at(&repo, &dir)?;
        } else {
            let dir_s = dir.to_string_lossy();
            let mut args = vec!["clone", "--depth", "1"];
            if let Some(r) = repo.r#ref.as_deref() {
                args.extend(["--branch", r]);
            }
            args.extend(["--", &repo.url, &dir_s]);
            git(&args, None)?;
        }
    }
    let root = match &repo.subdir {
        Some(sub) => dir.join(sub),
        None => dir.clone(),
    };
    if repo.subdir.is_some() && !root.exists() {
        return Err(format!(
            "path \"{}\" not found in {}",
            repo.subdir.unwrap_or_default(),
            repo.url
        ));
    }
    Ok(Fetched {
        root,
        sha: git(&["rev-parse", "HEAD"], Some(&dir))?,
    })
}

// `git clone --branch` cannot take a commit SHA, so fetch it into a fresh repo.
fn clone_at(repo: &RepoSpec, dir: &Path) -> Result<(), String> {
    let r = repo.r#ref.as_deref().unwrap_or_default();
    let run = || -> Result<(), String> {
        std::fs::create_dir(dir).map_err(|e| e.to_string())?;
        git(&["init", "-q"], Some(dir))?;
        git(&["fetch", "--depth", "1", "--", &repo.url, r], Some(dir))?;
        git(&["checkout", "-q", "--detach", "FETCH_HEAD"], Some(dir))?;
        Ok(())
    };
    run().inspect_err(|_| {
        let _ = std::fs::remove_dir_all(dir);
    })
}

/// `spec` pinned to `sha`, replacing any `#ref`.
pub fn pin_spec(spec: &str, sha: &str) -> String {
    let base = spec.rfind('#').map_or(spec, |i| &spec[..i]);
    format!("{base}#{sha}")
}

/// The spec to fetch for a manifest entry: a string is the repo itself; a market entry object is
/// fetched at its recorded commit.
pub fn entry_spec(entry: &serde_json::Value) -> Option<String> {
    match entry {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(o) => {
            let repo = o.get("repo")?.as_str()?;
            Some(match o.get("ref").and_then(|r| r.as_str()) {
                Some(r) => pin_spec(repo, r),
                None => repo.to_string(),
            })
        }
        _ => None,
    }
}

/// The configured pool. `SPM_REPO` replaces the manifest's pool with a single repo.
pub fn repo_specs(manifest_repos: &[String]) -> Result<Vec<String>, String> {
    let specs: Vec<String> = match std::env::var("SPM_REPO") {
        Ok(r) if !r.is_empty() => vec![r],
        _ => manifest_repos.to_vec(),
    };
    if specs.is_empty() {
        return Err("no skills repo configured: add one to the repo pool first".into());
    }
    Ok(specs)
}

fn default_name(spec: &str) -> String {
    let Ok(r) = parse_repo(spec) else {
        return String::new();
    };
    let source = r.subdir.as_deref().unwrap_or(&r.url);
    let base = source.trim_end_matches('/');
    let base = base.rsplit(['/', '\\']).next().unwrap_or(base);
    let base = base.strip_suffix(".git").unwrap_or(base);
    base.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// One place a skill was found.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub repo: String,
    pub src: PathBuf,
    pub sha: String,
}

/// Skill name to where it is found, in repo order.
pub type Pool = BTreeMap<String, Vec<Candidate>>;

/// One fetched repo: its skills as `(name, path)` and the checkout's identifier.
type RepoIndex = Result<(Vec<(String, PathBuf)>, String), String>;

/// Fetch every repo (in parallel) and index their skills by name.
pub fn load_pool(specs: &[String]) -> Result<Pool, String> {
    let indexed: Vec<RepoIndex> = std::thread::scope(|s| {
        let handles: Vec<_> = specs
            .iter()
            .map(|spec| {
                s.spawn(move || {
                    let f = fetch_repo(spec)?;
                    Ok((find_skills(&f.root, &default_name(spec)), f.sha))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err("fetch thread panicked".into()))
            })
            .collect()
    });
    let mut pool = Pool::new();
    for (spec, result) in specs.iter().zip(indexed) {
        let (skills, sha) = result?;
        for (name, src) in skills {
            pool.entry(name).or_default().push(Candidate {
                repo: spec.clone(),
                src,
                sha: sha.clone(),
            });
        }
    }
    Ok(pool)
}

/// The candidate for a skill, preferring the `wanted` repo when given.
pub fn pick_candidate<'a>(
    candidates: Option<&'a Vec<Candidate>>,
    wanted: Option<&str>,
) -> Option<&'a Candidate> {
    let candidates = candidates?;
    match wanted {
        None => candidates.first(),
        Some(w) => {
            let key = repo_key(w);
            candidates.iter().find(|c| repo_key(&c.repo) == key)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_parse_like_the_node_cli() {
        let r = parse_repo("owner/repo").unwrap();
        assert_eq!(r.url, "https://github.com/owner/repo.git");
        assert_eq!((r.r#ref, r.subdir), (None, None));

        let r = parse_repo("owner/repo/skills/x#v1").unwrap();
        assert_eq!(r.url, "https://github.com/owner/repo.git");
        assert_eq!(r.r#ref.as_deref(), Some("v1"));
        assert_eq!(r.subdir.as_deref(), Some("skills/x"));

        let r = parse_repo("https://github.com/o/r/tree/main/a/b").unwrap();
        assert_eq!(r.url, "https://github.com/o/r.git");
        assert_eq!(r.r#ref.as_deref(), Some("main"));
        assert_eq!(r.subdir.as_deref(), Some("a/b"));

        let r = parse_repo("git@host:o/r.git").unwrap();
        assert_eq!(r.url, "git@host:o/r.git");

        assert!(parse_repo("o/r/../x").is_err());
    }

    #[test]
    fn equivalent_spellings_share_a_key() {
        assert_eq!(repo_key("o/r"), repo_key("https://github.com/o/r.git"));
        assert_ne!(repo_key("o/r"), repo_key("o/r#dev"));
    }

    #[test]
    fn cache_folder_matches_the_node_cli() {
        // sha1("https://github.com/o/r.git#") = the 8 hex chars the Node CLI picks.
        let repo = parse_repo("o/r").unwrap();
        let p = cache_path(&repo);
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("r-"), "{name}");
        assert_eq!(name.len(), "r-".len() + 8);
    }

    #[test]
    fn pinning_replaces_the_ref() {
        assert_eq!(pin_spec("o/r#main", "abc"), "o/r#abc");
        assert_eq!(pin_spec("o/r", "abc"), "o/r#abc");
    }
}
