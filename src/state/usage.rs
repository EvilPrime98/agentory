//! Token usage: the account's plan limits (Anthropic's OAuth usage endpoint) and per-session
//! token counts (tailing Claude's transcript files).
//!
//! Both sources are undocumented and may change; every failure degrades to "unavailable" with the
//! last known values kept. The credentials file is only ever read.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{Context, Entity};
use serde_json::Value;

use crate::runtime;
use crate::state::sessions::SessionsStore;

const LIMITS_INTERVAL: Duration = Duration::from_secs(120);
const LIMITS_DEBOUNCE: Duration = Duration::from_secs(30);
const TRANSCRIPT_INTERVAL: Duration = Duration::from_secs(2);
/// Transcripts last written before the session began (minus this slack) belong to someone else.
const START_SLACK: Duration = Duration::from_secs(10);
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const DEFAULT_WINDOW: u64 = 200_000;
const LARGE_WINDOW: u64 = 1_000_000;

#[derive(Clone, Copy, Debug)]
pub struct Limit {
    /// 0–100.
    pub percent: f32,
    /// Seconds since the Unix epoch.
    pub resets_at: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PlanUsage {
    pub five_hour: Option<Limit>,
    pub weekly: Option<Limit>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// Prompt size of the latest assistant message.
    pub context_tokens: u64,
    pub model: Option<String>,
}

impl SessionUsage {
    pub fn context_window(&self) -> u64 {
        match &self.model {
            Some(m) if m.to_lowercase().contains("[1m]") => LARGE_WINDOW,
            _ => DEFAULT_WINDOW,
        }
    }

    /// Context fill, 0–100 (clamped).
    pub fn context_percent(&self) -> f32 {
        (self.context_tokens as f32 / self.context_window() as f32 * 100.0).clamp(0.0, 100.0)
    }
}

/// Read-side state of one session's transcripts.
#[derive(Default)]
struct Tracker {
    start: Option<SystemTime>,
    /// Byte offset consumed per transcript file.
    files: HashMap<PathBuf, u64>,
    /// Message ids already counted (a message is written once per content block).
    seen: HashSet<String>,
    usage: SessionUsage,
}

pub struct UsageStore {
    /// Last known plan limits.
    pub plan: Option<PlanUsage>,
    /// The last limits request failed (or there is no usable token).
    pub unavailable: bool,
    pub sessions: HashMap<String, SessionUsage>,
    trackers: HashMap<String, Tracker>,
    scanning: bool,
    fetching: bool,
    last_fetch: Option<Instant>,
}

impl UsageStore {
    pub fn new(sessions: Entity<SessionsStore>, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |s, cx| s.refresh_limits(true, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(LIMITS_INTERVAL).await;
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TRANSCRIPT_INTERVAL).await;
                let ok = this.update(cx, |s, cx| {
                    let jobs: Vec<_> = sessions
                        .read(cx)
                        .items
                        .iter()
                        .map(|i| (i.id.clone(), i.cwd.clone()))
                        .collect();
                    s.scan(jobs, cx);
                });
                if ok.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            plan: None,
            unavailable: false,
            sessions: HashMap::new(),
            trackers: HashMap::new(),
            scanning: false,
            fetching: false,
            last_fetch: None,
        }
    }

    /// Fetch the plan limits. `force` skips the debounce (the periodic poll); on-demand callers
    /// pass `false`.
    pub fn refresh_limits(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.fetching
            || (!force
                && self
                    .last_fetch
                    .is_some_and(|t| t.elapsed() < LIMITS_DEBOUNCE))
        {
            return;
        }
        self.fetching = true;
        self.last_fetch = Some(Instant::now());
        cx.spawn(async move |this, cx| {
            let result = runtime::run(fetch_limits()).await;
            this.update(cx, |s, cx| {
                s.fetching = false;
                match result {
                    Ok(plan) => {
                        s.plan = Some(plan);
                        s.unavailable = false;
                    }
                    Err(e) => {
                        tracing::debug!("usage limits unavailable: {e}");
                        s.unavailable = true;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn scan(&mut self, jobs: Vec<(String, PathBuf)>, cx: &mut Context<Self>) {
        if self.scanning {
            return;
        }
        self.scanning = true;
        let trackers = std::mem::take(&mut self.trackers);
        cx.spawn(async move |this, cx| {
            let trackers = runtime::run(async move { scan_all(trackers, jobs) }).await;
            this.update(cx, |s, cx| {
                s.scanning = false;
                let sessions: HashMap<_, _> = trackers
                    .iter()
                    .map(|(id, t)| (id.clone(), t.usage.clone()))
                    .collect();
                if sessions != s.sessions {
                    s.sessions = sessions;
                    cx.notify();
                }
                s.trackers = trackers;
            })
            .ok();
        })
        .detach();
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

async fn fetch_limits() -> Result<PlanUsage, String> {
    let path = dirs::home_dir()
        .ok_or("no home directory")?
        .join(".claude")
        .join(".credentials.json");
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("credentials: {e}"))?;
    let creds: Value = serde_json::from_str(&raw).map_err(|e| format!("credentials: {e}"))?;
    let oauth = &creds["claudeAiOauth"];
    let token = oauth["accessToken"].as_str().ok_or("no access token")?;
    if oauth["expiresAt"]
        .as_u64()
        .is_some_and(|ms| ms / 1000 <= now_secs())
    {
        return Err("token expired".into());
    }
    let resp = reqwest::Client::new()
        .get(USAGE_URL)
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let body: Value = resp.json().await.map_err(|e| e.to_string())?;
    let limit = |key: &str| -> Option<Limit> {
        let v = body.get(key).filter(|v| !v.is_null())?;
        Some(Limit {
            percent: v["utilization"].as_f64()? as f32,
            resets_at: v["resets_at"].as_str().and_then(parse_iso),
        })
    };
    let plan = PlanUsage {
        five_hour: limit("five_hour"),
        weekly: limit("seven_day"),
    };
    if plan.five_hour.is_none() && plan.weekly.is_none() {
        return Err("unrecognised response".into());
    }
    Ok(plan)
}

/// `2026-10-30T15:00:00.123+00:00` → Unix seconds. Fractions are ignored.
fn parse_iso(s: &str) -> Option<u64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let time_end = rest.find(['Z', '+', '-']).unwrap_or(rest.len());
    let (time, zone) = rest.split_at(time_end);
    let mut t = time.split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let mi: i64 = t.next()?.parse().ok()?;
    let sec: i64 = t.next()?.split('.').next()?.parse().ok()?;
    let offset = match zone.chars().next() {
        Some(sign @ ('+' | '-')) => {
            let (oh, om) = zone[1..].split_once(':')?;
            let secs = oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60;
            if sign == '+' { secs } else { -secs }
        }
        _ => 0,
    };
    // Days from civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + h * 3600 + mi * 60 + sec - offset).ok()
}

/// Claude names a project's transcript folder after its cwd with every non-alphanumeric
/// character replaced by `-`.
fn project_dir(cwd: &Path) -> Option<PathBuf> {
    let name: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    Some(
        dirs::home_dir()?
            .join(".claude")
            .join("projects")
            .join(name),
    )
}

fn scan_all(
    mut trackers: HashMap<String, Tracker>,
    jobs: Vec<(String, PathBuf)>,
) -> HashMap<String, Tracker> {
    trackers.retain(|id, _| jobs.iter().any(|(j, _)| j == id));
    for (id, cwd) in jobs {
        let t = trackers.entry(id).or_default();
        let start = *t.start.get_or_insert_with(SystemTime::now);
        scan_session(t, &cwd, start);
    }
    trackers
}

fn scan_session(t: &mut Tracker, cwd: &Path, start: SystemTime) {
    let Some(dir) = project_dir(cwd) else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    // The newest transcript written since the session began is this session's.
    let newest = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .filter(|(m, _)| *m + START_SLACK >= start)
        .max_by_key(|(m, _)| *m);
    if let Some((_, path)) = newest {
        t.files.entry(path).or_insert(0);
    }
    let paths: Vec<PathBuf> = t.files.keys().cloned().collect();
    for path in paths {
        let offset = t.files[&path];
        if let Some(new_offset) = read_new(t, &path, offset) {
            t.files.insert(path, new_offset);
        }
    }
}

/// Consume complete lines appended after `offset`; returns the new offset.
fn read_new(t: &mut Tracker, path: &Path, offset: u64) -> Option<u64> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    if len <= offset {
        return None;
    }
    f.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = Vec::new();
    f.take(len - offset).read_to_end(&mut buf).ok()?;
    let end = buf.iter().rposition(|&b| b == b'\n')? + 1;
    for line in buf[..end].split(|&b| b == b'\n') {
        if let Ok(v) = serde_json::from_slice::<Value>(line) {
            apply_line(t, &v);
        }
    }
    Some(offset + end as u64)
}

fn apply_line(t: &mut Tracker, v: &Value) {
    if v["type"] != "assistant" {
        return;
    }
    let msg = &v["message"];
    let Some(usage) = msg.get("usage").filter(|u| u.is_object()) else {
        return;
    };
    // Synthetic entries (e.g. "<synthetic>") carry no real usage.
    let Some(model) = msg["model"].as_str().filter(|m| !m.starts_with('<')) else {
        return;
    };
    if let Some(id) = msg["id"].as_str()
        && !t.seen.insert(id.to_string())
    {
        return;
    }
    let n = |k: &str| usage[k].as_u64().unwrap_or(0);
    let (input, output) = (n("input_tokens"), n("output_tokens"));
    let (cache_read, cache_write) = (
        n("cache_read_input_tokens"),
        n("cache_creation_input_tokens"),
    );
    let u = &mut t.usage;
    u.input += input;
    u.output += output;
    u.cache_read += cache_read;
    u.cache_write += cache_write;
    u.context_tokens = input + cache_read + cache_write;
    u.model = Some(model.to_string());
}
