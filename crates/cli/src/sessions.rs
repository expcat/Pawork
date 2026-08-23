//! `pawork sessions list/show`。

use pawork_app::{
    parse_session_source, AppCore, LocalSessionSource, SessionImportFormat, SessionImportOutcome,
};
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
        SessionsCommand::Export { session, out } => export(core, session, out, json).await,
        SessionsCommand::Import {
            path,
            format,
            source,
            from,
        } => import(core, path, format, source, from, json).await,
        SessionsCommand::Fork {
            session,
            event,
            no_switch,
        } => fork(core, session, event, !no_switch, json).await,
    }
}

async fn fork(
    core: &AppCore,
    session: Option<String>,
    event: String,
    switch: bool,
    json: bool,
) -> Result<(), CliError> {
    let spec = session.as_deref().unwrap_or("latest");
    let session_id = core.resolve_session(spec).await?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let short = event.chars().take(8).collect::<String>();
    let branch_id = format!("fork-{millis}-{short}");
    let store = core.store()?;
    store
        .fork_from_event(
            &session_id,
            &branch_id,
            &pawork_domain::EventId::from(event.as_str()),
        )
        .await
        .map_err(pawork_app::AppError::from)?;
    if switch {
        store
            .switch_branch(&session_id, &branch_id)
            .await
            .map_err(pawork_app::AppError::from)?;
    }
    if json {
        println!(
            "{}",
            serde_json::json!({
                "session_id": session_id.as_str(),
                "branch_id": branch_id,
                "parent_event_id": event,
                "switched": switch,
            })
        );
    } else {
        println!(
            "forked {} branch {branch_id} from {event}{}",
            session_id.as_str(),
            if switch { " (switched)" } else { "" }
        );
    }
    Ok(())
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

async fn export(
    core: &AppCore,
    session: Option<String>,
    out: Option<std::path::PathBuf>,
    json: bool,
) -> Result<(), CliError> {
    let (session_id, export) = core.export_session_doc(session.as_deref()).await?;
    let payload = serde_json::to_string_pretty(&export).map_err(json_err)?;
    if json && out.is_none() {
        println!("{payload}");
        return Ok(());
    }
    let path = out.unwrap_or_else(|| {
        std::path::PathBuf::from(format!("{}.export.json", session_id.as_str()))
    });
    std::fs::write(&path, payload)?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "session_id": session_id.as_str(),
                "path": path,
            })
        );
    } else {
        println!("exported {} → {}", session_id.as_str(), path.display());
    }
    Ok(())
}

async fn import(
    core: &AppCore,
    path: Option<std::path::PathBuf>,
    format: Option<String>,
    source: Option<String>,
    from: Option<String>,
    json: bool,
) -> Result<(), CliError> {
    if let Some(from) = from.as_deref() {
        return import_from_local(core, from, json).await;
    }
    let Some(path) = path else {
        return Err(CliError::Usage(
            "sessions import requires <path> or --from claude|codex".into(),
        ));
    };
    let mut sniffed_source = None;
    let format = match format.as_deref() {
        Some(name) => SessionImportFormat::parse(name)?,
        None => {
            let (format, source) = detect_session_format(&path)?;
            sniffed_source = source;
            format
        }
    };
    let source = match source.as_deref() {
        Some(name) => Some(parse_session_source(name)?),
        None => sniffed_source,
    };
    let outcome = core.import_session_file(&path, format, source).await?;
    match outcome {
        SessionImportOutcome::Export { session_id } => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "format": "export",
                        "session_id": session_id,
                    })
                );
            } else {
                println!("imported session {session_id}");
            }
        }
        SessionImportOutcome::Compat(report) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "format": "compat",
                        "session_id": report.session_id,
                        "imported_events": report.imported_events,
                        "deduplicated": report.deduplicated,
                    })
                );
            } else {
                println!(
                    "imported compat session {} ({} events{})",
                    report.session_id,
                    report.imported_events,
                    if report.deduplicated { ", deduplicated" } else { "" }
                );
            }
        }
        SessionImportOutcome::Pi(report) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "format": "pi",
                        "imported_messages": report.imported_messages,
                        "parsed_entries": report.parsed_entries,
                    })
                );
            } else {
                println!(
                    "imported pi session ({} messages, {} entries)",
                    report.imported_messages, report.parsed_entries
                );
            }
        }
    }
    Ok(())
}

async fn import_from_local(core: &AppCore, from: &str, json: bool) -> Result<(), CliError> {
    let source = parse_local_session_source(from)?;
    let files = core.scan_local_sessions(source, None)?;
    if files.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "format": "compat",
                    "from": source.as_str(),
                    "files": [],
                    "imported": 0,
                    "deduplicated": 0,
                    "failed": 0,
                })
            );
        } else {
            eprintln!("no local {source} sessions found");
        }
        return Ok(());
    }
    let compat_source = compat_source_for(source);
    let mut reports = Vec::new();
    let mut imported = 0usize;
    let mut deduplicated = 0usize;
    let mut failed = 0usize;
    for file in files {
        let entry = match core
            .import_session_file(
                &file.path,
                SessionImportFormat::Compat,
                Some(compat_source),
            )
            .await
        {
            Ok(SessionImportOutcome::Compat(report)) => {
                if report.deduplicated {
                    deduplicated += 1;
                } else {
                    imported += 1;
                }
                serde_json::json!({
                    "path": file.path,
                    "status": if report.deduplicated { "deduplicated" } else { "imported" },
                    "session_id": report.session_id,
                    "imported_events": report.imported_events,
                })
            }
            Ok(_) => {
                failed += 1;
                serde_json::json!({
                    "path": file.path,
                    "status": "error",
                    "error": "unexpected import outcome",
                })
            }
            Err(error) => {
                failed += 1;
                serde_json::json!({
                    "path": file.path,
                    "status": "error",
                    "error": error.to_string(),
                })
            }
        };
        reports.push(entry);
    }
    if json {
        println!(
            "{}",
            serde_json::json!({
                "format": "compat",
                "from": source.as_str(),
                "files": reports,
                "imported": imported,
                "deduplicated": deduplicated,
                "failed": failed,
            })
        );
    } else {
        for report in &reports {
            let path = report["path"].as_str().unwrap_or_default();
            match report["status"].as_str() {
                Some("imported") => println!(
                    "imported compat session {} ({} events) <- {path}",
                    report["session_id"].as_str().unwrap_or_default(),
                    report["imported_events"].as_u64().unwrap_or(0),
                ),
                Some("deduplicated") => println!(
                    "deduplicated {} <- {path}",
                    report["session_id"].as_str().unwrap_or_default(),
                ),
                _ => eprintln!(
                    "error importing {path}: {}",
                    report["error"].as_str().unwrap_or("unknown error"),
                ),
            }
        }
        println!("summary: {imported} imported, {deduplicated} deduplicated, {failed} failed");
    }
    if failed > 0 {
        return Err(CliError::Turn(format!(
            "{failed} local session file(s) failed to import"
        )));
    }
    Ok(())
}

fn parse_local_session_source(name: &str) -> Result<LocalSessionSource, CliError> {
    match name.trim().to_ascii_lowercase().as_str() {
        "claude" => Ok(LocalSessionSource::Claude),
        "codex" => Ok(LocalSessionSource::Codex),
        other => Err(CliError::Usage(format!(
            "unknown local session source '{other}' (claude|codex)"
        ))),
    }
}

fn compat_source_for(source: LocalSessionSource) -> pawork_storage::session::ExternalSource {
    match source {
        LocalSessionSource::Claude => pawork_storage::session::ExternalSource::Claude,
        LocalSessionSource::Codex => pawork_storage::session::ExternalSource::Codex,
    }
}

fn detect_session_format(
    path: &std::path::Path,
) -> Result<
    (
        SessionImportFormat,
        Option<pawork_storage::session::ExternalSource>,
    ),
    CliError,
> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if name.ends_with(".jsonl") {
        return match sniff_jsonl_session(path)? {
            Some((format, source)) => Ok((format, Some(source))),
            None => Ok((SessionImportFormat::Pi, None)),
        };
    }
    if name.ends_with(".export.json") {
        return Ok((SessionImportFormat::Export, None));
    }
    let text = std::fs::read_to_string(path)?;
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') {
        if trimmed.contains("\"schema_version\"") {
            return Ok((SessionImportFormat::Export, None));
        }
        return Ok((SessionImportFormat::Compat, None));
    }
    Err(CliError::Usage(
        "cannot detect session format; pass --format export|compat|pi".into(),
    ))
}

/// .jsonl 首非空行签名嗅探(逐行读取,读完首个完整非空行即停):Codex 信封(timestamp+type+payload)
/// → compat/codex;Claude 本地行(有 sessionId、无 payload、有 type;首行常为
/// ai-title/queue-operation 等无 message 行)→ compat/claude;签名不明确 → None(维持 Pi 默认)。
fn sniff_jsonl_session(
    path: &std::path::Path,
) -> Result<
    Option<(
        SessionImportFormat,
        pawork_storage::session::ExternalSource,
    )>,
    CliError,
> {
    use std::io::{BufRead as _, BufReader};

    let file = std::fs::File::open(path)?;
    let mut first = String::new();
    let mut reader = BufReader::new(file);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        if !line.trim().is_empty() {
            first = line;
            break;
        }
    }
    let first = first.trim();
    if first.is_empty() {
        return Ok(None);
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(first) else {
        return Ok(None);
    };
    let Some(obj) = value.as_object() else {
        return Ok(None);
    };
    if obj.contains_key("timestamp")
        && obj.contains_key("type")
        && obj.contains_key("payload")
    {
        return Ok(Some((
            SessionImportFormat::Compat,
            pawork_storage::session::ExternalSource::Codex,
        )));
    }
    // Claude Code 本地行:sessionId 必有,message 可缺(标题/噪声行先行);
    // payload 不存在用于排除 Codex 信封形态。
    if obj.contains_key("sessionId")
        && !obj.contains_key("payload")
        && obj.contains_key("type")
    {
        return Ok(Some((
            SessionImportFormat::Compat,
            pawork_storage::session::ExternalSource::Claude,
        )));
    }
    Ok(None)
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
    fn detects_jsonl_by_first_line_signature() {
        let dir = tempfile::tempdir().expect("tempdir");

        let codex = dir.path().join("rollout.jsonl");
        std::fs::write(
            &codex,
            "{\"timestamp\":\"2026-08-23T10:00:00.000Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"s\"}}\n",
        )
        .expect("codex");
        let (format, source) = detect_session_format(&codex).expect("codex detect");
        assert_eq!(format, SessionImportFormat::Compat);
        assert!(matches!(
            source,
            Some(pawork_storage::session::ExternalSource::Codex)
        ));

        let claude = dir.path().join("session.jsonl");
        std::fs::write(
            &claude,
            "{\"type\":\"user\",\"sessionId\":\"s1\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n",
        )
        .expect("claude");
        let (format, source) = detect_session_format(&claude).expect("claude detect");
        assert_eq!(format, SessionImportFormat::Compat);
        assert!(matches!(
            source,
            Some(pawork_storage::session::ExternalSource::Claude)
        ));

        let pi = dir.path().join("pi.jsonl");
        std::fs::write(&pi, "{\"type\":\"message\",\"role\":\"user\"}\n").expect("pi");
        let (format, source) = detect_session_format(&pi).expect("pi detect");
        assert_eq!(format, SessionImportFormat::Pi);
        assert!(source.is_none());
    }

    #[test]
    fn detects_claude_jsonl_when_first_line_has_no_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        std::fs::write(
            &path,
            "\n{\"type\":\"ai-title\",\"sessionId\":\"s1\",\"aiTitle\":\"synthetic\"}\n{\"type\":\"queue-operation\",\"sessionId\":\"s1\"}\n{\"type\":\"user\",\"sessionId\":\"s1\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n",
        )
        .expect("claude");
        let (format, source) = detect_session_format(&path).expect("detect");
        assert_eq!(format, SessionImportFormat::Compat);
        assert!(matches!(
            source,
            Some(pawork_storage::session::ExternalSource::Claude)
        ));
    }

    #[test]
    fn detects_jsonl_when_first_line_exceeds_8k() {
        let dir = tempfile::tempdir().expect("tempdir");
        let padding = "p".repeat(9 * 1024);

        // Codex rollout 首行 session_meta 常含超长 base_instructions(合成 padding)。
        let codex_meta = serde_json::json!({
            "timestamp": "2026-08-23T10:00:00.000Z",
            "type": "session_meta",
            "payload": {
                "id": "synthetic-session",
                "base_instructions": padding,
            },
        });
        let codex = dir.path().join("rollout-long.jsonl");
        std::fs::write(&codex, format!("{codex_meta}\n")).expect("codex");
        let (format, source) = detect_session_format(&codex).expect("codex detect");
        assert_eq!(format, SessionImportFormat::Compat);
        assert!(matches!(
            source,
            Some(pawork_storage::session::ExternalSource::Codex)
        ));

        // Claude 本地首行同理:超长合成标题不得因读取截断而误落 Pi 默认。
        let claude_title = serde_json::json!({
            "type": "ai-title",
            "sessionId": "synthetic-claude",
            "aiTitle": padding,
        });
        let claude = dir.path().join("session-long.jsonl");
        std::fs::write(&claude, format!("{claude_title}\n")).expect("claude");
        let (format, source) = detect_session_format(&claude).expect("claude detect");
        assert_eq!(format, SessionImportFormat::Compat);
        assert!(matches!(
            source,
            Some(pawork_storage::session::ExternalSource::Claude)
        ));
    }

    #[test]
    fn parse_local_session_source_accepts_only_supported_sources() {
        assert_eq!(
            parse_local_session_source("claude").unwrap(),
            LocalSessionSource::Claude
        );
        assert_eq!(
            parse_local_session_source("codex").unwrap(),
            LocalSessionSource::Codex
        );
        assert!(parse_local_session_source("grok").is_err());
    }

    #[test]
    fn format_millis_unix_epoch() {
        assert_eq!(format_millis(0), "1970-01-01 00:00:00Z");
        assert_eq!(format_millis(1_704_067_200_000), "2024-01-01 00:00:00Z");
    }
}
