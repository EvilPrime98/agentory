//! Data layout: clone and exes under `%APPDATA%\Agentory`, the multi-GB shared target dir under
//! `%LOCALAPPDATA%\Agentory\target`.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `%APPDATA%\Agentory`
    roaming: PathBuf,
    /// `%LOCALAPPDATA%\Agentory`
    local: PathBuf,
}

impl Paths {
    pub fn system() -> Option<Paths> {
        Some(Paths::in_dirs(dirs::config_dir()?, dirs::data_local_dir()?))
    }

    /// Layout from the two base dirs (`%APPDATA%`, `%LOCALAPPDATA%`).
    pub fn in_dirs(roaming: impl AsRef<Path>, local: impl AsRef<Path>) -> Paths {
        Paths {
            roaming: roaming.as_ref().join("Agentory"),
            local: local.as_ref().join("Agentory"),
        }
    }

    /// The local git clone of the source.
    pub fn clone_dir(&self) -> PathBuf {
        self.roaming.join("src")
    }
    /// Directory holding `active`, `known-good` and `fallback` exes (shared with the helper).
    pub fn bin_dir(&self) -> PathBuf {
        self.roaming.join("bin")
    }
    /// The ready marker the app writes after its first frame (shared with the helper).
    pub fn ready_marker(&self) -> PathBuf {
        self.roaming.join("ready")
    }
    /// Present once self-build is enabled. The installer reads it (see `agentory.iss`) to leave
    /// `active.exe` alone when a newer official release is installed.
    pub fn selfbuild_marker(&self) -> PathBuf {
        self.roaming.join("selfbuild")
    }
    /// Where a gated build waits for the helper to install it (shared with the helper's `--new`).
    pub fn staged_exe(&self) -> PathBuf {
        self.bin_dir()
            .join(format!("staged{}", std::env::consts::EXE_SUFFIX))
    }
    /// Shared `CARGO_TARGET_DIR`, reused so builds stay incremental.
    pub fn target_dir(&self) -> PathBuf {
        self.local.join("target")
    }
    /// The release exe `cargo build --release` produces in the shared target dir.
    pub fn built_exe(&self) -> PathBuf {
        built_exe(&self.target_dir(), env!("CARGO_PKG_NAME"))
    }
}

pub fn built_exe(target_dir: &Path, bin: &str) -> PathBuf {
    target_dir
        .join("release")
        .join(format!("{bin}{}", std::env::consts::EXE_SUFFIX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> Paths {
        Paths::in_dirs("R", "L")
    }

    #[test]
    fn roaming_holds_clone_bin_and_marker() {
        let p = p();
        let root = Path::new("R").join("Agentory");
        assert_eq!(p.clone_dir(), root.join("src"));
        assert_eq!(p.bin_dir(), root.join("bin"));
        assert_eq!(p.ready_marker(), root.join("ready"));
        assert_eq!(p.staged_exe().parent(), Some(root.join("bin").as_path()));
    }

    #[test]
    fn target_dir_is_local_not_roaming() {
        assert_eq!(
            p().target_dir(),
            Path::new("L").join("Agentory").join("target")
        );
    }

    #[test]
    fn built_exe_is_in_release() {
        let exe = p().built_exe();
        assert!(
            exe.starts_with(
                Path::new("L")
                    .join("Agentory")
                    .join("target")
                    .join("release")
            )
        );
        assert!(
            exe.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("agentory")
        );
    }
}
