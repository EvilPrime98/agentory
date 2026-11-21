use std::path::PathBuf;

use futures_channel::mpsc::UnboundedSender;
use sha2::{Digest, Sha256};

use super::config::{ModelSpec, WHISPER_SMALL, models_root};

pub fn dir() -> Option<PathBuf> {
    Some(models_root()?.join(WHISPER_SMALL.dir))
}

pub fn is_installed() -> bool {
    let Some(dir) = dir() else { return false };
    complete(&WHISPER_SMALL, &dir)
}

fn complete(spec: &ModelSpec, dir: &std::path::Path) -> bool {
    spec.files
        .iter()
        .all(|f| std::fs::metadata(dir.join(f.name)).is_ok_and(|m| m.len() == f.size))
}

pub fn total_size() -> u64 {
    WHISPER_SMALL.files.iter().map(|f| f.size).sum()
}

pub type Progress = UnboundedSender<u64>;

pub async fn download(progress: Progress) -> Result<(), String> {
    let dir = dir().ok_or("no data directory")?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let client = reqwest::Client::new();
    let mut done = 0u64;
    for file in WHISPER_SMALL.files {
        let target = dir.join(file.name);
        if std::fs::metadata(&target).is_ok_and(|m| m.len() == file.size) {
            done += file.size;
            let _ = progress.unbounded_send(done);
            continue;
        }
        let part = dir.join(format!("{}.part", file.name));
        let result = fetch(&client, file.url, &part, file.sha256, &mut done, &progress).await;
        if let Err(e) = result {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(format!("{}: {e}", file.name));
        }
        tokio::fs::rename(&part, &target)
            .await
            .map_err(|e| format!("cannot install {}: {e}", file.name))?;
    }
    Ok(())
}

async fn fetch(
    client: &reqwest::Client,
    url: &str,
    part: &std::path::Path,
    sha256: &str,
    done: &mut u64,
    progress: &Progress,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt as _;

    let mut resp = client
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?;
    let mut out = tokio::fs::File::create(part)
        .await
        .map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        hasher.update(&chunk);
        out.write_all(&chunk).await.map_err(|e| e.to_string())?;
        *done += chunk.len() as u64;
        let _ = progress.unbounded_send(*done);
    }
    out.flush().await.map_err(|e| e.to_string())?;
    let digest = format!("{:x}", hasher.finalize());
    if digest != sha256 {
        return Err(format!("checksum mismatch ({digest})"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_checksums_are_sha256_hex() {
        for f in WHISPER_SMALL.files {
            assert_eq!(f.sha256.len(), 64, "{}", f.name);
            assert!(f.sha256.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn incomplete_directory_is_not_installed() {
        let dir = std::env::temp_dir().join(format!("agentory-model-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!complete(&WHISPER_SMALL, &dir));
        std::fs::write(dir.join(WHISPER_SMALL.files[0].name), b"short").unwrap();
        assert!(!complete(&WHISPER_SMALL, &dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
