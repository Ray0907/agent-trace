mod api;
mod cost;
mod parser;
mod state;
mod watcher;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::net::TcpListener;

use crate::state::AppState;

#[derive(Debug, Parser)]
#[command(name = "agent-trace")]
#[command(about = "Serve Claude Code session traces over REST and WebSocket APIs")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve {
        #[arg(long, default_value_t = 7842)]
        port: u16,
        #[arg(long)]
        sessions_dir: Option<PathBuf>,
    },
    List {
        #[arg(long)]
        sessions_dir: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Serve { port, sessions_dir } => {
            serve(port, resolve_sessions_dir(sessions_dir)?).await
        }
        Command::List { sessions_dir } => list(resolve_sessions_dir(sessions_dir)?).await,
    }
}

fn resolve_sessions_dir(override_dir: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = override_dir {
        return Ok(path);
    }

    let home_dir = dirs::home_dir().context("could not determine home directory")?;
    Ok(default_sessions_dir(home_dir))
}

fn default_sessions_dir(home_dir: PathBuf) -> PathBuf {
    home_dir.join(".claude").join("projects")
}

async fn serve(port: u16, sessions_dir: PathBuf) -> Result<()> {
    let state = AppState::new(sessions_dir)?;
    state.refresh().await?;

    let _watcher = watcher::start_watcher(state.clone());
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    api::serve(listener, state).await
}

async fn list(sessions_dir: PathBuf) -> Result<()> {
    let state = AppState::new(sessions_dir)?;
    state.refresh().await?;

    let sessions = state.list_summaries().await;
    println!("{}", serde_json::to_string_pretty(&sessions)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::default_sessions_dir;

    #[test]
    fn defaults_to_claude_projects_directory() {
        assert_eq!(
            default_sessions_dir(PathBuf::from("/tmp/home")),
            PathBuf::from("/tmp/home/.claude/projects")
        );
    }
}
