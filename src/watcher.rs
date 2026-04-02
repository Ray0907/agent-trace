use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tokio::task::JoinHandle;
use tokio::time::{self, Duration};

use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileFingerprint {
    modified_at: Option<SystemTime>,
    size_bytes: u64,
}

pub fn start_watcher(state: AppState) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut known_files = snapshot_session_files(&state.sessions_dir);
        let mut ticker = time::interval(Duration::from_secs(1));

        loop {
            ticker.tick().await;

            let current_files = snapshot_session_files(&state.sessions_dir);
            let changed_paths = diff_paths(&known_files, &current_files);
            if changed_paths.is_empty() {
                known_files = current_files;
                continue;
            }

            if state.refresh().await.is_ok() {
                for path in &changed_paths {
                    state.publish_session_update_for_path(path).await;
                }
            }

            known_files = current_files;
        }
    })
}

fn snapshot_session_files(root: &Path) -> HashMap<PathBuf, FileFingerprint> {
    let mut snapshot = HashMap::new();
    let mut directories = vec![root.to_path_buf()];

    while let Some(directory) = directories.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                directories.push(path);
                continue;
            }

            if !is_session_file(&path) {
                continue;
            }

            let Ok(metadata) = entry.metadata() else {
                continue;
            };

            snapshot.insert(
                path,
                FileFingerprint {
                    modified_at: metadata.modified().ok(),
                    size_bytes: metadata.len(),
                },
            );
        }
    }

    snapshot
}

fn diff_paths(
    previous: &HashMap<PathBuf, FileFingerprint>,
    current: &HashMap<PathBuf, FileFingerprint>,
) -> Vec<PathBuf> {
    let mut changed = Vec::new();

    for (path, fingerprint) in current {
        if previous.get(path) != Some(fingerprint) {
            changed.push(path.clone());
        }
    }

    for path in previous.keys() {
        if !current.contains_key(path) {
            changed.push(path.clone());
        }
    }

    changed.sort();
    changed
}

fn is_session_file(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("jsonl"))
}
