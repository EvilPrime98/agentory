//! Claude Code's own session transcripts (`~/.claude/projects/<project>/<uuid>.jsonl`), read-only,
//! plus the sidecar that holds the titles the user gave them in Agentory.
//!
//! Only the head and the tail of each file are parsed; the message count is a streaming newline
//! count. Results are cached by path and mtime so later visits skip unchanged files.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

/// How much of each end of a transcript is parsed.
const WINDOW: u64 = 64 * 1024;
const MAX_TITLE_CHARS: usize = 80;

/// One session found on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionMeta {
    /// Claude's session id: the file stem, as `claude --resume` takes it.
    pub id: String,
    /// Where the session ran. `None` when the transcript never recorded it.
    pub cwd: Option<PathBuf>,
    pub title: String,
    /// The first real user prompt (searched together with the title).
    pub first_prompt: String,
    pub modified_ms: i64,
    pub messages: usize,
}

/// What the head and tail of a transcript say.
#[derive(Debug, Default, PartialEq)]
struct Parsed {
    /// Claude's custom title or summary; the newest one wins.
    title: Option<String>,
    first_prompt: Option<String>,
    cwd: Option<String>,
}

fn truncate(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = one_line.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{}…", head.trim_end())
    } else {
        head
    }
}

/// The prompt text of a `user` entry, `None` for tool results, meta entries and the
/// `<command-…>` / `<local-command-…>` wrappers Claude writes around slash commands.
fn prompt_text(entry: &Value) -> Option<String> {
    if entry.get("isMeta").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let content = entry.pointer("/message/content")?;
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty() && !text.starts_with('<')).then(|| text.to_string())
}

/// Fold the JSON lines of `chunk` into `out`. Corrupt lines, and the partial line a window can
/// start in the middle of, are skipped.
fn parse_chunk(chunk: &str, out: &mut Parsed) {
    for line in chunk.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if out.cwd.is_none() {
            out.cwd = entry
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
                .map(str::to_string);
        }
        match entry.get("type").and_then(Value::as_str) {
            Some("custom-title") => {
                if let Some(t) = entry.get("customTitle").and_then(Value::as_str) {
                    out.title = Some(t.to_string());
                }
            }
            Some("summary") => {
                // A custom title outranks a summary, whatever the order in the file.
                if out.title.is_none() {
                    out.title = entry
                        .get("summary")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
            }
            Some("user") if out.first_prompt.is_none() => out.first_prompt = prompt_text(&entry),
            _ => {}
        }
    }
}

/// Parse the head and the tail text of a transcript. The tail comes first-class for titles
/// because Claude appends summaries and renames at the end.
fn parse_ends(head: &str, tail: &str) -> Parsed {
    let mut out = Parsed::default();
    parse_chunk(head, &mut out);
    let mut tail_parsed = Parsed::default();
    // Drop the first line of a tail window: it usually starts mid-line.
    let tail = tail.split_once('\n').map_or("", |(_, rest)| rest);
    parse_chunk(tail, &mut tail_parsed);
    Parsed {
        title: tail_parsed.title.or(out.title),
        first_prompt: out.first_prompt.or(tail_parsed.first_prompt),
        cwd: out.cwd.or(tail_parsed.cwd),
    }
}

fn read_window(file: &mut File, from: u64, len: u64) -> String {
    let mut buf = Vec::new();
    if file.seek(SeekFrom::Start(from)).is_ok() {
        let _ = file.take(len).read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Newlines in the file, streamed in blocks.
fn count_lines(file: &mut File) -> usize {
    let mut buf = vec![0u8; 256 * 1024];
    let mut lines = 0;
    let mut last = b'\n';
    if file.seek(SeekFrom::Start(0)).is_err() {
        return 0;
    }
    while let Ok(n) = file.read(&mut buf) {
        if n == 0 {
            break;
        }
        lines += buf[..n].iter().filter(|&&b| b == b'\n').count();
        last = buf[n - 1];
    }
    // A last line without a trailing newline still counts.
    lines + usize::from(last != b'\n')
}

fn mtime_ms(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

fn read_meta(path: &Path, modified_ms: i64) -> Option<SessionMeta> {
    let id = path.file_stem()?.to_str()?.to_string();
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let head = read_window(&mut file, 0, WINDOW);
    let tail = if size > WINDOW {
        read_window(&mut file, size - WINDOW, WINDOW)
    } else {
        String::new()
    };
    let parsed = parse_ends(&head, &tail);
    let messages = count_lines(&mut file);
    let first_prompt = parsed.first_prompt.unwrap_or_default();
    let title = parsed
        .title
        .map(|t| truncate(&t, MAX_TITLE_CHARS))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| truncate(&first_prompt, MAX_TITLE_CHARS));
    Some(SessionMeta {
        id,
        cwd: parsed.cwd.map(PathBuf::from),
        title,
        first_prompt: truncate(&first_prompt, 400),
        modified_ms,
        messages,
    })
}

/// Claude's data folder: `SPM_CLAUDE_DIR` (as the rest of the app honours it) or `~/.claude`.
fn claude_dir() -> Option<PathBuf> {
    std::env::var_os("SPM_CLAUDE_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".claude")))
}

/// A transcript file with its mtime.
#[derive(Debug, Clone)]
pub struct Entry {
    path: PathBuf,
    modified_ms: i64,
}

/// Every `*.jsonl` of every project folder, newest first. Cheap: no file is opened.
pub fn list() -> Vec<Entry> {
    let Some(root) = claude_dir().map(|d| d.join("projects")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for dir in fs::read_dir(root).into_iter().flatten().flatten() {
        for file in fs::read_dir(dir.path()).into_iter().flatten().flatten() {
            let path = file.path();
            if path.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            if let Ok(meta) = file.metadata() {
                out.push(Entry {
                    path,
                    modified_ms: mtime_ms(&meta),
                });
            }
        }
    }
    out.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    out
}

type Cache = Mutex<HashMap<PathBuf, (i64, Option<SessionMeta>)>>;

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The metadata of `entries`, from the cache when the mtime is unchanged. Unreadable files and
/// empty transcripts (no message at all) are left out.
pub fn load(entries: &[Entry]) -> Vec<SessionMeta> {
    entries
        .iter()
        .filter_map(|e| {
            let cached = cache()
                .lock()
                .ok()
                .and_then(|c| c.get(&e.path).filter(|(m, _)| *m == e.modified_ms).cloned());
            let meta = match cached {
                Some((_, meta)) => meta,
                None => {
                    let meta = read_meta(&e.path, e.modified_ms);
                    if let Ok(mut c) = cache().lock() {
                        c.insert(e.path.clone(), (e.modified_ms, meta.clone()));
                    }
                    meta
                }
            };
            meta.filter(|m| m.messages > 0)
        })
        .collect()
}

/// Whether two paths name the same folder, ignoring case and separator style (Windows).
pub fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// Relative age for a card: `(unit, n)` with unit one of `now`, `minute`, `hour`, `day`, `month`.
pub fn age(modified_ms: i64, now_ms: i64) -> (&'static str, i64) {
    let secs = ((now_ms - modified_ms) / 1000).max(0);
    match secs {
        0..=59 => ("now", 0),
        60..=3599 => ("minute", secs / 60),
        3600..=86_399 => ("hour", secs / 3600),
        86_400..=2_591_999 => ("day", secs / 86_400),
        _ => ("month", secs / 2_592_000),
    }
}

// ---------------------------------------------------------------------------------------------
// Rename sidecar
// ---------------------------------------------------------------------------------------------

fn titles_file() -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join("Agentory")
            .join("session_titles.json"),
    )
}

/// Session id → the title given in Agentory.
pub fn read_titles() -> HashMap<String, String> {
    titles_file()
        .and_then(|p| fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Store (or, with a blank `title`, clear) the title of a session.
pub fn write_title(id: &str, title: &str) -> std::io::Result<()> {
    let Some(path) = titles_file() else {
        return Ok(());
    };
    let mut titles = read_titles();
    let title = title.trim();
    if title.is_empty() {
        titles.remove(id);
    } else {
        titles.insert(id.to_string(), truncate(title, MAX_TITLE_CHARS));
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, serde_json::to_vec_pretty(&titles)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> String {
        format!(
            r#"{{"type":"user","cwd":"C:\\p","message":{{"role":"user","content":{}}}}}"#,
            serde_json::to_string(text).unwrap()
        )
    }

    #[test]
    fn first_prompt_skips_meta_and_command_wrappers() {
        let text = [
            r#"{"type":"user","isMeta":true,"message":{"content":"caveat"}}"#.to_string(),
            user("<command-name>/clear</command-name>"),
            user("fix the login bug"),
            user("second"),
        ]
        .join("\n");
        let p = parse_ends(&text, "");
        assert_eq!(p.first_prompt.as_deref(), Some("fix the login bug"));
        assert_eq!(p.cwd.as_deref(), Some(r"C:\p"));
    }

    #[test]
    fn block_content_and_tool_results() {
        let blocks = r#"{"type":"user","message":{"content":[{"type":"text","text":"hello"},{"type":"text","text":"there"}]}}"#;
        let tool =
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"x"}]}}"#;
        let p = parse_ends(&format!("{tool}\n{blocks}"), "");
        assert_eq!(p.first_prompt.as_deref(), Some("hello there"));
    }

    #[test]
    fn title_prefers_custom_over_summary_and_tail_over_head() {
        let head = format!(
            "{}\n{}",
            r#"{"type":"summary","summary":"old summary"}"#,
            user("prompt")
        );
        let tail = format!(
            "partial line\n{}\n{}",
            r#"{"type":"summary","summary":"new summary"}"#,
            r#"{"type":"custom-title","customTitle":"Mine"}"#
        );
        assert_eq!(parse_ends(&head, &tail).title.as_deref(), Some("Mine"));
        let only_summaries = format!(
            "partial\n{}",
            r#"{"type":"summary","summary":"new summary"}"#
        );
        assert_eq!(
            parse_ends(&head, &only_summaries).title.as_deref(),
            Some("new summary")
        );
        assert_eq!(parse_ends(&head, "").title.as_deref(), Some("old summary"));
    }

    #[test]
    fn missing_fields_and_corrupt_lines() {
        let p = parse_ends("not json\n{\"type\":\"assistant\"}\n{broken", "");
        assert_eq!(p, Parsed::default());
    }

    #[test]
    fn truncation_flattens_whitespace() {
        assert_eq!(truncate("a\n  b\tc", 10), "a b c");
        let long = truncate(&"word ".repeat(50), 20);
        assert!(long.ends_with('…'));
        assert!(long.chars().count() <= 21);
    }

    #[test]
    fn read_meta_falls_back_to_the_first_prompt() {
        let dir = std::env::temp_dir().join(format!("agentory-tr-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("abc-123.jsonl");
        fs::write(
            &path,
            format!("{}\n{}\n", user("do the thing"), user("more")),
        )
        .unwrap();
        let meta = read_meta(&path, 5).unwrap();
        assert_eq!(meta.id, "abc-123");
        assert_eq!(meta.title, "do the thing");
        assert_eq!(meta.messages, 2);
        assert_eq!(meta.cwd, Some(PathBuf::from(r"C:\p")));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn line_count_includes_an_unterminated_last_line() {
        let path = std::env::temp_dir().join(format!("agentory-lines-{}", std::process::id()));
        fs::write(&path, "a\nb\nc").unwrap();
        assert_eq!(count_lines(&mut File::open(&path).unwrap()), 3);
        fs::remove_file(&path).ok();
    }

    #[test]
    fn same_dir_ignores_case_and_separators() {
        assert!(same_dir(
            Path::new(r"C:\Users\A\p"),
            Path::new("c:/users/a/p/")
        ));
        assert!(!same_dir(Path::new("C:/a"), Path::new("C:/b")));
    }

    #[test]
    fn ages() {
        assert_eq!(age(0, 30_000), ("now", 0));
        assert_eq!(age(0, 5 * 60_000), ("minute", 5));
        assert_eq!(age(0, 3 * 3_600_000), ("hour", 3));
        assert_eq!(age(0, 2 * 86_400_000), ("day", 2));
        assert_eq!(age(0, 70 * 86_400_000), ("month", 2));
    }
}
