#![cfg_attr(windows, windows_subsystem = "windows")]

use agentory_helper::{Layout, SMOKE_SECS, launch, swap};
use std::path::PathBuf;
use std::time::Duration;

fn run() -> Result<(), String> {
    let layout = Layout::default_location().map_err(|e| e.to_string())?;
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("launch") => launch(&layout)
            .map(|_| ())
            .map_err(|e| format!("launch failed: {e}")),
        Some("swap") => {
            let (mut pid, mut new, mut build) = (None, None, None);
            while let Some(flag) = args.next() {
                let val = args.next().ok_or(format!("missing value for {flag}"))?;
                match flag.as_str() {
                    "--pid" => pid = Some(val.parse::<u32>().map_err(|e| e.to_string())?),
                    "--new" => new = Some(PathBuf::from(val)),
                    "--build-id" => build = Some(val),
                    _ => return Err(format!("unknown flag {flag}")),
                }
            }
            let (pid, new, build) = (
                pid.ok_or("--pid is required")?,
                new.ok_or("--new is required")?,
                build.ok_or("--build-id is required")?,
            );
            swap(&layout, pid, &new, &build, Duration::from_secs(SMOKE_SECS))
                .map(|_| ())
                .map_err(|e| format!("swap failed: {e}"))
        }
        _ => Err(
            "usage: agentory-helper launch | swap --pid <pid> --new <path> --build-id <id>".into(),
        ),
    }
}

fn main() {
    if let Err(msg) = run() {
        // GUI subsystem: no console, so leave a trace next to the exes.
        if let Ok(l) = Layout::default_location() {
            let _ = std::fs::create_dir_all(&l.bin);
            let _ = std::fs::write(l.bin.join("helper.log"), format!("{msg}\n"));
        }
        std::process::exit(1);
    }
}
