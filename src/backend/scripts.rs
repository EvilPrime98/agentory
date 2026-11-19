use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedCommand {
    pub label: String,
    pub command: String,
}

fn cmd(label: impl Into<String>, command: impl Into<String>) -> DetectedCommand {
    DetectedCommand {
        label: label.into(),
        command: command.into(),
    }
}

pub fn detect(root: &Path) -> Vec<DetectedCommand> {
    let mut out = Vec::new();
    node(root, &mut out);
    if root.join("Cargo.toml").is_file() {
        for sub in ["build", "run", "test"] {
            out.push(cmd(format!("cargo {sub}"), format!("cargo {sub}")));
        }
    }
    python(root, &mut out);
    if root.join("go.mod").is_file() {
        out.push(cmd("go build", "go build ./..."));
        out.push(cmd("go run", "go run ."));
        out.push(cmd("go test", "go test ./..."));
    }
    make(root, &mut out);
    if [
        "docker-compose.yml",
        "docker-compose.yaml",
        "compose.yml",
        "compose.yaml",
    ]
    .iter()
    .any(|f| root.join(f).is_file())
    {
        for sub in ["up", "down", "build"] {
            out.push(cmd(
                format!("docker compose {sub}"),
                format!("docker compose {sub}"),
            ));
        }
    }
    out
}

fn read(root: &Path, file: &str) -> Option<String> {
    std::fs::read_to_string(root.join(file)).ok()
}

fn node(root: &Path, out: &mut Vec<DetectedCommand>) {
    let Some(text) = read(root, "package.json") else {
        return;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        tracing::warn!("could not parse package.json in {}", root.display());
        return;
    };
    let Some(scripts) = json.get("scripts").and_then(|s| s.as_object()) else {
        return;
    };
    let manager = package_manager(root);
    for name in scripts.keys() {
        let command = match manager {
            "yarn" => format!("yarn {name}"),
            m => format!("{m} run {name}"),
        };
        out.push(cmd(format!("{manager} {name}"), command));
    }
}

fn package_manager(root: &Path) -> &'static str {
    let has = |f: &str| root.join(f).is_file();
    if has("pnpm-lock.yaml") {
        "pnpm"
    } else if has("yarn.lock") {
        "yarn"
    } else if has("bun.lockb") || has("bun.lock") {
        "bun"
    } else {
        "npm"
    }
}

fn python(root: &Path, out: &mut Vec<DetectedCommand>) {
    let Some(text) = read(root, "pyproject.toml") else {
        return;
    };
    let mut in_scripts = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_scripts = matches!(line, "[project.scripts]" | "[tool.poetry.scripts]");
        } else if in_scripts {
            if let Some((name, _)) = line.split_once('=') {
                let name = name.trim().trim_matches(['"', '\'']);
                if !name.is_empty() && !name.starts_with('#') {
                    out.push(cmd(name, name));
                }
            }
        }
    }
    if text.contains("[tool.pytest") {
        out.push(cmd("pytest", "pytest"));
    }
}

fn make(root: &Path, out: &mut Vec<DetectedCommand>) {
    let Some(text) = ["Makefile", "makefile", "GNUmakefile"]
        .iter()
        .find_map(|f| read(root, f))
    else {
        return;
    };
    let mut seen = Vec::new();
    for line in text.lines() {
        let Some((head, rest)) = line.split_once(':') else {
            continue;
        };
        if rest.starts_with('=') || line.starts_with(['\t', ' ', '#', '.']) {
            continue;
        }
        for target in head.split_whitespace() {
            let valid = target
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
            if valid && !seen.iter().any(|t| t == target) {
                seen.push(target.to_string());
            }
        }
    }
    for t in seen {
        out.push(cmd(format!("make {t}"), format!("make {t}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(files: &[(&str, &str)]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(crate::model::new_id("scripts-test"));
        std::fs::create_dir_all(&d).unwrap();
        for (name, body) in files {
            std::fs::write(d.join(name), body).unwrap();
        }
        d
    }

    fn commands(files: &[(&str, &str)]) -> Vec<String> {
        let d = dir(files);
        let found = detect(&d).into_iter().map(|c| c.command).collect();
        let _ = std::fs::remove_dir_all(d);
        found
    }

    #[test]
    fn empty_folder_has_no_commands() {
        assert!(commands(&[]).is_empty());
    }

    #[test]
    fn node_scripts_use_the_lockfile_manager() {
        let pkg = r#"{"scripts":{"dev":"vite","build":"vite build"}}"#;
        assert_eq!(
            commands(&[("package.json", pkg)]),
            ["npm run dev", "npm run build"]
        );
        assert_eq!(
            commands(&[("package.json", pkg), ("pnpm-lock.yaml", "")]),
            ["pnpm run dev", "pnpm run build"]
        );
        assert_eq!(
            commands(&[("package.json", pkg), ("yarn.lock", "")]),
            ["yarn dev", "yarn build"]
        );
        assert_eq!(
            commands(&[("package.json", pkg), ("bun.lockb", "")]),
            ["bun run dev", "bun run build"]
        );
    }

    #[test]
    fn broken_manifest_is_skipped() {
        assert!(commands(&[("package.json", "{ nope")]).is_empty());
    }

    #[test]
    fn makefile_targets_skip_specials_and_recipes() {
        let mk = ".PHONY: all\nall: build\n\tcc x\nbuild test:\nCC := gcc\n%.o: %.c\n";
        assert_eq!(
            commands(&[("Makefile", mk)]),
            ["make all", "make build", "make test"]
        );
    }

    #[test]
    fn cargo_go_python_and_compose() {
        assert_eq!(commands(&[("Cargo.toml", "")]).len(), 3);
        assert_eq!(commands(&[("go.mod", "")]).len(), 3);
        assert_eq!(commands(&[("compose.yaml", "")]).len(), 3);
        let py = "[project.scripts]\nserve = \"a:b\"\n[tool.pytest.ini_options]\n";
        assert_eq!(commands(&[("pyproject.toml", py)]), ["serve", "pytest"]);
    }
}
