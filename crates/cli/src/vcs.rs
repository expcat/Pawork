//! `pawork diff` / `pawork rollback`。

use std::io::{self, IsTerminal, Write};

use pawork_app::{render_session_diff, AppCore, CheckpointSummary};
use pawork_domain::SessionId;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::CliError;

pub async fn run_diff(
    core: &AppCore,
    session: Option<String>,
    page: Option<usize>,
    json: bool,
) -> Result<(), CliError> {
    let session = resolve_session(core, session.as_deref()).await?;
    let mut diff = core.session_diff(&session).await?;
    if let Some(page) = page {
        let paged = pawork_app::paginate_diff(std::mem::take(&mut diff.files), page, 10);
        diff.files = paged.files;
        if json {
            if let Some(git) = &diff.git {
                eprintln!(
                    "git {} dirty={} worktree={}",
                    git.branch,
                    git.dirty_files,
                    git.work_dir.display()
                );
            }
            println!(
                "{}",
                serde_json::json!({
                    "session_id": diff.session_id,
                    "page": paged.page,
                    "page_size": paged.page_size,
                    "total_files": paged.total_files,
                    "files": diff.files,
                    "git": diff.git,
                })
            );
            return Ok(());
        }
    } else if json {
        if let Some(git) = &diff.git {
            eprintln!(
                "git {} dirty={} worktree={}",
                git.branch,
                git.dirty_files,
                git.work_dir.display()
            );
        }
        println!(
            "{}",
            serde_json::to_string(&diff).map_err(|error| CliError::Turn(error.to_string()))?
        );
        return Ok(());
    }
    if let Some(git) = &diff.git {
        eprintln!(
            "git {}  {} dirty  {}",
            git.branch,
            git.dirty_files,
            git.work_dir.display()
        );
    }
    let text = render_session_diff(&diff);
    if text.is_empty() {
        eprintln!("(no session changes)");
        return Ok(());
    }
    println!("{text}");
    Ok(())
}

pub async fn run_rollback(
    core: &AppCore,
    checkpoint: Option<String>,
    session: Option<String>,
    yes: bool,
    json: bool,
) -> Result<(), CliError> {
    let session = resolve_session(core, session.as_deref()).await?;
    let listed = core.list_checkpoints(&session).await?;
    let spec = match checkpoint {
        Some(id) => id,
        None if yes => latest_run_id(&listed)
            .ok_or_else(|| CliError::Usage("no checkpoints in this session".into()))?
            .to_string(),
        None if json || !io::stdin().is_terminal() => {
            return Err(CliError::Usage(
                "rollback requires an explicit checkpoint id when --json or stdin is not a TTY"
                    .into(),
            ));
        }
        None => {
            if listed.is_empty() {
                return Err(CliError::Usage("no checkpoints in this session".into()));
            }
            print_checkpoint_list(&listed);
            prompt_checkpoint(&listed).await?
        }
    };
    let summary = listed
        .iter()
        .find(|item| item.checkpoint_id == spec)
        .cloned();
    if !yes && !json && io::stdin().is_terminal() {
        let files = summary
            .as_ref()
            .map(|item| item.files.clone())
            .unwrap_or_default();
        if !confirm_restore(&files)? {
            return Err(CliError::Cancelled);
        }
    }
    let outcome = core.rollback(&session, &spec).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&outcome).map_err(|error| CliError::Turn(error.to_string()))?
        );
        return Ok(());
    }
    println!("rolled back {}", outcome.checkpoint_id);
    for path in &outcome.restored {
        println!("  restored {path}");
    }
    Ok(())
}

async fn resolve_session(core: &AppCore, spec: Option<&str>) -> Result<SessionId, CliError> {
    Ok(core.resolve_session(spec.unwrap_or("latest")).await?)
}

fn latest_run_id(listed: &[CheckpointSummary]) -> Option<&str> {
    listed
        .iter()
        .rev()
        .find(|item| item.tool_call_id.is_none())
        .or_else(|| listed.last())
        .map(|item| item.checkpoint_id.as_str())
}

fn print_checkpoint_list(listed: &[CheckpointSummary]) {
    for item in listed {
        let kind = if item.tool_call_id.is_some() {
            "call"
        } else {
            "run"
        };
        println!(
            "{}\t{}\t{} file(s)",
            item.checkpoint_id,
            kind,
            item.files.len()
        );
    }
}

async fn prompt_checkpoint(listed: &[CheckpointSummary]) -> Result<String, CliError> {
    eprint!("checkpoint id: ");
    if let Err(error) = io::stderr().flush() {
        tracing::debug!(%error, "checkpoint prompt stderr flush failed");
    }
    let mut line = String::new();
    let mut reader = BufReader::new(tokio::io::stdin());
    reader.read_line(&mut line).await?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(CliError::Usage("checkpoint id is required".into()));
    }
    if listed.iter().any(|item| item.checkpoint_id == trimmed)
        || listed
            .iter()
            .any(|item| item.tool_call_id.as_deref() == Some(trimmed))
    {
        return Ok(trimmed.to_string());
    }
    Ok(trimmed.to_string())
}

fn confirm_restore(files: &[String]) -> Result<bool, CliError> {
    if files.is_empty() {
        eprint!("Restore checkpoint files? [y/N] ");
    } else {
        eprintln!("files:");
        for path in files {
            eprintln!("  {path}");
        }
        eprint!("Restore {} file(s)? [y/N] ", files.len());
    }
    if let Err(error) = io::stderr().flush() {
        tracing::debug!(%error, "restore confirm stderr flush failed");
    }
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y"))
}
