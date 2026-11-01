//! Windows only: embed the app icon and version info into the .exe.
//! Failing to do so (no `rc.exe`) must not break the build, so it only warns.

/// Stdout of `git <args>` when it succeeds.
fn git(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Embed the git sha as the build id (the ready marker reports it). Same `--short=12` the
/// self-build gate uses to compute the expected value.
fn emit_build_id() {
    let id = git(&["rev-parse", "--short=12", "HEAD"])
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=AGENTORY_BUILD_ID={id}");
    // Rebuild when HEAD moves: the HEAD file itself, and the branch ref it points at.
    if let Some(head) = git(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head}");
    }
    if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git(&["rev-parse", "--git-path", &r]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=packaging/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    emit_build_id();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    if std::path::Path::new("packaging/icon.ico").exists() {
        res.set_icon("packaging/icon.ico");
    }
    res.set("ProductName", "Agentory");
    res.set("FileDescription", "Agentory");
    res.set("LegalCopyright", "Copyright © AminPerez. GPL-3.0-or-later");
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed the Windows icon/version resources: {e}");
    }
}
