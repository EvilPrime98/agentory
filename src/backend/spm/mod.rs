//! Built-in skill package manager, compatible with the Node `spm` CLI: same `spm.json`, same
//! `.claude/.spm-state.json`, same `~/.cache/spm` clones, same `SPM_CACHE_DIR`, `SPM_REPO` and
//! `SPM_GLOBAL_DIR`, same isolation through `skillOverrides`. A project works with either tool.
//!
//! Market search, pinned market installs, `update --latest` and `-g` global installs are all
//! here. Market installs and `--latest` are two-phase: `prepare_*` fetches the skill and returns a
//! [`Review`] (or finishes when there is nothing to review), and [`confirm`] copies it once the
//! user has approved. Market entries in `spm.json` are reinstalled at their pinned commit.
//!
//! The work is blocking (filesystem, `git`, HTTP), so every public function runs on the blocking
//! pool.

mod doctor;
mod market;
mod repo;
mod review;
mod store;

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::model::{DoctorReport, SpmManifest};
pub use market::{Hit, parse_ref as parse_market_ref};
use repo::{Candidate, Pool, entry_spec, load_pool, pick_candidate, repo_key, repo_specs};
pub use review::Review;
use store::{Manifest, State};

/// What is on disk for a project.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// `None` when the project has no `spm.json`.
    pub manifest: Option<SpmManifest>,
    /// Folder names under `.claude/skills`.
    pub installed: Vec<String>,
}

pub fn read_snapshot(project: &Path) -> Snapshot {
    let manifest = std::fs::read_to_string(project.join("spm.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    let mut installed: Vec<String> = std::fs::read_dir(project.join(".claude").join("skills"))
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    installed.sort();
    Snapshot {
        manifest,
        installed,
    }
}

/// The folder of global skills that isolation hides: `SPM_GLOBAL_DIR` or `~/.claude/skills`.
fn global_skills_dir() -> PathBuf {
    std::env::var_os("SPM_GLOBAL_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".claude").join("skills")))
        .unwrap_or_default()
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| format!("spm task failed: {e}"))?
}

// ---- project state ------------------------------------------------------------------------

struct Paths {
    skills: PathBuf,
    manifest: PathBuf,
    state: PathBuf,
    settings: PathBuf,
}

impl Paths {
    fn of(project: &Path) -> Self {
        let claude = project.join(".claude");
        Self {
            skills: claude.join("skills"),
            manifest: project.join("spm.json"),
            state: claude.join(".spm-state.json"),
            settings: claude.join("settings.json"),
        }
    }
}

struct Config {
    manifest: Manifest,
    state: State,
}

fn load(p: &Paths) -> Result<Config, String> {
    Ok(Config {
        manifest: store::read_manifest(&p.manifest)?,
        state: store::read_state(&p.state)?,
    })
}

fn save(p: &Paths, cfg: &Config) -> Result<(), String> {
    store::write_manifest(&p.manifest, &cfg.manifest)?;
    store::write_state(&p.state, &cfg.state)?;
    if cfg.manifest.isolate() {
        store::sync_overrides(&p.settings, &global_skills_dir(), &p.skills)?;
    }
    Ok(())
}

fn io(e: std::io::Error) -> String {
    e.to_string()
}

fn spm_repo_set() -> bool {
    std::env::var_os("SPM_REPO").is_some_and(|v| !v.is_empty())
}

/// Add `spec` to the pool unless an equivalent one is already there.
fn with_repo(repos: &[String], spec: &str) -> Vec<String> {
    let key = repo_key(spec);
    let mut out = repos.to_vec();
    if !repos.iter().any(|r| repo_key(r) == key) {
        out.push(spec.to_string());
    }
    out
}

fn find_repo<'a>(repos: &'a [String], spec: &str) -> Option<&'a String> {
    let key = repo_key(spec);
    repos.iter().find(|r| repo_key(r) == key)
}

/// The repo a skill is pinned to in the manifest; `SPM_REPO` overrides pins.
fn pinned_repo(m: &Manifest, name: &str) -> Option<String> {
    if spm_repo_set() {
        return None;
    }
    m.skills.get(name).and_then(entry_spec)
}

fn is_market(entry: Option<&Value>) -> bool {
    entry.is_some_and(Value::is_object)
}

/// Repos to fetch for `names`: those the skills are pinned to, or the whole pool when any is
/// unpinned.
fn repos_for(m: &Manifest, names: &[String]) -> Result<Vec<String>, String> {
    if spm_repo_set() {
        return repo_specs(&m.repos);
    }
    let mut pins: Vec<String> = Vec::new();
    for n in names {
        let pin = match m.skills.get(n) {
            Some(e @ Value::Object(_)) => entry_spec(e),
            Some(Value::String(s)) => find_repo(&m.repos, s).cloned(),
            _ => None,
        };
        match pin {
            Some(p) if !pins.contains(&p) => pins.push(p),
            Some(_) => {}
            None => return repo_specs(&m.repos),
        }
    }
    Ok(pins)
}

fn suggest(name: &str, available: &[&String]) -> String {
    let close: Vec<&str> = available
        .iter()
        .filter(|a| a.contains(name) || name.contains(a.as_str()))
        .take(3)
        .map(|a| a.as_str())
        .collect();
    if close.is_empty() {
        String::new()
    } else {
        format!("\n  did you mean: {}?", close.join(", "))
    }
}

fn finish(log: Vec<String>, failed: Vec<String>) -> Result<String, String> {
    if failed.is_empty() {
        Ok(log.join("\n"))
    } else {
        Err(failed.join("\n"))
    }
}

// ---- commands -----------------------------------------------------------------------------

/// `spm init [repo]`: create `spm.json` and `.claude/skills`, optionally seeding the pool with
/// `repo`. Re-running it on an existing manifest switches isolation: on by default, off with
/// `isolate: false`.
pub async fn init(project: &Path, repo: Option<&str>, isolate: bool) -> Result<String, String> {
    let (project, repo) = (project.to_path_buf(), repo.map(str::to_string));
    blocking(move || {
        let p = Paths::of(&project);
        std::fs::create_dir_all(&p.skills).map_err(io)?;
        let mut cfg = load(&p)?;
        if let Some(r) = &repo {
            cfg.manifest.repos = with_repo(&cfg.manifest.repos, r);
        }
        cfg.manifest.set_isolate(isolate);
        save(&p, &cfg)?;
        if !isolate {
            store::remove_overrides(&p.settings, &global_skills_dir())?;
        }
        Ok(if isolate {
            "Initialized spm.json and .claude/skills; global skills are hidden for this project (.claude/settings.json)"
        } else {
            "Initialized spm.json and .claude/skills"
        }
        .to_string())
    })
    .await
}

/// `spm install <skills…>`: add them to `spm.json` and copy them into `.claude/skills` from the
/// first repo of the pool that has them. With no names, recreate `.claude/skills` from `spm.json`.
pub async fn install(project: &Path, skills: &[String]) -> Result<String, String> {
    let (project, names) = (project.to_path_buf(), skills.to_vec());
    blocking(move || install_sync(&project, &names)).await
}

/// The note for a skill that several repos of the pool provide: the first one is used.
fn also_in(name: &str, candidates: Option<&Vec<Candidate>>, used: &Candidate) -> Option<String> {
    let others = candidates?.get(1..).filter(|o| !o.is_empty())?;
    let repos: Vec<&str> = others.iter().map(|c| c.repo.as_str()).collect();
    Some(format!(
        "! {name}: also in {}; using {}",
        repos.join(", "),
        used.repo
    ))
}

fn install_sync(project: &Path, names: &[String]) -> Result<String, String> {
    let p = Paths::of(project);
    let mut cfg = load(&p)?;
    let targets: Vec<String> = if names.is_empty() {
        cfg.manifest.skills.keys().cloned().collect()
    } else {
        names.to_vec()
    };
    if targets.is_empty() {
        return Ok("Nothing to install.".into());
    }
    // A project made only of market installs has no pool, and does not need one to reinstall.
    let specs = if !names.is_empty() || spm_repo_set() {
        repo_specs(&cfg.manifest.repos)?
    } else {
        cfg.manifest.repos.clone()
    };
    let fetch = if names.is_empty() {
        repos_for(&cfg.manifest, &targets)?
    } else {
        specs.clone()
    };
    let pool = load_pool(&fetch)?;
    let available: Vec<&String> = pool.keys().collect();
    std::fs::create_dir_all(&p.skills).map_err(io)?;

    let (mut log, mut failed) = (Vec::new(), Vec::new());
    for name in &targets {
        let candidates = pool.get(name);
        // Reinstalling from spm.json keeps the repo each skill came from.
        let wanted = if names.is_empty() {
            pinned_repo(&cfg.manifest, name)
        } else {
            None
        };
        let Some(found) = pick_candidate(candidates, wanted.as_deref()) else {
            failed.push(format!(
                "✗ {name}: not found in {}{}",
                wanted.unwrap_or_else(|| specs.join(", ")),
                suggest(name, &available)
            ));
            continue;
        };
        if wanted.is_none() {
            log.extend(also_in(name, candidates, &found));
        }
        let current = cfg.manifest.skills.get(name).cloned();
        if spm_repo_set() {
            cfg.manifest
                .skills
                .insert(name.clone(), current.unwrap_or(Value::Null));
        } else if !names.is_empty() || !is_market(current.as_ref()) {
            cfg.manifest
                .skills
                .insert(name.clone(), Value::String(found.repo.clone()));
        }
        let dest = p.skills.join(name);
        if dest.exists() {
            log.push(format!("• {name}: already installed"));
            if !cfg.state.contains_key(name) {
                let hash = store::hash_dir(&dest).map_err(io)?;
                cfg.state
                    .insert(name.clone(), store::state_entry(&hash, &found.sha));
            }
            continue;
        }
        store::copy_skill(&found.src, &dest).map_err(io)?;
        let hash = store::hash_dir(&dest).map_err(io)?;
        cfg.state
            .insert(name.clone(), store::state_entry(&hash, &found.sha));
        log.push(format!("✓ {name}"));
    }
    save(&p, &cfg)?;
    finish(log, failed)
}

/// `spm install -g <skills…>`: copy them from the pool into the global skills folder. Nothing is
/// recorded; the project manifest is only read, for its pool.
pub async fn install_global(project: &Path, skills: &[String]) -> Result<String, String> {
    let (project, names) = (project.to_path_buf(), skills.to_vec());
    blocking(move || {
        if names.is_empty() {
            return Err("usage: install -g <skill...>".into());
        }
        let cfg = load(&Paths::of(&project))?;
        let specs = repo_specs(&cfg.manifest.repos)?;
        let pool = load_pool(&specs)?;
        let available: Vec<&String> = pool.keys().collect();
        let dir = global_skills_dir();
        std::fs::create_dir_all(&dir).map_err(io)?;
        let (mut log, mut failed) = (Vec::new(), Vec::new());
        for name in &names {
            let candidates = pool.get(name);
            let Some(found) = pick_candidate(candidates, None) else {
                failed.push(format!(
                    "✗ {name}: not found in {}{}",
                    specs.join(", "),
                    suggest(name, &available)
                ));
                continue;
            };
            log.extend(also_in(name, candidates, &found));
            let dest = dir.join(name);
            if dest.exists() {
                log.push(format!("• {name}: already installed"));
                continue;
            }
            store::copy_skill(&found.src, &dest).map_err(io)?;
            log.push(format!("✓ {name} (global)"));
        }
        finish(log, failed)
    })
    .await
}

// ---- markets ------------------------------------------------------------------------------

/// Search the skill markets by keyword.
pub async fn search_market(term: &str) -> Result<Vec<Hit>, String> {
    let term = term.trim().to_string();
    blocking(move || {
        if term.is_empty() {
            return Ok(Vec::new());
        }
        market::search(&term)
    })
    .await
}

/// Where a prepare step ended up.
#[derive(Debug, Clone)]
pub enum Prepared {
    /// Nothing to approve; the text says what happened.
    Done(String),
    /// Show the review, then call [`confirm`] if the user approves.
    Review(Box<Review>),
}

/// The skill and where it was found, for a market repo: the one named `want`, or the only one.
fn market_entry(pool: &Pool, want: &str, repo: &str) -> Result<(String, Candidate), String> {
    let (name, cands) = match pool.get_key_value(want) {
        Some(kv) => kv,
        None if pool.len() == 1 => pool.iter().next().unwrap(),
        None => return Err(format!("{want} not found in {repo}")),
    };
    Ok((name.clone(), cands[0].clone()))
}

fn build_review(
    name: &str,
    found: &Candidate,
    market: &str,
    from: Option<String>,
    global: bool,
) -> Result<Review, String> {
    if name.is_empty() || name.contains(['/', '\\']) || name == ".." || name == "." {
        return Err(format!("✗ {name}: not a valid skill name"));
    }
    let report = review::inspect_skill(&found.src).map_err(io)?;
    if !report.symlinks.is_empty() {
        return Err(format!(
            "✗ {name}: contains symlinks ({}), not installing",
            report.symlinks.join(", ")
        ));
    }
    Ok(Review {
        name: name.to_string(),
        repo: found.repo.clone(),
        sha: found.sha.clone(),
        hash: store::hash_dir(&found.src).map_err(io)?,
        market: market.to_string(),
        from,
        global,
        report,
        src: found.src.clone(),
    })
}

/// First phase of `install <market>:<id>`: resolve it on the market, fetch the repo at its
/// current commit and inspect the skill.
pub async fn prepare_install(
    project: &Path,
    market: &str,
    id: &str,
    global: bool,
) -> Result<Prepared, String> {
    let (project, market, id) = (project.to_path_buf(), market.to_string(), id.to_string());
    blocking(move || {
        if market != market::SKILLSMP {
            return Err(format!(
                "unknown market \"{market}\" (available: {})",
                market::SKILLSMP
            ));
        }
        let hit = market::resolve(&id)?;
        prepare_install_hit(&project, &hit, &market, global)
    })
    .await
}

fn prepare_install_hit(
    project: &Path,
    hit: &Hit,
    market: &str,
    global: bool,
) -> Result<Prepared, String> {
    let pool = load_pool(std::slice::from_ref(&hit.repo))?;
    let (name, found) = market_entry(&pool, &hit.name, &hit.repo)?;
    let skills_dir = if global {
        global_skills_dir()
    } else {
        Paths::of(project).skills
    };
    if skills_dir.join(&name).exists() {
        return Ok(Prepared::Done(format!("• {name}: already installed")));
    }
    Ok(Prepared::Review(Box::new(build_review(
        &name, &found, market, None, global,
    )?)))
}

/// First phase of `update --latest` for one market skill: fetch the newest commit of its repo.
/// When the content is unchanged only the recorded commit moves and nothing is asked.
pub async fn prepare_update_latest(project: &Path, skill: &str) -> Result<Prepared, String> {
    let (project, skill) = (project.to_path_buf(), skill.to_string());
    blocking(move || {
        let p = Paths::of(&project);
        let mut cfg = load(&p)?;
        let (repo, pinned, market) = match cfg.manifest.skills.get(&skill) {
            Some(Value::Object(o)) => (
                o.get("repo").and_then(Value::as_str).map(str::to_string),
                o.get("ref").and_then(Value::as_str).map(str::to_string),
                o.get("market")
                    .and_then(Value::as_str)
                    .unwrap_or(market::SKILLSMP)
                    .to_string(),
            ),
            Some(_) => return Err(format!("✗ {skill}: not installed from a market")),
            None => return Err(format!("✗ {skill}: not installed")),
        };
        let repo = repo.ok_or_else(|| format!("✗ {skill}: no repo recorded in spm.json"))?;
        let pool = load_pool(std::slice::from_ref(&repo))?;
        let Some(found) = pick_candidate(pool.get(&skill), Some(&repo)) else {
            return Err(format!("✗ {skill}: no longer in the repo ({repo})"));
        };
        let dest = p.skills.join(&skill);
        let repo_hash = store::hash_dir(&found.src).map_err(io)?;
        let local_hash = dest
            .exists()
            .then(|| store::hash_dir(&dest))
            .transpose()
            .map_err(io)?;
        let moved = pinned.as_deref() != Some(found.sha.as_str());
        if local_hash.as_deref() == Some(repo_hash.as_str()) {
            if moved {
                if let Some(Value::Object(o)) = cfg.manifest.skills.get_mut(&skill) {
                    o.insert("ref".into(), Value::String(found.sha.clone()));
                }
            }
            cfg.state
                .insert(skill.clone(), store::state_entry(&repo_hash, &found.sha));
            save(&p, &cfg)?;
            return Ok(Prepared::Done(format!("• {skill}: up to date")));
        }
        if !moved {
            return Ok(Prepared::Done(format!(
                "• {skill}: already at the latest commit (use Update all to restore local edits)"
            )));
        }
        Ok(Prepared::Review(Box::new(build_review(
            &skill, &found, &market, pinned, false,
        )?)))
    })
    .await
}

/// Second phase: copy a reviewed skill. A global install records nothing; a project install pins
/// the manifest entry to the reviewed commit.
pub async fn confirm(project: &Path, review: Review) -> Result<String, String> {
    let project = project.to_path_buf();
    blocking(move || {
        let name = &review.name;
        // The clone is shared; make sure what is copied is what was shown.
        if store::hash_dir(&review.src).map_err(io)? != review.hash {
            return Err(format!("✗ {name}: changed since it was reviewed, not installing"));
        }
        if review.global {
            let dir = global_skills_dir();
            std::fs::create_dir_all(&dir).map_err(io)?;
            store::copy_skill(&review.src, &dir.join(name)).map_err(io)?;
            return Ok(format!("✓ {name} (global)"));
        }
        let p = Paths::of(&project);
        let mut cfg = load(&p)?;
        let dest = p.skills.join(name);
        let edited = review.from.is_some()
            && dest.exists()
            && Some(store::hash_dir(&dest).map_err(io)?) != store::state_hash(&cfg.state, name);
        std::fs::create_dir_all(&p.skills).map_err(io)?;
        store::copy_skill(&review.src, &dest).map_err(io)?;
        match cfg.manifest.skills.get_mut(name) {
            Some(Value::Object(o)) if review.from.is_some() => {
                o.insert("ref".into(), Value::String(review.sha.clone()));
            }
            _ => {
                cfg.manifest.skills.insert(
                    name.clone(),
                    serde_json::json!({ "repo": review.repo, "ref": review.sha, "market": review.market }),
                );
            }
        }
        cfg.state.insert(name.clone(), store::state_entry(&review.hash, &review.sha));
        save(&p, &cfg)?;
        Ok(if review.from.is_some() {
            format!("✓ {name}: updated{}", if edited { " (local edits overwritten)" } else { "" })
        } else {
            format!("✓ {name}")
        })
    })
    .await
}

/// A skill in the repo pool, as `spm available` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableSkill {
    pub name: String,
    /// Empty when the pool has no usable description.
    pub description: String,
}

/// Every skill in the repo pool with its full description, by name.
pub async fn list_available(project: &Path) -> Result<Vec<AvailableSkill>, String> {
    let project = project.to_path_buf();
    blocking(move || {
        let cfg = load(&Paths::of(&project))?;
        let pool = load_pool(&repo_specs(&cfg.manifest.repos)?)?;
        Ok(available_rows(&pool))
    })
    .await
}

fn available_rows(pool: &Pool) -> Vec<AvailableSkill> {
    let mut names: Vec<&String> = pool.keys().collect();
    names.sort_by(|a, b| store::locale_cmp(a, b));
    names
        .into_iter()
        .flat_map(|name| pool[name].iter().map(move |c| (name, c)))
        .map(|(name, c): (&String, &Candidate)| AvailableSkill {
            name: name.clone(),
            description: std::fs::read_to_string(c.src.join("SKILL.md"))
                .map(|t| store::parse_skill(&t).description)
                .unwrap_or_default(),
        })
        .collect()
}

pub async fn uninstall(project: &Path, skill: &str) -> Result<String, String> {
    let (project, skill) = (project.to_path_buf(), skill.to_string());
    blocking(move || {
        let p = Paths::of(&project);
        let mut cfg = load(&p)?;
        let dest = p.skills.join(&skill);
        if !dest.exists() && !cfg.manifest.skills.contains_key(&skill) {
            return Err(format!("✗ {skill}: not installed"));
        }
        if dest.exists() {
            std::fs::remove_dir_all(&dest).map_err(io)?;
        }
        cfg.manifest.skills.remove(&skill);
        cfg.state.remove(&skill);
        save(&p, &cfg)?;
        Ok(format!("✓ removed {skill}"))
    })
    .await
}

/// `spm update`: re-copy every installed skill from its repo. Market entries stay on their
/// pinned commit.
pub async fn update(project: &Path) -> Result<String, String> {
    let project = project.to_path_buf();
    blocking(move || {
        let p = Paths::of(&project);
        let mut cfg = load(&p)?;
        let targets: Vec<String> = cfg.manifest.skills.keys().cloned().collect();
        if targets.is_empty() {
            return Ok("No skills installed in this project.".into());
        }
        let pool = load_pool(&repos_for(&cfg.manifest, &targets)?)?;
        let (mut log, mut failed) = (Vec::new(), Vec::new());
        for name in &targets {
            let dest = p.skills.join(name);
            let wanted = pinned_repo(&cfg.manifest, name);
            let Some(found) = pick_candidate(pool.get(name), wanted.as_deref()) else {
                failed.push(match wanted {
                    Some(w) => format!("✗ {name}: no longer in the repo ({w})"),
                    None => format!("✗ {name}: no longer in the repo pool"),
                });
                continue;
            };
            let market = is_market(cfg.manifest.skills.get(name));
            if !spm_repo_set() && !market {
                cfg.manifest
                    .skills
                    .insert(name.clone(), Value::String(found.repo.clone()));
            }
            let repo_hash = store::hash_dir(&found.src).map_err(io)?;
            let local_hash = dest
                .exists()
                .then(|| store::hash_dir(&dest))
                .transpose()
                .map_err(io)?;
            if local_hash.as_deref() == Some(repo_hash.as_str()) {
                cfg.state
                    .insert(name.clone(), store::state_entry(&repo_hash, &found.sha));
                log.push(format!("• {name}: up to date"));
                continue;
            }
            let edited = local_hash.is_some() && local_hash != store::state_hash(&cfg.state, name);
            store::copy_skill(&found.src, &dest).map_err(io)?;
            cfg.state
                .insert(name.clone(), store::state_entry(&repo_hash, &found.sha));
            log.push(format!(
                "✓ {name}: updated{}",
                if edited {
                    " (local edits overwritten)"
                } else {
                    ""
                }
            ));
        }
        save(&p, &cfg)?;
        finish(log, failed)
    })
    .await
}

pub async fn repo_add(project: &Path, repo: &str) -> Result<String, String> {
    let (project, spec) = (project.to_path_buf(), repo.to_string());
    blocking(move || {
        let p = Paths::of(&project);
        let mut cfg = load(&p)?;
        if find_repo(&cfg.manifest.repos, &spec).is_some() {
            return Ok(format!("• {spec}: already in the pool"));
        }
        let count = load_pool(std::slice::from_ref(&spec))?.len();
        cfg.manifest.repos = with_repo(&cfg.manifest.repos, &spec);
        save(&p, &cfg)?;
        Ok(format!("✓ added {spec} ({count} skills)"))
    })
    .await
}

pub async fn repo_rm(project: &Path, repo: &str) -> Result<String, String> {
    let (project, spec) = (project.to_path_buf(), repo.to_string());
    blocking(move || {
        let p = Paths::of(&project);
        let mut cfg = load(&p)?;
        let existing = find_repo(&cfg.manifest.repos, &spec)
            .cloned()
            .ok_or_else(|| format!("{spec} is not in the repo pool"))?;
        let users: Vec<String> = cfg
            .manifest
            .skills
            .iter()
            .filter(|(_, v)| v.as_str() == Some(existing.as_str()))
            .map(|(k, _)| k.clone())
            .collect();
        if !users.is_empty() {
            return Err(format!("{existing} is used by {}", users.join(", ")));
        }
        cfg.manifest.repos.retain(|r| *r != existing);
        save(&p, &cfg)?;
        Ok(format!("✓ removed {existing}"))
    })
    .await
}

/// Context cost, usage over the last 60 days and overlaps of the skills Claude sees here.
pub async fn doctor(project: &Path) -> Result<DoctorReport, String> {
    let project = project.to_path_buf();
    blocking(move || doctor::report(&project, 60)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentory-spm-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    fn write_skill(repo: &Path, name: &str, description: &str) {
        let d = repo.join("skills").join(name);
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\nbody of {name}\n"),
        )
        .unwrap();
    }

    #[test]
    fn snapshot_of_missing_folder_is_empty() {
        let s = read_snapshot(Path::new("Z:/definitely/not/here"));
        assert!(s.manifest.is_none());
        assert!(s.installed.is_empty());
    }

    /// Both tests below point the process-wide env vars at their own folders.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn market_install_update_latest_and_global() {
        let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let root = scratch("market");
        let (cache, global) = (root.join("cache"), root.join("global"));
        unsafe {
            std::env::set_var("SPM_CACHE_DIR", &cache);
            std::env::set_var("SPM_GLOBAL_DIR", &global);
            std::env::remove_var("SPM_REPO");
        }
        let upstream = root.join("upstream");
        fs::create_dir_all(&upstream).unwrap();
        git(&upstream, &["init", "-q"]);
        write_skill(&upstream, "alpha", "Does a thing");
        fs::write(upstream.join("skills/alpha/run.sh"), "echo hi").unwrap();
        git(&upstream, &["add", "."]);
        git(&upstream, &["commit", "-q", "-m", "one"]);
        let spec = upstream.to_string_lossy().replace('\\', "/");
        let hit = Hit {
            name: "alpha".into(),
            author: "me".into(),
            description: String::new(),
            repo: spec.clone(),
            stars: 0,
        };

        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Review first: nothing is copied until it is confirmed.
            let Prepared::Review(review) =
                prepare_install_hit(&project, &hit, "skillsmp", false).unwrap()
            else {
                panic!("expected a review");
            };
            assert!(!project.join(".claude/skills/alpha").exists());
            assert!(
                review.text().contains("script or binary: run.sh"),
                "{}",
                review.text()
            );
            let first_sha = review.sha.clone();
            confirm(&project, (*review).clone()).await.unwrap();
            assert!(project.join(".claude/skills/alpha/run.sh").is_file());
            let manifest: Value =
                serde_json::from_str(&fs::read_to_string(project.join("spm.json")).unwrap())
                    .unwrap();
            assert_eq!(
                manifest["skills"]["alpha"],
                serde_json::json!({"repo": spec, "ref": first_sha, "market": "skillsmp"})
            );
            let state: Value = serde_json::from_str(
                &fs::read_to_string(project.join(".claude/.spm-state.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(state["skills"]["alpha"]["hash"], review.hash.as_str());
            assert!(matches!(
                prepare_install_hit(&project, &hit, "skillsmp", false).unwrap(),
                Prepared::Done(_)
            ));

            // Nothing new upstream.
            let Prepared::Done(msg) = prepare_update_latest(&project, "alpha").await.unwrap()
            else {
                panic!("expected up to date");
            };
            assert!(msg.contains("up to date"), "{msg}");

            // A new upstream commit asks for a review, and only confirming moves the pin.
            fs::write(upstream.join("skills/alpha/extra.md"), "more").unwrap();
            git(&upstream, &["add", "."]);
            git(&upstream, &["commit", "-q", "-m", "two"]);
            let Prepared::Review(next) = prepare_update_latest(&project, "alpha").await.unwrap()
            else {
                panic!("expected a review");
            };
            assert_eq!(next.from.as_deref(), Some(first_sha.as_str()));
            assert_ne!(next.sha, first_sha);
            assert!(!project.join(".claude/skills/alpha/extra.md").exists());
            let out = confirm(&project, *next.clone()).await.unwrap();
            assert!(out.contains("alpha: updated"), "{out}");
            assert!(project.join(".claude/skills/alpha/extra.md").is_file());
            let manifest: Value =
                serde_json::from_str(&fs::read_to_string(project.join("spm.json")).unwrap())
                    .unwrap();
            assert_eq!(manifest["skills"]["alpha"]["ref"], next.sha.as_str());

            // A plain update stays on the pinned commit; a pool skill is not a market skill.
            assert!(
                update(&project)
                    .await
                    .unwrap()
                    .contains("alpha: up to date")
            );
            let err = prepare_update_latest(&project, "nope").await.unwrap_err();
            assert!(err.contains("not installed"), "{err}");

            // Global installs: reviewed market ones and pool ones, nothing recorded.
            let Prepared::Review(g) =
                prepare_install_hit(&project, &hit, "skillsmp", true).unwrap()
            else {
                panic!("expected a review");
            };
            confirm(&project, *g).await.unwrap();
            assert!(global.join("alpha/SKILL.md").is_file());
            init(&project, Some(&spec), false).await.unwrap();
            fs::remove_dir_all(global.join("alpha")).unwrap();
            let out = install_global(&project, &["alpha".into()]).await.unwrap();
            assert!(out.contains("alpha (global)"), "{out}");
            assert!(global.join("alpha/SKILL.md").is_file());
            assert!(
                install_global(&project, &["zzz".into()])
                    .await
                    .unwrap_err()
                    .contains("not found")
            );
            let manifest: Value =
                serde_json::from_str(&fs::read_to_string(project.join("spm.json")).unwrap())
                    .unwrap();
            assert!(
                manifest["skills"]["alpha"].is_object(),
                "global installs record nothing"
            );
        });
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn full_lifecycle_against_a_local_repo() {
        let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let root = scratch("lifecycle");
        let (cache, global) = (root.join("cache"), root.join("global"));
        // SAFETY of parallel tests: only this test depends on these variables.
        unsafe {
            std::env::set_var("SPM_CACHE_DIR", &cache);
            std::env::set_var("SPM_GLOBAL_DIR", &global);
            std::env::remove_var("SPM_REPO");
        }
        write_skill(&global, "noisy", "A global skill");
        fs::rename(global.join("skills").join("noisy"), global.join("noisy")).unwrap();

        let upstream = root.join("upstream");
        fs::create_dir_all(&upstream).unwrap();
        git(&upstream, &["init", "-q"]);
        write_skill(&upstream, "alpha", "Does a thing");
        write_skill(&upstream, "beta", "Second skill");
        git(&upstream, &["add", "."]);
        git(&upstream, &["commit", "-q", "-m", "one"]);
        let spec = upstream.to_string_lossy().replace('\\', "/");

        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            init(&project, Some(&spec), true).await.unwrap();
            let manifest: Value =
                serde_json::from_str(&fs::read_to_string(project.join("spm.json")).unwrap())
                    .unwrap();
            assert_eq!(manifest["repos"][0], spec.as_str());
            assert_eq!(manifest["isolate"], true);
            let settings: Value = serde_json::from_str(
                &fs::read_to_string(project.join(".claude").join("settings.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(settings["skillOverrides"]["noisy"], "off");

            let rows = list_available(&project).await.unwrap();
            let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
            assert_eq!(names, ["alpha", "beta"]);
            assert_eq!(rows[0].description, "Does a thing");

            install(&project, &["alpha".into()]).await.unwrap();
            assert!(project.join(".claude/skills/alpha/SKILL.md").is_file());
            let err = install(&project, &["missing".into()]).await.unwrap_err();
            assert!(err.contains("✗ missing: not found"), "{err}");

            // A skill edited by hand is reported when update overwrites it.
            fs::write(project.join(".claude/skills/alpha/extra.txt"), "mine").unwrap();
            let out = update(&project).await.unwrap();
            assert!(
                out.contains("alpha: updated (local edits overwritten)"),
                "{out}"
            );
            assert!(!project.join(".claude/skills/alpha/extra.txt").exists());
            assert!(
                update(&project)
                    .await
                    .unwrap()
                    .contains("alpha: up to date")
            );

            // Reinstalling from spm.json recreates a deleted folder.
            fs::remove_dir_all(project.join(".claude/skills/alpha")).unwrap();
            install(&project, &[]).await.unwrap();
            assert!(project.join(".claude/skills/alpha/SKILL.md").is_file());

            let err = repo_rm(&project, &spec).await.unwrap_err();
            assert!(err.contains("is used by alpha"), "{err}");
            uninstall(&project, "alpha").await.unwrap();
            assert!(!project.join(".claude/skills/alpha").exists());
            repo_rm(&project, &spec).await.unwrap();
            repo_add(&project, &spec).await.unwrap();

            let report = doctor(&project).await.unwrap();
            assert!(report.skills.is_empty(), "{report:?}");

            init(&project, None, false).await.unwrap();
            let settings: Value = serde_json::from_str(
                &fs::read_to_string(project.join(".claude").join("settings.json")).unwrap(),
            )
            .unwrap();
            assert!(settings.get("skillOverrides").is_none());
        });
        let _ = fs::remove_dir_all(&root);
    }
}
