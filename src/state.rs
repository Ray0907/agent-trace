use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::sync::{broadcast, RwLock};

use crate::parser::parse_session_file;

#[derive(Debug, Clone, Serialize)]
pub struct ToolCall {
    pub index: usize,
    pub tool_use_id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub output: String,
    pub is_error: bool,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_write_tokens: u32,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub file_path: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub task: String,
    pub tool_calls: Vec<ToolCall>,
    pub total_cost_usd: f64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub file_path: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub task: String,
    pub total_cost_usd: f64,
    pub status: String,
    pub tool_call_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionTraceResponse {
    pub session: Session,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCostSummary {
    pub name: String,
    pub cost_usd: f64,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionCostResponse {
    pub total_usd: f64,
    pub per_tool: Vec<ToolCostSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionUpdatedEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub session_id: String,
    pub updated_at_ms: u64,
}

#[derive(Clone)]
pub struct AppState {
    pub sessions_dir: PathBuf,
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    updates_tx: broadcast::Sender<SessionUpdatedEvent>,
}

impl AppState {
    pub fn new(sessions_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&sessions_dir)
            .with_context(|| format!("failed to create {}", sessions_dir.display()))?;

        let (updates_tx, _) = broadcast::channel(256);
        Ok(Self {
            sessions_dir,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            updates_tx,
        })
    }

    pub async fn refresh(&self) -> Result<()> {
        let sessions_dir = self.sessions_dir.clone();
        let loaded_sessions =
            tokio::task::spawn_blocking(move || load_sessions(&sessions_dir)).await??;

        let mut guard = self.sessions.write().await;
        *guard = loaded_sessions;
        Ok(())
    }

    pub async fn list_summaries(&self) -> Vec<SessionSummary> {
        let guard = self.sessions.read().await;
        let mut sessions: Vec<SessionSummary> = guard.values().map(session_summary).collect();
        sessions.sort_by(|left, right| {
            right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then_with(|| left.id.cmp(&right.id))
        });
        sessions
    }

    pub async fn trace_response(&self, id: &str) -> Option<SessionTraceResponse> {
        let session = self.get_session(id).await?;
        Some(SessionTraceResponse {
            tool_calls: session.tool_calls.clone(),
            session,
        })
    }

    pub async fn cost_response(&self, id: &str) -> Option<SessionCostResponse> {
        let session = self.get_session(id).await?;
        let mut per_tool = BTreeMap::<String, ToolCostSummary>::new();

        for tool_call in &session.tool_calls {
            let entry = per_tool
                .entry(tool_call.name.clone())
                .or_insert_with(|| ToolCostSummary {
                    name: tool_call.name.clone(),
                    cost_usd: 0.0,
                    count: 0,
                });
            entry.cost_usd += tool_call.cost_usd;
            entry.count += 1;
        }

        let mut per_tool: Vec<ToolCostSummary> = per_tool.into_values().collect();
        per_tool.sort_by(|left, right| {
            right
                .cost_usd
                .partial_cmp(&left.cost_usd)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.name.cmp(&right.name))
        });

        Some(SessionCostResponse {
            total_usd: session.total_cost_usd,
            per_tool,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SessionUpdatedEvent> {
        self.updates_tx.subscribe()
    }

    pub async fn publish_session_update_for_path(&self, path: &Path) {
        let Some(session) = self.session_for_path(path).await else {
            return;
        };

        let _ = self.updates_tx.send(SessionUpdatedEvent {
            event_type: "session_updated".to_string(),
            session_id: session.id,
            updated_at_ms: session.updated_at_ms,
        });
    }

    async fn get_session(&self, id: &str) -> Option<Session> {
        let guard = self.sessions.read().await;
        guard.get(id).cloned()
    }

    async fn session_for_path(&self, path: &Path) -> Option<Session> {
        let target = path.to_string_lossy();
        let guard = self.sessions.read().await;
        guard
            .values()
            .find(|session| session.file_path == target)
            .cloned()
    }
}

fn load_sessions(root: &Path) -> Result<HashMap<String, Session>> {
    let mut sessions = HashMap::new();

    for path in discover_session_files(root)? {
        let session = match parse_session_file(&path) {
            Ok(session) => session,
            Err(error) => Session {
                id: error_session_id(&path),
                file_path: path.display().to_string(),
                created_at_ms: 0,
                updated_at_ms: 0,
                task: error.to_string(),
                tool_calls: Vec::new(),
                total_cost_usd: 0.0,
                status: "error".to_string(),
            },
        };

        sessions.insert(session.id.clone(), session);
    }

    Ok(sessions)
}

fn discover_session_files(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut directories = vec![root.to_path_buf()];
    let mut files = Vec::new();

    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory)
            .with_context(|| format!("failed to read {}", directory.display()))?
        {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                directories.push(path);
                continue;
            }

            if path.extension() == Some(OsStr::new("jsonl")) {
                files.push(path);
            }
        }
    }

    files.sort();
    Ok(files)
}

fn error_session_id(path: &Path) -> String {
    let mut id = String::from("error:");
    for ch in path.to_string_lossy().chars() {
        if ch.is_ascii_alphanumeric() {
            id.push(ch);
        } else {
            id.push('_');
        }
    }
    id
}

fn session_summary(session: &Session) -> SessionSummary {
    SessionSummary {
        id: session.id.clone(),
        file_path: session.file_path.clone(),
        created_at_ms: session.created_at_ms,
        updated_at_ms: session.updated_at_ms,
        task: session.task.clone(),
        total_cost_usd: session.total_cost_usd,
        status: session.status.clone(),
        tool_call_count: session.tool_calls.len(),
    }
}
