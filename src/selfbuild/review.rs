//! Reading a diff for review: the per-file summary and the flags for changes that could weaken
//! the gate. The human review is the defense against a weakened gate (`docs/self_built.md`), so
//! this errs on the side of flagging too much.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FlagKind {
    /// Test files, or lines that add, remove or change tests and assertions.
    Tests,
    /// The ready marker the smoke launch relies on.
    ReadyMarker,
    /// The build and gate code itself.
    Gate,
    /// Cargo manifests, the build script, toolchain and CI configuration.
    BuildConfig,
}

impl FlagKind {
    pub fn label_key(self) -> &'static str {
        match self {
            FlagKind::Tests => "review.flag.tests",
            FlagKind::ReadyMarker => "review.flag.ready",
            FlagKind::Gate => "review.flag.gate",
            FlagKind::BuildConfig => "review.flag.build",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flag {
    pub kind: FlagKind,
    pub file: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub added: usize,
    pub removed: usize,
}

/// One file's part of a unified diff.
struct Section<'a> {
    path: String,
    lines: Vec<&'a str>,
}

fn sections(diff: &str) -> Vec<Section<'_>> {
    let mut out: Vec<Section> = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // `a/<path> b/<path>`; the b side is the file as it is after the change.
            let path = rest
                .rsplit_once(" b/")
                .map_or(rest, |(_, b)| b)
                .trim_matches('"');
            out.push(Section {
                path: path.to_string(),
                lines: Vec::new(),
            });
        } else if let Some(s) = out.last_mut() {
            s.lines.push(line);
        }
    }
    out
}

/// Added and removed content lines of a section (headers excluded).
fn changed<'a>(s: &'a Section<'_>) -> impl Iterator<Item = (char, &'a str)> {
    s.lines.iter().filter_map(|l| {
        if l.starts_with("+++") || l.starts_with("---") {
            return None;
        }
        let c = l.chars().next()?;
        matches!(c, '+' | '-').then(|| (c, &l[1..]))
    })
}

pub fn files(diff: &str) -> Vec<FileChange> {
    sections(diff)
        .iter()
        .map(|s| {
            let (mut added, mut removed) = (0, 0);
            for (c, _) in changed(s) {
                if c == '+' {
                    added += 1;
                } else {
                    removed += 1;
                }
            }
            FileChange {
                path: s.path.clone(),
                added,
                removed,
            }
        })
        .collect()
}

const TEST_MARKERS: &[&str] = &[
    "#[test]",
    "#[cfg(test)]",
    "#[tokio::test",
    "#[ignore",
    "#[should_panic",
    "mod tests",
    "assert!",
    "assert_eq!",
    "assert_ne!",
    "assert_matches!",
];
const READY_MARKERS: &[&str] = &[
    "READY_ENV",
    "AGENTORY_READY_FILE",
    "ready::",
    "--smoke",
    "BUILD_ID",
    "SMOKE_SECS",
];

/// `line` contains `needle` not as the tail of a longer name (`debug_assert!` is not `assert!`).
fn mentions(line: &str, needle: &str) -> bool {
    line.match_indices(needle).any(|(i, _)| {
        !line[..i]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

fn is_test_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.split('/').any(|c| c == "tests")
        || name == "tests.rs"
        || name.ends_with("_test.rs")
        || name.ends_with("_tests.rs")
}

fn is_build_config(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(
        name,
        "Cargo.toml"
            | "Cargo.lock"
            | "build.rs"
            | "rust-toolchain"
            | "rust-toolchain.toml"
            | "deny.toml"
    ) || path.starts_with(".cargo/")
        || path.starts_with(".github/")
}

/// Every reason to look closely at this diff, one entry per kind and file, most dangerous kinds
/// first.
pub fn flags(diff: &str) -> Vec<Flag> {
    let mut out: Vec<Flag> = Vec::new();
    let mut add = |kind, file: &str| {
        let flag = Flag {
            kind,
            file: file.to_string(),
        };
        if !out.contains(&flag) {
            out.push(flag);
        }
    };
    for s in sections(diff) {
        let touches =
            |needles: &[&str]| changed(&s).any(|(_, l)| needles.iter().any(|n| mentions(l, n)));
        if is_test_path(&s.path) || touches(TEST_MARKERS) {
            add(FlagKind::Tests, &s.path);
        }
        if s.path == "src/selfbuild/ready.rs" || touches(READY_MARKERS) {
            add(FlagKind::ReadyMarker, &s.path);
        }
        if s.path.starts_with("src/selfbuild/") {
            add(FlagKind::Gate, &s.path);
        }
        if is_build_config(&s.path) {
            add(FlagKind::BuildConfig, &s.path);
        }
    }
    out.sort_by_key(|f| f.kind);
    out
}

/// Conflict markers committed into the change: a "resolution" that did not resolve anything.
pub fn has_conflict_markers(diff: &str) -> bool {
    diff.lines()
        .any(|l| l.starts_with("+<<<<<<< ") || l.starts_with("+>>>>>>> ") || l == "+=======")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, body: &str) -> String {
        format!(
            "diff --git a/{path} b/{path}\nindex 1..2 100644\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n{body}\n"
        )
    }

    fn kinds(diff: &str) -> Vec<FlagKind> {
        flags(diff).into_iter().map(|f| f.kind).collect()
    }

    #[test]
    fn summarizes_files() {
        let diff = file("src/a.rs", "-old\n+new\n+more") + &file("src/b.rs", "+x");
        assert_eq!(
            files(&diff),
            [
                FileChange {
                    path: "src/a.rs".into(),
                    added: 2,
                    removed: 1
                },
                FileChange {
                    path: "src/b.rs".into(),
                    added: 1,
                    removed: 0
                },
            ]
        );
    }

    #[test]
    fn header_lines_are_not_counted() {
        let f = &files(&file("src/a.rs", " ctx"))[0];
        assert_eq!((f.added, f.removed), (0, 0));
    }

    #[test]
    fn plain_change_has_no_flags() {
        assert!(flags(&file("src/ui/theme.rs", "-let a = 1;\n+let a = 2;")).is_empty());
    }

    #[test]
    fn test_files_and_test_lines_are_flagged() {
        assert_eq!(
            kinds(&file("agentory-helper/tests/swap.rs", "+x")),
            [FlagKind::Tests]
        );
        assert_eq!(
            kinds(&file("src/ui/theme.rs", "-    #[test]")),
            [FlagKind::Tests]
        );
        assert_eq!(
            kinds(&file(
                "src/ui/theme.rs",
                "-    assert_eq!(a, 1);\n+    assert_eq!(a, 2);"
            )),
            [FlagKind::Tests]
        );
        assert_eq!(kinds(&file("src/x.rs", "+#[ignore]")), [FlagKind::Tests]);
    }

    #[test]
    fn debug_assert_is_not_a_test() {
        assert!(flags(&file("src/x.rs", "+debug_assert!(a);")).is_empty());
    }

    #[test]
    fn ready_marker_code_is_flagged_by_path_and_content() {
        assert!(kinds(&file("src/selfbuild/ready.rs", "+x")).contains(&FlagKind::ReadyMarker));
        assert_eq!(
            kinds(&file(
                "src/app.rs",
                "-        crate::selfbuild::ready::on_frame();"
            )),
            [FlagKind::ReadyMarker]
        );
        assert_eq!(
            kinds(&file("src/main.rs", "+    Some(0)  // --smoke")),
            [FlagKind::ReadyMarker]
        );
    }

    #[test]
    fn gate_code_is_flagged() {
        assert!(kinds(&file("src/selfbuild/gate.rs", "+x")).contains(&FlagKind::Gate));
        assert!(kinds(&file("src/selfbuild/build.rs", "+x")).contains(&FlagKind::Gate));
    }

    #[test]
    fn build_config_is_flagged() {
        for p in [
            "Cargo.toml",
            "Cargo.lock",
            "build.rs",
            ".cargo/config.toml",
            "rust-toolchain.toml",
            ".github/workflows/ci.yml",
        ] {
            assert_eq!(kinds(&file(p, "+x")), [FlagKind::BuildConfig], "{p}");
        }
        // Only the real file names count, not look-alikes.
        assert!(flags(&file("src/ui/mybuild.rs", "+x")).is_empty());
    }

    #[test]
    fn one_flag_per_kind_and_file_most_dangerous_first() {
        let diff = file("Cargo.toml", "+a\n+b") + &file("src/selfbuild/gate.rs", "-#[test]\n+x");
        let f = flags(&diff);
        assert_eq!(f.iter().filter(|f| f.kind == FlagKind::Tests).count(), 1);
        assert!(f.windows(2).all(|w| w[0].kind <= w[1].kind));
        assert_eq!(f.first().unwrap().kind, FlagKind::Tests);
        assert_eq!(f.last().unwrap().kind, FlagKind::BuildConfig);
    }

    #[test]
    fn detects_committed_conflict_markers() {
        assert!(has_conflict_markers(&file(
            "a.rs",
            "+<<<<<<< HEAD\n+x\n+=======\n+y\n+>>>>>>> user"
        )));
        assert!(!has_conflict_markers(&file("a.rs", "+let eq = a == b;")));
    }
}
