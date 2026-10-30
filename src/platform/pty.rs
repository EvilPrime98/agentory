//! Pseudo-terminal behind a trait, so ConPTY (Windows) is the only implementation for now but
//! Linux/macOS can follow with the same `portable-pty` backend.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Everything needed to start a process in a PTY.
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub rows: u16,
    pub cols: u16,
}

/// The writable half of a running PTY.
pub trait Pty: Send {
    fn write(&mut self, data: &[u8]) -> std::io::Result<()>;
    fn resize(&self, rows: u16, cols: u16);
    fn kill(&mut self);
    /// Non-blocking: has the child process ended?
    fn has_exited(&mut self) -> bool;
}

pub struct Spawned {
    pub pty: Box<dyn Pty>,
    pub reader: Box<dyn Read + Send>,
}

struct PortablePty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl Pty for PortablePty {
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(data)?;
        self.writer.flush()
    }

    fn resize(&self, rows: u16, cols: u16) {
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
    }

    fn has_exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }
}

pub fn spawn(spec: SpawnSpec) -> Result<Spawned> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: spec.rows,
            cols: spec.cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("could not open a pseudo-terminal")?;

    // `.cmd`/`.bat` shims cannot be started directly.
    let (program, args) = if is_script(&spec.program) {
        let mut args = vec!["/c".to_string(), spec.program.clone()];
        args.extend(spec.args.iter().cloned());
        ("cmd.exe".to_string(), args)
    } else {
        (spec.program.clone(), spec.args.clone())
    };
    let mut cmd = CommandBuilder::new(&program);
    cmd.args(&args);
    cmd.cwd(&spec.cwd);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .with_context(|| format!("could not start {}", spec.program))?;
    let reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;
    Ok(Spawned {
        pty: Box::new(PortablePty {
            master: pair.master,
            writer,
            child,
        }),
        reader,
    })
}

fn is_script(program: &str) -> bool {
    let lower = program.to_ascii_lowercase();
    lower.ends_with(".cmd") || lower.ends_with(".bat")
}

/// Find `name` on `PATH` (trying the Windows executable extensions). An existing path is returned
/// as is.
pub fn resolve_program(name: &str) -> Option<PathBuf> {
    let direct = Path::new(name);
    if direct.components().count() > 1 && direct.exists() {
        return Some(direct.to_path_buf());
    }
    let exts: &[&str] = if cfg!(windows) {
        &["exe", "cmd", "bat"]
    } else {
        &[""]
    };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in exts {
            let mut candidate = dir.join(name);
            if !ext.is_empty() {
                candidate.set_extension(ext);
            }
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_detected() {
        assert!(is_script("C:/x/spm.CMD"));
        assert!(!is_script("claude.exe"));
    }

    #[test]
    fn missing_program_is_none() {
        assert!(resolve_program("definitely-not-a-program-xyz").is_none());
    }
}
