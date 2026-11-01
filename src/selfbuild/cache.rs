//! The shared target dir: how much disk it uses, and clearing it.

use std::path::Path;

use super::paths::Paths;

/// Total size in bytes of the files under `dir` (0 if it does not exist).
pub fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

pub fn target_size(paths: &Paths) -> u64 {
    dir_size(&paths.target_dir())
}

/// Delete the build cache; the next build is cold again.
pub fn clean(paths: &Paths) -> Result<(), String> {
    let dir = paths.target_dir();
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not delete {}: {e}", dir.display())),
    }
}

/// `1.4 GB`, `312 MB`, `0 B`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(300 * 1024 * 1024), "300 MB");
        assert_eq!(format_bytes(5 * 1024 * 1024 * 1024), "5.0 GB");
    }

    #[test]
    fn sizes_and_cleans_the_target_dir() {
        let root = std::env::temp_dir().join(format!("agentory-cache-{}", std::process::id()));
        let paths = Paths::in_dirs(root.join("r"), root.join("l"));
        assert_eq!(target_size(&paths), 0);
        let nested = paths.target_dir().join("release").join("deps");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("a"), [0u8; 100]).unwrap();
        std::fs::write(paths.target_dir().join("b"), [0u8; 20]).unwrap();
        assert_eq!(target_size(&paths), 120);
        clean(&paths).unwrap();
        assert_eq!(target_size(&paths), 0);
        clean(&paths).unwrap(); // already gone is fine
        let _ = std::fs::remove_dir_all(&root);
    }
}
