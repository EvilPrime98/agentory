//! Deterministic inspection of a `SKILL.md` against the Claude Code skills standard.
//!
//! `analyze` is a pure function: file text plus the set of files shipped next to it in, a
//! `SkillReport` out. No model, no GPUI, no I/O.

use std::collections::{BTreeMap, BTreeSet};

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use serde_yaml_ng::{Mapping, Value};

/// Claude Code recommends keeping the body under this many lines.
pub const BODY_LINE_GUIDELINE: usize = 500;
const DESCRIPTION_MAX: usize = 1024;
const NAME_MAX: usize = 64;

/// Fields of the base Agent Skills spec.
const BASE_FIELDS: &[&str] = &[
    "name",
    "description",
    "license",
    "compatibility",
    "metadata",
    "allowed-tools",
];
/// Fields Claude Code adds on top of the base spec.
const EXTENSION_FIELDS: &[&str] = &[
    "when_to_use",
    "argument-hint",
    "disable-model-invocation",
    "user-invocable",
    "model",
    "effort",
    "context",
    "agent",
    "hooks",
    "paths",
    "shell",
];
/// Folders a code span may point into without being a link.
const BUNDLED_PREFIXES: &[&str] = &["scripts/", "references/", "assets/", "${CLAUDE_SKILL_DIR}/"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    /// 1-based line in `SKILL.md`.
    pub line: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Base,
    Extension,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub key: String,
    /// The value rendered as text.
    pub value: String,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    pub title: String,
    pub line: usize,
    /// Diagnostics whose line falls in this section.
    pub findings: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlock {
    pub lang: String,
    pub line: usize,
    pub lines: usize,
}

/// A place where the harness rewrites the body: `$ARGUMENTS`, `$0`, `${CLAUDE_SKILL_DIR}`,
/// or a `` !`cmd` `` injection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Substitution {
    pub token: String,
    pub lines: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub path: String,
    pub line: usize,
    pub exists: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub body_lines: usize,
    /// Characters / 4, the usual rough estimate.
    pub approx_tokens: usize,
}

#[derive(Debug, Clone, Default)]
pub struct SkillReport {
    pub name: Option<String>,
    pub description: Option<String>,
    pub fields: Vec<Field>,
    pub tools: Vec<String>,
    pub argument_hint: Option<String>,
    pub user_invocable: bool,
    pub model_invocable: bool,
    pub body: String,
    /// File line of the first body line.
    pub body_first_line: usize,
    pub outline: Vec<Heading>,
    pub code_blocks: Vec<CodeBlock>,
    pub substitutions: Vec<Substitution>,
    pub references: Vec<Reference>,
    pub stats: Stats,
    pub diagnostics: Vec<Diagnostic>,
}

impl SkillReport {
    pub fn count(&self, severity: Severity) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == severity)
            .count()
    }
}

struct Split<'a> {
    yaml: Option<&'a str>,
    body: &'a str,
    body_line: usize,
    unterminated: bool,
}

fn split_frontmatter(text: &str) -> Split<'_> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next().filter(|l| l.trim_end() == "---") else {
        return Split {
            yaml: None,
            body: text,
            body_line: 1,
            unterminated: false,
        };
    };
    let yaml_start = first.len();
    let mut offset = yaml_start;
    let mut line_no = 1;
    for l in lines {
        line_no += 1;
        if l.trim_end() == "---" {
            return Split {
                yaml: Some(&text[yaml_start..offset]),
                body: &text[offset + l.len()..],
                body_line: line_no + 1,
                unterminated: false,
            };
        }
        offset += l.len();
    }
    Split {
        yaml: None,
        body: text,
        body_line: 1,
        unterminated: true,
    }
}

fn render_value(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.trim().to_string(),
        other => serde_yaml_ng::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

/// File line of a top-level `key:` in the frontmatter.
fn key_line(yaml: &str, key: &str) -> Option<usize> {
    yaml.lines()
        .position(|l| {
            l.strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with(':'))
        })
        .map(|i| i + 2)
}

/// Split an `allowed-tools` string on whitespace or commas outside parentheses.
fn split_tools(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                cur.push(c);
            }
            c if (c.is_whitespace() || c == ',') && depth <= 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= NAME_MAX
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn push(
    diags: &mut Vec<Diagnostic>,
    severity: Severity,
    message: impl Into<String>,
    line: Option<usize>,
) {
    diags.push(Diagnostic {
        severity,
        message: message.into(),
        line,
    });
}

/// Inspect one `SKILL.md`. `files` holds every file shipped with the skill, as paths relative to
/// the skill folder with `/` separators.
pub fn analyze(text: &str, files: &BTreeSet<String>) -> SkillReport {
    let split = split_frontmatter(text);
    let mut report = SkillReport {
        user_invocable: true,
        model_invocable: true,
        body: split.body.to_string(),
        body_first_line: split.body_line,
        ..SkillReport::default()
    };
    let mut diags = Vec::new();

    match (split.yaml, split.unterminated) {
        (Some(yaml), _) => analyze_frontmatter(yaml, &mut report, &mut diags),
        (None, true) => push(
            &mut diags,
            Severity::Error,
            "Frontmatter is not closed: add a closing `---` line",
            Some(1),
        ),
        (None, false) => push(
            &mut diags,
            Severity::Error,
            "No frontmatter: the file must start with a `---` line",
            Some(1),
        ),
    }

    analyze_body(&mut report, files, &mut diags);

    let hint_missing = report.argument_hint.is_none();
    if hint_missing
        && report
            .substitutions
            .iter()
            .any(|s| s.token.starts_with("$ARGUMENTS"))
    {
        push(
            &mut diags,
            Severity::Info,
            "Body uses $ARGUMENTS but there is no `argument-hint`",
            None,
        );
    }

    diags.sort_by_key(|d| (d.severity, d.line));
    for h in &mut report.outline {
        h.findings = 0;
    }
    for d in &diags {
        let Some(line) = d.line else { continue };
        if let Some(h) = report.outline.iter_mut().rev().find(|h| h.line <= line) {
            h.findings += 1;
        }
    }
    report.diagnostics = diags;
    report
}

fn analyze_frontmatter(yaml: &str, report: &mut SkillReport, diags: &mut Vec<Diagnostic>) {
    let value: Value = match serde_yaml_ng::from_str(yaml) {
        Ok(v) => v,
        Err(e) => {
            let line = e.location().map(|l| l.line() + 1);
            push(diags, Severity::Error, format!("Invalid YAML: {e}"), line);
            return;
        }
    };
    let map = match value {
        Value::Null => Mapping::new(),
        Value::Mapping(m) => m,
        _ => {
            push(
                diags,
                Severity::Error,
                "Frontmatter must be a YAML mapping of `key: value` pairs",
                Some(2),
            );
            return;
        }
    };

    for (k, v) in &map {
        let Some(key) = k.as_str() else {
            push(
                diags,
                Severity::Warning,
                "Frontmatter key is not a string",
                None,
            );
            continue;
        };
        let origin = if BASE_FIELDS.contains(&key) {
            Origin::Base
        } else if EXTENSION_FIELDS.contains(&key) {
            Origin::Extension
        } else {
            push(
                diags,
                Severity::Warning,
                format!("Unknown frontmatter key `{key}`"),
                key_line(yaml, key),
            );
            Origin::Unknown
        };
        report.fields.push(Field {
            key: key.to_string(),
            value: render_value(v),
            origin,
        });
    }

    let get = |key: &str| map.get(key);

    match get("name") {
        None | Some(Value::Null) => push(
            diags,
            Severity::Info,
            "No `name`: the folder name is used",
            None,
        ),
        Some(Value::String(name)) => {
            report.name = Some(name.clone());
            if !valid_name(name) {
                push(
                    diags,
                    Severity::Error,
                    format!(
                        "Invalid `name` `{name}`: use 1-{NAME_MAX} lowercase letters, digits and hyphens"
                    ),
                    key_line(yaml, "name"),
                );
            }
        }
        Some(_) => push(
            diags,
            Severity::Error,
            "`name` must be a string",
            key_line(yaml, "name"),
        ),
    }

    match get("description") {
        Some(Value::String(d)) if !d.trim().is_empty() => {
            let d = d.trim();
            report.description = Some(d.to_string());
            let len = d.chars().count();
            if len > DESCRIPTION_MAX {
                push(
                    diags,
                    Severity::Error,
                    format!("`description` is {len} characters, the limit is {DESCRIPTION_MAX}"),
                    key_line(yaml, "description"),
                );
            }
        }
        _ => push(
            diags,
            Severity::Error,
            "Missing `description`: Claude cannot decide when to use the skill",
            get("description").and_then(|_| key_line(yaml, "description")),
        ),
    }

    match get("allowed-tools") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) => report.tools = split_tools(s),
        Some(Value::Sequence(items)) => {
            for item in items {
                match item.as_str() {
                    Some(s) => report.tools.extend(split_tools(s)),
                    None => push(
                        diags,
                        Severity::Warning,
                        "`allowed-tools` entries must be strings",
                        key_line(yaml, "allowed-tools"),
                    ),
                }
            }
        }
        Some(_) => push(
            diags,
            Severity::Warning,
            "`allowed-tools` must be a string or a list",
            key_line(yaml, "allowed-tools"),
        ),
    }
    for tool in &report.tools {
        let opens = tool.matches('(').count();
        let closes = tool.matches(')').count();
        if opens != closes || tool.starts_with('(') {
            push(
                diags,
                Severity::Warning,
                format!("`allowed-tools`: malformed entry `{tool}`"),
                key_line(yaml, "allowed-tools"),
            );
        }
    }

    for key in ["disable-model-invocation", "user-invocable"] {
        match get(key) {
            None | Some(Value::Null) => {}
            Some(Value::Bool(b)) => {
                if key == "user-invocable" {
                    report.user_invocable = *b;
                } else {
                    report.model_invocable = !*b;
                }
            }
            Some(_) => push(
                diags,
                Severity::Warning,
                format!("`{key}` must be true or false"),
                key_line(yaml, key),
            ),
        }
    }

    report.argument_hint = get("argument-hint")
        .map(render_value)
        .filter(|s| !s.is_empty());
}

/// Rewrites the harness performs on the body, one token per hit.
fn scan_line(line: &str, mut hit: impl FnMut(String)) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !line.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let rest = &line[i..];
        if bytes[i] == b'$' {
            if let Some(inner) = rest.strip_prefix("${") {
                if let Some(end) = inner.find('}') {
                    let name = &inner[..end];
                    if name.starts_with("CLAUDE_")
                        && name.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                    {
                        hit(format!("${{{name}}}"));
                        i += end + 3;
                        continue;
                    }
                }
            } else if let Some(after) = rest.strip_prefix("$ARGUMENTS") {
                let mut len = "$ARGUMENTS".len();
                if let Some(idx) = after.strip_prefix('[')
                    && let Some(end) = idx.find(']')
                    && !idx[..end].is_empty()
                    && idx[..end].chars().all(|c| c.is_ascii_digit())
                {
                    len += end + 2;
                }
                hit(rest[..len].to_string());
                i += len;
                continue;
            } else {
                let digits = rest[1..].chars().take_while(char::is_ascii_digit).count();
                if digits > 0 {
                    hit(rest[..1 + digits].to_string());
                    i += 1 + digits;
                    continue;
                }
            }
        } else if bytes[i] == b'!'
            && let Some(inner) = rest.strip_prefix("!`")
            && let Some(end) = inner.find('`')
        {
            hit(rest[..end + 3].to_string());
            i += end + 3;
            continue;
        }
        i += 1;
    }
}

/// A path relative to the skill folder, or `None` for URLs, anchors and absolute paths.
fn local_path(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let raw = raw.strip_prefix("${CLAUDE_SKILL_DIR}/").unwrap_or(raw);
    if raw.is_empty() || raw.starts_with('#') || raw.starts_with('/') || raw.contains(':') {
        return None;
    }
    let raw = raw.split(['#', '?']).next()?;
    let raw = raw.strip_prefix("./").unwrap_or(raw);
    if raw.is_empty() || raw.contains(char::is_whitespace) || raw.contains('$') {
        return None;
    }
    Some(raw.replace('\\', "/"))
}

fn exists(files: &BTreeSet<String>, path: &str) -> bool {
    if files.contains(path) {
        return true;
    }
    let dir = format!("{}/", path.trim_end_matches('/'));
    files
        .range(dir.clone()..)
        .next()
        .is_some_and(|f| f.starts_with(&dir))
}

fn analyze_body(report: &mut SkillReport, files: &BTreeSet<String>, diags: &mut Vec<Diagnostic>) {
    let first_line = report.body_first_line;
    let body = report.body.clone();

    let mut starts = vec![0];
    starts.extend(body.match_indices('\n').map(|(i, _)| i + 1));
    let line_of = |offset: usize| first_line + starts.partition_point(|&s| s <= offset) - 1;

    let body_lines = body.lines().count();
    report.stats = Stats {
        body_lines,
        approx_tokens: body.chars().count() / 4,
    };
    if body_lines > BODY_LINE_GUIDELINE {
        push(
            diags,
            Severity::Warning,
            format!(
                "Body is {body_lines} lines, the guideline is {BODY_LINE_GUIDELINE}: move detail into bundled files"
            ),
            None,
        );
    }

    let mut subs: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, line) in body.lines().enumerate() {
        let n = first_line + i;
        scan_line(line, |token| {
            let lines = subs.entry(token).or_default();
            if lines.last() != Some(&n) {
                lines.push(n);
            }
        });
    }
    report.substitutions = subs
        .into_iter()
        .map(|(token, lines)| Substitution { token, lines })
        .collect();

    let mut refs: Vec<(String, usize)> = Vec::new();
    let mut heading: Option<(u8, usize, String)> = None;
    let mut code: Option<CodeBlock> = None;
    for (event, range) in Parser::new_ext(&body, Options::empty()).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                heading = Some((level as u8, line_of(range.start), String::new()));
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, line, title)) = heading.take() {
                    report.outline.push(Heading {
                        level,
                        title: title.trim().to_string(),
                        line,
                        findings: 0,
                    });
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => {
                        l.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some(CodeBlock {
                    lang,
                    line: line_of(range.start),
                    lines: 0,
                });
            }
            Event::End(TagEnd::CodeBlock) => report.code_blocks.extend(code.take()),
            Event::Text(t) => {
                if let Some(c) = code.as_mut() {
                    c.lines += t.lines().count();
                } else if let Some((_, _, title)) = heading.as_mut() {
                    title.push_str(&t);
                }
            }
            Event::Code(c) => {
                if let Some((_, _, title)) = heading.as_mut() {
                    title.push_str(&c);
                }
                if BUNDLED_PREFIXES.iter().any(|p| c.starts_with(p))
                    && let Some(p) = local_path(&c)
                {
                    refs.push((p, line_of(range.start)));
                }
            }
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                if let Some(p) = local_path(&dest_url) {
                    refs.push((p, line_of(range.start)));
                }
            }
            _ => {}
        }
    }

    let mut seen = BTreeSet::new();
    for (path, line) in refs {
        if !seen.insert((path.clone(), line)) {
            continue;
        }
        let found = exists(files, &path);
        if !found {
            push(
                diags,
                Severity::Warning,
                format!("Referenced file not found: {path}"),
                Some(line),
            );
        }
        report.references.push(Reference {
            path,
            line,
            exists: found,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    const VALID: &str = "---\nname: pdf-tools\ndescription: Work with PDF files\nallowed-tools: Read Bash(git add:*), Grep\nargument-hint: <file>\nuser-invocable: true\n---\n# PDF tools\n\nRun `scripts/extract.py` on $ARGUMENTS.\n\n## Details\n\nSee [the guide](references/guide.md) and ${CLAUDE_SKILL_DIR}/assets/logo.png.\n\n```bash\npython scripts/extract.py $0\n```\n\nBranch: !`git branch --show-current`\n";

    fn valid_files() -> BTreeSet<String> {
        files(&[
            "SKILL.md",
            "scripts/extract.py",
            "references/guide.md",
            "assets/logo.png",
        ])
    }

    #[test]
    fn valid_skill_has_no_errors_or_warnings() {
        let r = analyze(VALID, &valid_files());
        assert_eq!(r.diagnostics, vec![], "{:?}", r.diagnostics);
        assert_eq!(r.name.as_deref(), Some("pdf-tools"));
        assert_eq!(r.tools, vec!["Read", "Bash(git add:*)", "Grep"]);
        assert_eq!(r.argument_hint.as_deref(), Some("<file>"));
        assert!(r.user_invocable && r.model_invocable);
        assert_eq!(
            r.fields
                .iter()
                .filter(|f| f.origin == Origin::Extension)
                .count(),
            2
        );
    }

    #[test]
    fn outline_code_blocks_and_stats() {
        let r = analyze(VALID, &valid_files());
        let titles: Vec<_> = r
            .outline
            .iter()
            .map(|h| (h.level, h.title.as_str(), h.line))
            .collect();
        assert_eq!(titles, vec![(1, "PDF tools", 8), (2, "Details", 12)]);
        assert_eq!(r.code_blocks.len(), 1);
        assert_eq!(r.code_blocks[0].lang, "bash");
        assert_eq!(r.code_blocks[0].lines, 1);
        assert_eq!(r.stats.body_lines, r.body.lines().count());
    }

    #[test]
    fn substitutions_record_their_lines() {
        let r = analyze(VALID, &valid_files());
        let line_of = |t: &str| {
            r.substitutions
                .iter()
                .find(|s| s.token == t)
                .map(|s| s.lines.clone())
        };
        assert_eq!(line_of("$ARGUMENTS"), Some(vec![10]));
        assert_eq!(line_of("$0"), Some(vec![17]));
        assert_eq!(line_of("${CLAUDE_SKILL_DIR}"), Some(vec![14]));
        assert_eq!(line_of("!`git branch --show-current`"), Some(vec![20]));
    }

    #[test]
    fn bad_yaml_is_an_error_with_a_line() {
        let r = analyze(
            "---\nname: ok\ndescription: [unclosed\n---\nbody\n",
            &BTreeSet::new(),
        );
        let d = &r.diagnostics[0];
        assert_eq!(d.severity, Severity::Error);
        assert!(d.message.starts_with("Invalid YAML"));
        assert!(d.line.is_some());
        assert_eq!(r.body.trim(), "body");
    }

    #[test]
    fn missing_and_invalid_required_fields() {
        let r = analyze("---\nname: Bad Name\n---\nbody\n", &BTreeSet::new());
        let errors: Vec<_> = r
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors.iter().any(|m| m.contains("Invalid `name`")));
        assert!(errors.iter().any(|m| m.contains("Missing `description`")));
    }

    #[test]
    fn long_description_is_an_error() {
        let text = format!(
            "---\nname: a\ndescription: {}\n---\nbody\n",
            "x".repeat(1025)
        );
        let r = analyze(&text, &BTreeSet::new());
        assert_eq!(r.count(Severity::Error), 1);
    }

    #[test]
    fn missing_frontmatter_and_unclosed_frontmatter() {
        let r = analyze("# just a body\n", &BTreeSet::new());
        assert_eq!(r.count(Severity::Error), 1);
        assert_eq!(r.outline.len(), 1);
        let r = analyze("---\nname: a\n", &BTreeSet::new());
        assert!(r.diagnostics[0].message.contains("not closed"));
    }

    #[test]
    fn broken_references_warn_and_flag_their_section() {
        let text = "---\nname: a\ndescription: d\n---\n# Top\n\nSee [x](references/gone.md) and `scripts/run.sh`.\n";
        let r = analyze(text, &files(&["scripts/run.sh"]));
        let missing: Vec<_> = r.references.iter().filter(|x| !x.exists).collect();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].path, "references/gone.md");
        assert_eq!(missing[0].line, 7);
        assert_eq!(r.count(Severity::Warning), 1);
        assert_eq!(r.outline[0].findings, 1);
    }

    #[test]
    fn urls_anchors_and_folders_are_not_missing() {
        let text =
            "---\nname: a\ndescription: d\n---\n[a](https://x.dev) [b](#top) [c](references/)\n";
        let r = analyze(text, &files(&["references/one.md"]));
        assert_eq!(r.references.len(), 1);
        assert!(r.references[0].exists);
        assert_eq!(r.diagnostics, vec![]);
    }

    #[test]
    fn unknown_key_warns_and_extension_keys_do_not() {
        let text = "---\nname: a\ndescription: d\nwhen_to_use: now\nflavour: mint\n---\nbody\n";
        let r = analyze(text, &BTreeSet::new());
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].severity, Severity::Warning);
        assert_eq!(r.diagnostics[0].line, Some(5));
        assert!(
            r.fields
                .iter()
                .any(|f| f.key == "flavour" && f.origin == Origin::Unknown)
        );
    }

    #[test]
    fn bad_allowed_tools_and_flags() {
        let text = "---\nname: a\ndescription: d\nallowed-tools: Bash(git add\ndisable-model-invocation: yes please\nuser-invocable: false\n---\nbody\n";
        let r = analyze(text, &BTreeSet::new());
        assert_eq!(r.count(Severity::Warning), 2, "{:?}", r.diagnostics);
        assert!(!r.user_invocable);
    }

    #[test]
    fn long_body_warns() {
        let body = "line\n".repeat(BODY_LINE_GUIDELINE + 1);
        let r = analyze(
            &format!("---\nname: a\ndescription: d\n---\n{body}"),
            &BTreeSet::new(),
        );
        assert_eq!(r.count(Severity::Warning), 1);
    }

    #[test]
    fn arguments_without_hint_is_info() {
        let r = analyze(
            "---\nname: a\ndescription: d\n---\nUse $ARGUMENTS\n",
            &BTreeSet::new(),
        );
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].severity, Severity::Info);
    }

    #[test]
    fn crlf_files_parse() {
        let r = analyze(
            "---\r\nname: a\r\ndescription: d\r\n---\r\n# T\r\n",
            &BTreeSet::new(),
        );
        assert_eq!(r.diagnostics, vec![]);
        assert_eq!(r.outline[0].line, 5);
    }
}
