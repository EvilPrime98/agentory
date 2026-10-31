//! Build id and ready marker. Deliberately small and isolated in one file: the gate trusts the
//! marker, so the review screen flags any diff here.
//!
//! Marker format (shared with `agentory-helper`): line 1 pid, line 2 build id. Location:
//! `AGENTORY_READY_FILE` if set (the helper and the gate set it), else `%APPDATA%\Agentory\ready`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::paths::Paths;

/// Git sha this exe was built from (set by `build.rs`).
pub const BUILD_ID: &str = env!("AGENTORY_BUILD_ID");
pub const READY_ENV: &str = "AGENTORY_READY_FILE";

fn marker_path() -> Option<PathBuf> {
    match std::env::var_os(READY_ENV) {
        Some(p) => Some(p.into()),
        None => Some(Paths::system()?.ready_marker()),
    }
}

/// Call from every render; only the first call writes.
pub fn on_frame() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if let Some(path) = marker_path() {
        if let Err(e) = write(&path, std::process::id(), BUILD_ID) {
            tracing::warn!("could not write the ready marker: {e}");
        }
    }
}

pub fn write(path: &Path, pid: u32, build_id: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, format!("{pid}\n{build_id}\n"))
}

/// `(pid, build id)` from a marker file, or `None` if missing or malformed.
pub fn read(path: &Path) -> Option<(u32, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let pid = lines.next()?.trim().parse().ok()?;
    let id = lines.next()?.trim();
    (!id.is_empty()).then(|| (pid, id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let path = std::env::temp_dir().join(format!("agentory-ready-{}", std::process::id()));
        write(&path, 42, "abc123").unwrap();
        assert_eq!(read(&path), Some((42, "abc123".into())));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(read(&path), None);
    }

    #[test]
    fn malformed_is_none() {
        let path = std::env::temp_dir().join(format!("agentory-ready-bad-{}", std::process::id()));
        std::fs::write(&path, "notapid\nabc\n").unwrap();
        assert_eq!(read(&path), None);
        std::fs::write(&path, "7\n").unwrap();
        assert_eq!(read(&path), None);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn build_id_is_embedded() {
        assert!(!BUILD_ID.is_empty());
    }
}
