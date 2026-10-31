//! Launch and swap/revert logic for the Agentory helper. Std only (plus `windows-sys` for waiting
//! on a pid); it must stay independent of the app so a broken app cannot break the safety net.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// Seconds a freshly swapped exe must stay alive, with a matching ready marker, to be kept.
pub const SMOKE_SECS: u64 = 10;
/// Upper bound on waiting for the old app process to exit before swapping.
pub const EXIT_WAIT: Duration = Duration::from_secs(60);
/// Env var telling the app where to write its ready marker.
pub const READY_ENV: &str = "AGENTORY_READY_FILE";
/// Env var telling the app where this helper is.
pub const HELPER_ENV: &str = "AGENTORY_HELPER";

const EXT: &str = if cfg!(windows) { "exe" } else { "bin" };

#[derive(Debug, Clone)]
pub struct Layout {
    pub bin: PathBuf,
    pub ready: PathBuf,
}

impl Layout {
    /// `%APPDATA%\Agentory\bin` with the marker at `%APPDATA%\Agentory\ready`.
    pub fn default_location() -> io::Result<Layout> {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .ok_or_else(|| io::Error::other("APPDATA is not set"))?
            .join("Agentory");
        Ok(Layout::in_dir(&base))
    }

    /// Layout rooted at `root` (`root/bin/*`, `root/ready`).
    pub fn in_dir(root: &Path) -> Layout {
        Layout {
            bin: root.join("bin"),
            ready: root.join("ready"),
        }
    }

    pub fn active(&self) -> PathBuf {
        self.bin.join(format!("active.{EXT}"))
    }
    pub fn known_good(&self) -> PathBuf {
        self.bin.join(format!("known-good.{EXT}"))
    }
    pub fn fallback(&self) -> PathBuf {
        self.bin.join(format!("fallback.{EXT}"))
    }
    pub fn revert_log(&self) -> PathBuf {
        self.bin.join("revert.log")
    }
}

fn spawn(layout: &Layout, exe: &Path) -> io::Result<Child> {
    let mut cmd = Command::new(exe);
    cmd.env(READY_ENV, &layout.ready);
    // Lets the app find this helper again when it asks for a swap.
    if let Ok(me) = std::env::current_exe() {
        cmd.env(HELPER_ENV, me);
    }
    cmd.spawn()
}

/// Start `active`, falling back to `known-good`, then `fallback`.
pub fn launch(layout: &Layout) -> io::Result<Child> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "no executable found");
    for exe in [layout.active(), layout.known_good(), layout.fallback()] {
        if !exe.exists() {
            continue;
        }
        match spawn(layout, &exe) {
            Ok(child) => return Ok(child),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Retry briefly: Windows keeps an exe locked for a moment after its process exits.
fn retry<T>(mut f: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut tries = 0;
    loop {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) if tries >= 30 => return Err(e),
            Err(_) => {
                tries += 1;
                sleep(Duration::from_millis(100));
            }
        }
    }
}

fn remove_if_exists(p: &Path) -> io::Result<()> {
    match fs::remove_file(p) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Move `from` to `to`, copying when they are on different volumes.
fn place(from: &Path, to: &Path) -> io::Result<()> {
    retry(|| {
        remove_if_exists(to)?;
        if fs::rename(from, to).is_err() {
            fs::copy(from, to)?;
            let _ = fs::remove_file(from);
        }
        Ok(())
    })
}

/// Block until process `pid` has exited. Waits on the process handle, not a sleep.
#[cfg(windows)]
pub fn wait_for_exit(pid: u32, timeout: Duration) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return Ok(()); // already gone (or never existed)
        }
        let ms = timeout.as_millis().min(u32::MAX as u128 - 1) as u32;
        let r = WaitForSingleObject(h, ms);
        CloseHandle(h);
        if r == WAIT_OBJECT_0 {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("pid {pid} did not exit"),
            ))
        }
    }
}

#[cfg(not(windows))]
pub fn wait_for_exit(pid: u32, timeout: Duration) -> io::Result<()> {
    let start = Instant::now();
    while Path::new(&format!("/proc/{pid}")).exists() {
        if start.elapsed() > timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("pid {pid} did not exit"),
            ));
        }
        sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// Marker format: first line pid, second line build id.
fn marker_build_id(ready: &Path) -> Option<String> {
    let text = fs::read_to_string(ready).ok()?;
    let mut lines = text.lines();
    lines.next()?.trim().parse::<u32>().ok()?;
    Some(lines.next()?.trim().to_string())
}

/// Pass means: still alive after `dur` and the ready marker carries `build_id`.
pub fn smoke_check(
    child: &mut Child,
    layout: &Layout,
    build_id: &str,
    dur: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + dur;
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("exited early with {status}"));
        }
        sleep(Duration::from_millis(50));
    }
    if let Ok(Some(status)) = child.try_wait() {
        return Err(format!("exited early with {status}"));
    }
    match marker_build_id(&layout.ready) {
        Some(id) if id == build_id => Ok(()),
        Some(id) => Err(format!(
            "ready marker has build id {id}, expected {build_id}"
        )),
        None => Err("no valid ready marker".into()),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Kept,
    Reverted(String),
}

/// Wait for `pid` to exit, install `new` as `active` (keeping the old one as `known-good`),
/// relaunch and smoke check. On failure restore the previous exe and relaunch it.
pub fn swap(
    layout: &Layout,
    pid: u32,
    new: &Path,
    build_id: &str,
    smoke: Duration,
) -> io::Result<Outcome> {
    fs::create_dir_all(&layout.bin)?;
    wait_for_exit(pid, EXIT_WAIT)?;
    let _ = fs::remove_file(&layout.ready);

    let (active, known_good) = (layout.active(), layout.known_good());
    let had_active = active.exists();
    if had_active {
        place(&active, &known_good)?;
    }
    place(new, &active)?;

    let mut child = spawn(layout, &active)?;
    let reason = match smoke_check(&mut child, layout, build_id, smoke) {
        Ok(()) => return Ok(Outcome::Kept),
        Err(r) => r,
    };

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(&layout.ready);
    if had_active {
        place(&known_good, &active)?;
    } else if layout.fallback().exists() {
        retry(|| remove_if_exists(&active))?;
        fs::copy(layout.fallback(), &active)?;
    } else {
        retry(|| remove_if_exists(&active))?;
    }
    let _ = fs::write(
        layout.revert_log(),
        format!("reverted build {build_id}: {reason}\n"),
    );
    launch(layout)?;
    Ok(Outcome::Reverted(reason))
}
