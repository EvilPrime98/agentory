//! Skill markets. Only SkillsMP (<https://skillsmp.com>) through its keyword-search REST API, as
//! in the Node CLI. Anonymous use is limited to 50 requests a day; `SKILLSMP_API_KEY` raises it.

use std::collections::HashSet;
use std::time::Duration;

use serde_json::Value;

pub const SKILLSMP: &str = "skillsmp";
const ENDPOINT: &str = "https://skillsmp.com/api/v1/skills/search";

/// A skill found on a market.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub name: String,
    pub author: String,
    pub description: String,
    /// Repo spec accepted by `parse_repo`, including the subdirectory.
    pub repo: String,
    pub stars: u64,
}

impl Hit {
    /// The reference to install it with: `author/name`, so same-named skills stay apart.
    pub fn id(&self) -> String {
        if self.author.is_empty() {
            self.name.clone()
        } else {
            format!("{}/{}", self.author, self.name)
        }
    }
}

/// Splits `<market>:<id>`; `None` unless it starts with a known market id.
pub fn parse_ref(arg: &str) -> Option<(&str, &str)> {
    let (market, id) = arg.split_once(':')?;
    (market == SKILLSMP && !id.is_empty()).then_some((market, id))
}

fn is_github(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://github.com/") else {
        return false;
    };
    let mut parts = rest.splitn(3, '/');
    let ok = |p: Option<&str>| {
        p.is_some_and(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        })
    };
    ok(parts.next()) && ok(parts.next())
}

/// Hits of a search response: only GitHub sources (anything else would not go through the normal
/// clone path), translated copies of one skill collapsed.
pub fn hits_from_body(body: &Value) -> Result<Vec<Hit>, String> {
    if body.get("success").and_then(Value::as_bool) != Some(true) {
        let msg = body["error"]["message"]
            .as_str()
            .unwrap_or("unexpected response");
        return Err(format!("skillsmp: {msg}"));
    }
    let skills = body["data"]["skills"]
        .as_array()
        .ok_or("skillsmp: unexpected response")?;
    let mut seen = HashSet::new();
    Ok(skills
        .iter()
        .filter_map(|s| {
            let repo = s["githubUrl"].as_str().filter(|u| is_github(u))?;
            let text = |k: &str| s[k].as_str().unwrap_or_default().to_string();
            Some(Hit {
                name: text("name"),
                author: text("author"),
                description: text("description"),
                repo: repo.to_string(),
                stars: s["stars"].as_u64().unwrap_or(0),
            })
        })
        .filter(|h| seen.insert(format!("{}/{}/{}", h.author, h.name, h.repo)))
        .collect())
}

fn query(term: &str, limit: u32) -> Result<Value, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("agentory (spm-compatible)")
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let send = || {
        let mut req = client
            .get(ENDPOINT)
            .query(&[("q", term), ("limit", limit.to_string().as_str())]);
        if let Ok(key) = std::env::var("SKILLSMP_API_KEY")
            && !key.is_empty()
        {
            req = req.bearer_auth(key);
        }
        req.send().map_err(|e| format!("skillsmp: {e}"))
    };
    let mut res = send()?;
    // Cloudflare challenges the first request on a fresh connection; the retry goes through.
    if res.status() == 403 {
        res = send()?;
    }
    if res.status() == 429 {
        let wait = res
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(|w| format!(", retry in {w}s"))
            .unwrap_or_default();
        return Err(format!(
            "skillsmp rate limit reached{wait} (set SKILLSMP_API_KEY for a higher quota)"
        ));
    }
    let status = res.status();
    res.json::<Value>()
        .map_err(|_| format!("skillsmp: HTTP {status}"))
}

/// Search the market by keyword.
pub fn search(term: &str) -> Result<Vec<Hit>, String> {
    hits_from_body(&query(term, 20)?)
}

/// Pick the single hit for `id` (`name` or `author/name`) among search results.
pub fn pick(id: &str, hits: Vec<Hit>) -> Result<Hit, String> {
    let (author, name) = match id.split_once('/') {
        Some((a, n)) => (Some(a), n),
        None => (None, id),
    };
    let mut found: Vec<Hit> = hits
        .into_iter()
        .filter(|h| h.name == name && author.is_none_or(|a| h.author == a))
        .collect();
    match found.len() {
        0 => Err(format!("skillsmp: no skill \"{id}\" found")),
        1 => Ok(found.remove(0)),
        _ => Err(format!(
            "skillsmp: \"{id}\" is ambiguous, pick one of: {}",
            found
                .iter()
                .map(|h| format!("skillsmp:{}", h.id()))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Resolve a market reference to a single hit, through the network.
pub fn resolve(id: &str) -> Result<Hit, String> {
    let name = id.split_once('/').map_or(id, |(_, n)| n);
    pick(id, search(name)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body() -> Value {
        json!({"success": true, "data": {"skills": [
            {"name": "pdf", "author": "a", "description": "d", "githubUrl": "https://github.com/a/r/tree/main/pdf", "stars": 5},
            {"name": "pdf", "author": "a", "description": "translated", "githubUrl": "https://github.com/a/r/tree/main/pdf", "stars": 5},
            {"name": "pdf", "author": "b", "githubUrl": "https://github.com/b/r"},
            {"name": "evil", "author": "c", "githubUrl": "https://example.com/c/r"}
        ]}})
    }

    #[test]
    fn parses_hits_like_the_node_cli() {
        let hits = hits_from_body(&body()).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id(), "a/pdf");
        assert_eq!(hits[0].stars, 5);
        let err = hits_from_body(&json!({"success": false, "error": {"message": "nope"}}));
        assert!(err.unwrap_err().contains("nope"));
    }

    #[test]
    fn picks_a_single_hit() {
        let hits = hits_from_body(&body()).unwrap();
        assert!(pick("pdf", hits.clone()).unwrap_err().contains("ambiguous"));
        assert_eq!(pick("b/pdf", hits.clone()).unwrap().author, "b");
        assert!(pick("zip", hits).unwrap_err().contains("no skill"));
    }

    #[test]
    fn refs_need_a_known_market() {
        assert_eq!(parse_ref("skillsmp:a/pdf"), Some(("skillsmp", "a/pdf")));
        assert_eq!(parse_ref("other:pdf"), None);
        assert_eq!(parse_ref("skillsmp:"), None);
        assert_eq!(parse_ref("pdf"), None);
    }
}
