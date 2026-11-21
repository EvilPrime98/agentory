use agentory_helper::{Layout, Outcome, swap};
use std::path::PathBuf;
use std::time::Duration;
use std::{fs, process};

fn fake_app() -> PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // deps
    p.pop();
    p.join("examples")
        .join(format!("fake_app{}", std::env::consts::EXE_SUFFIX))
}

fn setup(name: &str) -> (Layout, PathBuf) {
    let root = std::env::temp_dir().join(format!("agentory-helper-{name}-{}", process::id()));
    let _ = fs::remove_dir_all(&root);
    let layout = Layout::in_dir(&root);
    fs::create_dir_all(&layout.bin).unwrap();
    fs::copy(fake_app(), layout.active()).unwrap(); // the previous exe
    let new = root.join("new-build");
    fs::copy(fake_app(), &new).unwrap();
    (layout, new)
}

fn run(name: &str, mode: &str) -> (Layout, Outcome) {
    let (layout, new) = setup(name);
    unsafe {
        std::env::set_var("FAKE_MODE", mode);
        std::env::set_var("FAKE_BUILD_ID", "abc123");
    }
    let out = swap(&layout, 0, &new, "abc123", Duration::from_millis(1500)).unwrap();
    (layout, out)
}

fn cleanup(layout: &Layout) {
    let root = layout.ready.parent().unwrap();
    let _ = if cfg!(windows) {
        let script = format!(
            "Get-CimInstance Win32_Process | Where-Object {{ $_.ExecutablePath -like '{}\\*' }} | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force }}",
            root.display()
        );
        process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .output()
    } else {
        let name = format!("fake_app{}", std::env::consts::EXE_SUFFIX);
        process::Command::new("pkill").args(["-f", &name]).output()
    };
    let _ = fs::remove_dir_all(root);
}

// The scenarios share FAKE_* env vars, so they run in one test.
#[test]
fn swap_keeps_good_and_reverts_bad() {
    let (l, out) = run("good", "marker");
    assert_eq!(out, Outcome::Kept);
    assert!(l.known_good().exists(), "previous exe kept as known-good");
    assert!(!l.revert_log().exists());
    cleanup(&l);

    let (l, out) = run("crash", "exit");
    assert!(matches!(out, Outcome::Reverted(_)), "{out:?}");
    assert!(l.revert_log().exists() && l.active().exists());
    cleanup(&l);

    let (l, out) = run("nomarker", "hang");
    assert!(matches!(out, Outcome::Reverted(_)), "{out:?}");
    assert!(l.revert_log().exists() && l.active().exists());
    cleanup(&l);
}
