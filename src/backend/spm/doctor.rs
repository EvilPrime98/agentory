//! `spm doctor`: what the skills Claude sees cost in context, how often each was used in the
//! project's transcripts, and which ones say the same thing.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::store::{self, list_skills, parse_skill, skill_names};
use crate::model::{DoctorOverlap, DoctorReport, DoctorSkill};

const STOPWORDS: &str = "the and for with this that use used using when user users you your from into any not are can has have will its it's all also than then them their there these those such only other more most some what which who how why where about over under after before while should would could may might must does did done being been was were out off per via etc";
const OVERLAP_THRESHOLD: f64 = 0.45;
const GLOBAL_NOISE_TOKENS: u32 = 500;
const DAY_MS: i64 = 86_400_000;

/// Tokens in a text, estimated as `chars / 4` rounded up (chars counted as UTF-16 units, like JS).
pub fn estimate_tokens(text: &str) -> u32 {
    (text.encode_utf16().count() as u32).div_ceil(4)
}

fn terms(text: &str) -> Vec<String> {
    let stop: BTreeSet<&str> = STOPWORDS.split(' ').collect();
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() > 2 && !stop.contains(t))
        .map(str::to_string)
        .collect()
}

/// Pairs of skills whose description text is at least 45% alike (TF-IDF cosine), highest first.
pub fn find_overlaps(docs: &[(String, String)]) -> Vec<DoctorOverlap> {
    let tokens: Vec<Vec<String>> = docs.iter().map(|(_, text)| terms(text)).collect();
    let mut df: BTreeMap<&str, f64> = BTreeMap::new();
    for ts in &tokens {
        for t in ts.iter().collect::<BTreeSet<_>>() {
            *df.entry(t).or_default() += 1.0;
        }
    }
    let n = docs.len() as f64;
    let vectors: Vec<(BTreeMap<&str, f64>, f64)> = tokens
        .iter()
        .map(|ts| {
            let mut v: BTreeMap<&str, f64> = BTreeMap::new();
            for t in ts {
                *v.entry(t).or_default() += 1.0;
            }
            for (t, w) in v.iter_mut() {
                *w *= (1.0 + n / df[t]).ln();
            }
            let norm = v.values().map(|w| w * w).sum::<f64>().sqrt();
            (v, norm)
        })
        .collect();
    let mut pairs = Vec::new();
    for i in 0..docs.len() {
        for j in i + 1..docs.len() {
            let ((x, nx), (y, ny)) = (&vectors[i], &vectors[j]);
            if *nx == 0.0 || *ny == 0.0 {
                continue;
            }
            let dot: f64 = x.iter().map(|(t, w)| w * y.get(t).copied().unwrap_or(0.0)).sum();
            let score = dot / (nx * ny);
            if score >= OVERLAP_THRESHOLD {
                pairs.push(DoctorOverlap {
                    a: docs[i].0.clone(),
                    b: docs[j].0.clone(),
                    score,
                });
            }
        }
    }
    pairs.sort_by(|p, q| q.score.total_cmp(&p.score));
    pairs
}

/// The folder name Claude Code uses for a project's transcripts: every non-alphanumeric becomes `-`.
pub fn transcript_folder(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Milliseconds since the epoch of an ISO-8601 timestamp (`Z` or `±hh:mm`).
fn parse_ms(ts: &str) -> Option<i64> {
    let b = ts.as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> { ts.get(from..from + len)?.parse().ok() };
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b' ') {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (h, mi, s) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    let mut rest = &ts[19..];
    let mut ms = 0;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits: String = frac.chars().take_while(char::is_ascii_digit).collect();
        ms = format!("{digits:0<3}")[..3].parse().ok()?;
        rest = &frac[digits.len()..];
    }
    let offset_min = match rest {
        "" | "Z" | "z" => 0,
        r if r.len() == 6 && (r.starts_with('+') || r.starts_with('-')) => {
            let m = r[1..3].parse::<i64>().ok()? * 60 + r[4..6].parse::<i64>().ok()?;
            if r.starts_with('+') { m } else { -m }
        }
        _ => return None,
    };
    // Days from civil (Howard Hinnant).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + mi - offset_min) * 60_000 + s * 1000 + ms)
}

fn skill_key(name: &str) -> String {
    let name = name.strip_prefix('/').unwrap_or(name);
    name.rsplit(':').next().unwrap_or(name).to_string()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Skill invocations per name in the project's Claude transcripts newer than `days`: `Skill` tool
/// calls and `/skill` slash commands. Also returns how many transcript files were read.
pub fn usage_counts(
    cwd: &str,
    days: u32,
    projects_dir: Option<&Path>,
    now: i64,
) -> (HashMap<String, u32>, u32) {
    let root = projects_dir.map(Path::to_path_buf).unwrap_or_else(|| {
        std::env::var_os("SPM_CLAUDE_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".claude")))
            .unwrap_or_default()
            .join("projects")
    });
    let dir = root.join(transcript_folder(cwd));
    let mut counts: HashMap<String, u32> = HashMap::new();
    let Ok(rd) = fs::read_dir(&dir) else {
        return (counts, 0);
    };
    let cutoff = now - i64::from(days) * DAY_MS;
    let mut files = 0;
    for e in rd.filter_map(Result::ok) {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "jsonl") {
            continue;
        }
        let modified = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as i64);
        if modified < cutoff {
            continue;
        }
        let Ok(file) = fs::File::open(&path) else { continue };
        files += 1;
        let mut bump = |name: &str| *counts.entry(skill_key(name)).or_default() += 1;
        for line in BufReader::new(file).split(b'\n').filter_map(Result::ok) {
            let line = String::from_utf8_lossy(&line);
            if !line.contains("\"Skill\"") && !line.contains("<command-name>") {
                continue;
            }
            let Ok(entry) = serde_json::from_str::<Value>(&line) else { continue };
            if entry
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(parse_ms)
                .is_some_and(|t| t < cutoff)
            {
                continue;
            }
            let content = entry.pointer("/message/content");
            match entry.get("type").and_then(Value::as_str) {
                Some("assistant") => {
                    for b in content.and_then(Value::as_array).into_iter().flatten() {
                        if b.get("type").and_then(Value::as_str) == Some("tool_use")
                            && b.get("name").and_then(Value::as_str) == Some("Skill")
                        {
                            if let Some(s) = b.pointer("/input/skill").and_then(Value::as_str) {
                                if !s.is_empty() {
                                    bump(s);
                                }
                            }
                        }
                    }
                }
                Some("user") => {
                    let text = match content {
                        Some(Value::String(s)) => s.as_str(),
                        Some(Value::Array(a)) => a
                            .first()
                            .and_then(|b| b.get("text"))
                            .and_then(Value::as_str)
                            .unwrap_or(""),
                        _ => "",
                    };
                    if let Some(cmd) = text
                        .split_once("<command-name>")
                        .and_then(|(_, rest)| rest.split_once("</command-name>"))
                        .map(|(cmd, _)| cmd)
                        .filter(|c| !c.is_empty() && !c.contains('<'))
                    {
                        bump(cmd);
                    }
                }
                _ => {}
            }
        }
    }
    (counts, files)
}

struct Scanned {
    folder: String,
    names: Vec<String>,
    skill: DoctorSkill,
    text: String,
}

fn scan(dir: &Path, source: &str, off: &BTreeSet<String>) -> Vec<Scanned> {
    let mut found = Vec::new();
    for folder in list_skills(dir) {
        let names = skill_names(dir, &folder);
        if source == "global" && names.iter().any(|n| off.contains(n)) {
            continue;
        }
        let Ok(text) = fs::read_to_string(dir.join(&folder).join("SKILL.md")) else {
            continue;
        };
        let p = parse_skill(&text);
        let shown = if p.name.is_empty() { &folder } else { &p.name };
        found.push(Scanned {
            names,
            skill: DoctorSkill {
                name: folder.clone(),
                source: source.into(),
                description_tokens: estimate_tokens(&format!("{shown} {}", p.description)),
                body_tokens: estimate_tokens(&p.body),
                ..Default::default()
            },
            text: format!("{} {shown}", p.description),
            folder,
        });
    }
    found
}

/// What the report is built from.
pub struct Input<'a> {
    pub manifest: &'a store::Manifest,
    pub skills_dir: &'a Path,
    pub global_dir: &'a Path,
    pub overrides: &'a BTreeMap<String, String>,
    pub counts: &'a HashMap<String, u32>,
    pub has_usage: bool,
}

/// Cost, usage and overlap of every skill Claude would see in the project: those declared in
/// `spm.json` (`spm`), hand-written ones (`project`) and global ones not turned off (`global`). A
/// skill is flagged when unused in the window, or when it overlaps one that is used more.
pub fn build_report(input: &Input) -> DoctorReport {
    let off: BTreeSet<String> = input
        .overrides
        .iter()
        .filter(|(_, v)| *v == "off")
        .map(|(k, _)| k.clone())
        .collect();
    let mut local = scan(input.skills_dir, "project", &off);
    for s in &mut local {
        if input.manifest.skills.contains_key(&s.folder) {
            s.skill.source = "spm".into();
        }
    }
    let shadowed: BTreeSet<&String> = local.iter().flat_map(|s| &s.names).collect();
    let global: Vec<Scanned> = scan(input.global_dir, "global", &off)
        .into_iter()
        .filter(|s| !s.names.iter().any(|n| shadowed.contains(n)))
        .collect();
    let global_tokens: u32 = global.iter().map(|s| s.skill.description_tokens).sum();
    let mut all: Vec<Scanned> = local.into_iter().chain(global).collect();
    for s in &mut all {
        s.skill.uses = s
            .names
            .iter()
            .map(|n| input.counts.get(n).copied().unwrap_or(0))
            .max()
            .unwrap_or(0);
    }
    let overlaps = find_overlaps(&all.iter().map(|s| (s.skill.name.clone(), s.text.clone())).collect::<Vec<_>>());
    let index: HashMap<String, usize> = all
        .iter()
        .enumerate()
        .map(|(i, s)| (s.skill.name.clone(), i))
        .collect();
    for o in &overlaps {
        let (x, y) = (index[&o.a], index[&o.b]);
        let cost = |i: usize| all[i].skill.description_tokens + all[i].skill.body_tokens;
        let (ux, uy) = (all[x].skill.uses, all[y].skill.uses);
        let loser = if ux == uy {
            if cost(x) >= cost(y) { x } else { y }
        } else if ux < uy {
            x
        } else {
            y
        };
        let winner = if loser == x { y } else { x };
        let reason = format!(
            "overlaps {} ({}%)",
            all[winner].skill.name,
            (o.score * 100.0).round()
        );
        all[loser].skill.reasons.push(reason);
    }
    let mut skills: Vec<DoctorSkill> = all
        .into_iter()
        .map(|mut s| {
            if input.has_usage && s.skill.uses == 0 {
                s.skill.reasons.insert(0, "unused".into());
            }
            s.skill.flagged = !s.skill.reasons.is_empty();
            s.skill
        })
        .collect();
    skills.sort_by_key(|s| std::cmp::Reverse(s.description_tokens + s.body_tokens));
    let total: u32 = skills.iter().map(|s| s.description_tokens).sum();
    let mut advice = Vec::new();
    if !input.manifest.isolate() && global_tokens >= GLOBAL_NOISE_TOKENS && global_tokens > total / 2 {
        advice.push(format!(
            "Global skills add ~{global_tokens} tokens to every session here. Turn on isolation to hide them in this project."
        ));
    }
    DoctorReport {
        skills,
        overlaps,
        advice,
        ..Default::default()
    }
}

/// The report for `project`, reading transcripts of the last `days` days.
pub fn report(project: &Path, days: u32) -> Result<DoctorReport, String> {
    let manifest = store::read_manifest(&project.join("spm.json"))?;
    let cwd = project.to_string_lossy();
    let cwd = cwd.strip_prefix(r"\\?\").unwrap_or(&cwd);
    let (counts, files) = usage_counts(cwd, days, None, now_ms());
    let settings = project.join(".claude").join("settings.json");
    let mut r = build_report(&Input {
        manifest: &manifest,
        skills_dir: &project.join(".claude").join("skills"),
        global_dir: &super::global_skills_dir(),
        overrides: &store::read_overrides(&settings),
        counts: &counts,
        has_usage: files > 0,
    });
    r.days = days;
    r.transcripts = files;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentory-doctor-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn skill(dir: &Path, folder: &str, description: &str, body: &str) {
        let d = dir.join(folder);
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("SKILL.md"),
            format!("---\nname: {folder}\ndescription: {description}\n---\n{body}"),
        )
        .unwrap();
    }

    #[test]
    fn tokens_are_chars_over_four_rounded_up() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    #[test]
    fn transcript_folder_replaces_every_symbol() {
        assert_eq!(transcript_folder(r"C:\Users\a b\p.x"), "C--Users-a-b-p-x");
    }

    #[test]
    fn timestamps_parse_to_epoch_ms() {
        assert_eq!(parse_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_ms("2024-03-01T12:00:00.250Z"), Some(1_709_294_400_250));
        assert_eq!(parse_ms("2024-03-01T14:00:00+02:00"), Some(1_709_294_400_000));
        assert_eq!(parse_ms("nope"), None);
    }

    #[test]
    fn similar_descriptions_overlap_and_unrelated_ones_do_not() {
        let docs = vec![
            ("a".to_string(), "Review pull request code changes for bugs and style".to_string()),
            ("b".to_string(), "Review pull request code changes for bugs and style issues".to_string()),
            ("c".to_string(), "Render chess boards from algebraic notation".to_string()),
        ];
        let o = find_overlaps(&docs);
        assert_eq!(o.len(), 1);
        assert_eq!((o[0].a.as_str(), o[0].b.as_str()), ("a", "b"));
    }

    #[test]
    fn folded_descriptions_join_into_one_line() {
        let p = parse_skill("---\nname: x\ndescription: >\n  first part\n  second part\n---\nbody");
        assert_eq!(p.description, "first part second part");
        assert_eq!(p.body, "body");
    }

    #[test]
    fn usage_flags_unused_and_prefers_the_used_overlap() {
        let root = scratch("report");
        let project = root.join("proj");
        let skills = project.join(".claude").join("skills");
        skill(&skills, "used", "Format Rust source files with rustfmt automatically", "body one");
        skill(&skills, "twin", "Format Rust source files with rustfmt automatically too", "body two");
        skill(&skills, "idle", "Draw chess boards from notation", "x");

        let projects = root.join("claude").join("projects");
        let transcripts = projects.join(transcript_folder(&project.to_string_lossy()));
        fs::create_dir_all(&transcripts).unwrap();
        let now = now_ms();
        let line = serde_json::json!({
            "type": "assistant",
            "timestamp": "2999-01-01T00:00:00Z",
            "message": {"content": [{"type": "tool_use", "name": "Skill", "input": {"skill": "used"}}]},
        });
        let slash = serde_json::json!({
            "type": "user",
            "message": {"content": "<command-name>/twin</command-name>"},
        });
        fs::write(transcripts.join("a.jsonl"), format!("{line}\n{line}\n{slash}\nnot json\n")).unwrap();

        let (counts, files) = usage_counts(&project.to_string_lossy(), 60, Some(&projects), now);
        assert_eq!((files, counts["used"], counts["twin"]), (1, 2, 1));

        let manifest = store::Manifest::default();
        let none = BTreeMap::new();
        let r = build_report(&Input {
            manifest: &manifest,
            skills_dir: &skills,
            global_dir: &root.join("no-global"),
            overrides: &none,
            counts: &counts,
            has_usage: true,
        });
        let by = |n: &str| r.skills.iter().find(|s| s.name == n).unwrap().clone();
        assert_eq!(by("idle").reasons, vec!["unused"]);
        assert!(by("used").reasons.is_empty());
        assert!(by("twin").reasons[0].starts_with("overlaps used ("), "{:?}", by("twin").reasons);
        assert_eq!(r.overlaps.len(), 1);
        let _ = fs::remove_dir_all(&root);
    }
}
