//! `pawork sessions list/show`。

use pawork_app::AppCore;
use pawork_domain::{ContentPart, Message, MessageRole};

use crate::{CliError, SessionsCommand};

pub async fn run_sessions(
    core: &AppCore,
    command: SessionsCommand,
    json: bool,
) -> Result<(), CliError> {
    match command {
        SessionsCommand::List => list(core, json).await,
        SessionsCommand::Show { session } => show(core, &session, json).await,
    }
}

async fn list(core: &AppCore, json: bool) -> Result<(), CliError> {
    let rows = core.list_sessions().await?;
    if json {
        let payload: Vec<serde_json::Value> = rows
            .iter()
            .map(|row| {
                serde_json::json!({
                    "session_id": row.session_id,
                    "title": row.title,
                    "created_at_ms": row.created_at_ms,
                    "updated_at_ms": row.updated_at_ms,
                    "active_branch": row.active_branch,
                })
            })
            .collect();
        println!("{}", serde_json::to_string(&payload).map_err(json_err)?);
        return Ok(());
    }
    if rows.is_empty() {
        eprintln!("(no sessions)");
        return Ok(());
    }
    for row in rows {
        println!(
            "{}\t{}\t{}",
            row.session_id,
            format_millis(row.updated_at_ms),
            row.title
        );
    }
    Ok(())
}

async fn show(core: &AppCore, spec: &str, json: bool) -> Result<(), CliError> {
    let session = core.resolve_session(spec).await?;
    let record = core.get_session(&session).await?;
    let messages = core.resume_messages(&session).await?;
    let usage = core.session_usage(&session).await?;
    let switches = model_switches(core, &session).await?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "session_id": record.session_id,
                "title": record.title,
                "created_at_ms": record.created_at_ms,
                "updated_at_ms": record.updated_at_ms,
                "active_branch": record.active_branch,
                "messages": messages,
                "usage": usage,
                "model_switches": switches,
            })
        );
        return Ok(());
    }
    println!("session: {}", record.session_id);
    println!("title: {}", record.title);
    println!("created: {}", format_millis(record.created_at_ms));
    println!("updated: {}", format_millis(record.updated_at_ms));
    println!("messages: {}", messages.len());
    println!(
        "usage: in {} out {} (cache read {} / write {})",
        usage.input_tokens, usage.output_tokens, usage.cache_read_tokens, usage.cache_write_tokens
    );
    println!();
    for message in messages {
        println!("[{}] {}", role_label(&message), message_text(&message));
    }
    if !switches.is_empty() {
        println!();
        println!("model switches:");
        for switch in &switches {
            let timestamp = switch["timestamp_ms"].as_i64().unwrap_or(0);
            let from_provider = switch["from"]["provider"].as_str().unwrap_or("?");
            let from_model = switch["from"]["model"].as_str().unwrap_or("?");
            let to_provider = switch["to"]["provider"].as_str().unwrap_or("?");
            let to_model = switch["to"]["model"].as_str().unwrap_or("?");
            println!("  [{}] {from_provider} {from_model} -> {to_provider} {to_model}", format_millis(timestamp));
        }
    }
    Ok(())
}

fn role_label(message: &Message) -> &'static str {
    match message.role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::System => "system",
        MessageRole::Tool => "tool",
    }
}

/// 读取 model.switched 诊断事件并投影为展示形状（from/to provider+model）。
async fn model_switches(
    core: &AppCore,
    session: &pawork_domain::SessionId,
) -> Result<Vec<serde_json::Value>, CliError> {
    let events = core
        .store()?
        .replay_events(session, 0, 10_000)
        .await
        .map_err(pawork_app::AppError::from)?;
    let mut switches = Vec::new();
    for envelope in events {
        if let pawork_domain::AgentEvent::Diagnostic { code, details } = envelope.payload {
            if code != "model.switched" {
                continue;
            }
            switches.push(serde_json::json!({
                "sequence": envelope.sequence.value(),
                "timestamp_ms": envelope.timestamp.as_unix_millis(),
                "from": details.get("from").cloned().unwrap_or(serde_json::Value::Null),
                "to": details.get("to").cloned().unwrap_or(serde_json::Value::Null),
            }));
        }
    }
    Ok(switches)
}

fn message_text(message: &Message) -> String {
    let mut parts = Vec::new();
    for part in &message.content {
        match part {
            ContentPart::Text(text) => parts.push(text.text.clone()),
            ContentPart::Thinking(thinking) => parts.push(format!("(thinking) {}", thinking.text)),
            ContentPart::ToolCall(call) => parts.push(format!("(tool {})", call.name)),
            _ => {}
        }
    }
    parts.join(" ")
}

pub(crate) fn format_millis(ms: i64) -> String {
    if ms < 0 {
        return ms.to_string();
    }
    let total_secs = (ms / 1000) as u64;
    let days = total_secs / 86_400;
    let secs_of_day = total_secs % 86_400;
    let hour = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;
    let sec = secs_of_day % 60;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}Z")
}

/// Howard Hinnant civil_from_days（Unix epoch 日序 → 公历）。
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m as u32, d as u32)
}

fn json_err(error: serde_json::Error) -> CliError {
    CliError::Turn(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_millis_unix_epoch() {
        assert_eq!(format_millis(0), "1970-01-01 00:00:00Z");
        assert_eq!(format_millis(1_704_067_200_000), "2024-01-01 00:00:00Z");
    }
}
