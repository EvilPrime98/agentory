//! Test double for the app. FAKE_MODE: `exit` | `hang` | `marker`. FAKE_BUILD_ID is written to the marker.
use std::{env, fs, process, thread, time::Duration};

fn main() {
    match env::var("FAKE_MODE").as_deref() {
        Ok("exit") => process::exit(1),
        Ok("marker") => {
            let path = env::var("AGENTORY_READY_FILE").unwrap();
            let id = env::var("FAKE_BUILD_ID").unwrap_or_default();
            fs::write(path, format!("{}\n{id}\n", process::id())).unwrap();
        }
        _ => {}
    }
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}
