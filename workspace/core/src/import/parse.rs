//! 解析层：外部源文件 → canonical 条目。
//!
//! 解析绝不执行任何内容：hook / MCP / script 只被转写为待审配置，运行时是否
//! 启用由上层按 requires_review 决定。未知键只记录键名、不复制值；明文 Secret
//! 一律丢弃，只保留 credential reference 与占位记录；无法安全映射的内容显式
//! 标为 Unsupported，不静默放宽权限。

use std::collections::BTreeMap;

use crate::config::ConfigTier;
use pawork_domain::{
    AgentProfileV2, ProfileIsolation, ProfileMemory, ProfileModel, ProfilePrompt, ProfileToolRules,
    ReasoningEffort,
};
use pawork_policy::ApprovalMode;
use crate::resources::{ResourceInstructionKind, SkillManifest};
use semver::Version;
use serde_json::Value;

use super::hook::{
    CommandHandler, HandlerConfig, HandlerLifecycle, HookConfig, HookScope, TriggerPoint,
};
use super::mcp::{McpServerConfig, SecretRef as McpSecretRef, TransportSpec};

use super::detect::DetectedFile;
use super::frontmatter::split_frontmatter;
use super::model::{
    CompatIssue, CompatItem, CompatPayload, CredentialReference, ImportCategory, ImportSource,
    ImportStatus, PendingCredential, PermissionDecision,
};
use super::source::SourceFileKind;

/// 单个解析产物：一条条目 + 随条目产生的 credential references。
#[derive(Clone, Debug)]
pub(crate) struct ParseOutcome {
    pub item: CompatItem,
    pub credentials: Vec<CredentialReference>,
}

/// 解析一个探测文件（内容已由调用方安全读取）。
/// 文件级诊断写入 issues，条目级诊断挂在条目上。
pub(crate) fn parse_content(
    file: &DetectedFile,
    content: &str,
    hook_scope: HookScope,
    issues: &mut Vec<CompatIssue>,
    outcomes: &mut Vec<ParseOutcome>,
) {
    match file.kind {
        SourceFileKind::InstructionsDoc => instructions(file, content, outcomes),
        SourceFileKind::ClaudeSettings => {
            claude_settings(file, content, hook_scope, issues, outcomes)
        }
        SourceFileKind::ConfigToml => config_toml(file, content, issues, outcomes),
        SourceFileKind::McpJson => mcp_json(file, content, issues, outcomes),
        SourceFileKind::SkillMarkdown => skill_markdown(file, content, outcomes),
        SourceFileKind::AgentMarkdown => agent_markdown(file, content, outcomes),
        SourceFileKind::AgentsJson => agents_json(file, content, issues, outcomes),
        SourceFileKind::PiSettings => pi_settings(file, content, issues, outcomes),
    }
}

// —— 公共辅助 ——

fn base_item(file: &DetectedFile, category: ImportCategory, id: String) -> CompatItem {
    CompatItem {
        id,
        category,
        status: ImportStatus::Imported,
        source: ImportSource::new(file.primary(), file.tier, file.relative_path.clone()),
        requires_review: false,
        payload: None,
        issues: Vec::new(),
    }
}

fn unsupported_item(
    file: &DetectedFile,
    category: ImportCategory,
    id: String,
    code: &str,
    message: String,
) -> CompatItem {
    let mut item = base_item(file, category, id.clone());
    item.status = ImportStatus::Unsupported;
    item.issues
        .push(CompatIssue::warning(code, message).for_item(
            category,
            id,
            file.relative_path.clone(),
        ));
    item
}

fn push(outcomes: &mut Vec<ParseOutcome>, item: CompatItem) {
    outcomes.push(ParseOutcome {
        item,
        credentials: Vec::new(),
    });
}

fn warn_file(issues: &mut Vec<CompatIssue>, file: &DetectedFile, code: &str, message: String) {
    issues.push(CompatIssue::warning(code, message).with_source(file.relative_path.clone()));
}

fn sanitize_id(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    cleaned.trim_matches('-').to_string()
}

/// SKILL.md / AGENT.md 的目录名兜底 id（如 docs-writer）。
fn skill_dir_name(relative_path: &str) -> String {
    relative_path
        .rsplit('/')
        .nth(1)
        .unwrap_or("skill")
        .to_string()
}

fn split_tool_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// 工具清单 → (allowed, denied)：`!x` / `-x` 前缀表示 denied，deny 优先。
fn classify_tools(tools: &[String]) -> (Vec<String>, Vec<String>) {
    let mut allowed: Vec<String> = Vec::new();
    let mut denied: Vec<String> = Vec::new();
    for raw in tools {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (is_denied, rest) = if let Some(rest) = trimmed.strip_prefix('!') {
            (true, rest)
        } else if let Some(rest) = trimmed.strip_prefix('-') {
            (true, rest)
        } else {
            (false, trimmed)
        };
        let name = rest
            .split('(')
            .next()
            .unwrap_or(rest)
            .trim()
            .trim_end_matches('*');
        if name.is_empty() {
            continue;
        }
        if is_denied {
            denied.push(name.to_string());
        } else {
            allowed.push(name.to_string());
        }
    }
    allowed.sort();
    allowed.dedup();
    denied.sort();
    denied.dedup();
    allowed.retain(|name| !denied.contains(name));
    (allowed, denied)
}

fn extract_string_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_json(
    file: &DetectedFile,
    content: &str,
    outcomes: &mut Vec<ParseOutcome>,
) -> Option<Value> {
    match serde_json::from_str::<Value>(content) {
        Ok(value) => Some(value),
        Err(_) => {
            push(
                outcomes,
                unsupported_item(
                    file,
                    ImportCategory::Instructions,
                    format!("parse:{}", file.relative_path),
                    "json_parse_failed",
                    "source file is not valid JSON".to_string(),
                ),
            );
            None
        }
    }
}

fn build_profile(
    file: &DetectedFile,
    name: &str,
    description: &str,
    system: &str,
    model: Option<String>,
    tools: &[String],
) -> AgentProfileV2 {
    let (allowed, denied) = classify_tools(tools);
    AgentProfileV2 {
        name: name.to_string(),
        prompt: ProfilePrompt {
            system: system.to_string(),
            instructions: (!description.is_empty()).then(|| description.to_string()),
        },
        model: ProfileModel {
            provider: Some(file.primary().as_str().to_string()),
            name: model,
        },
        effort: ReasoningEffort::Medium,
        tools: ProfileToolRules { allowed, denied },
        skills: Vec::new(),
        mcp: Vec::new(),
        permissions: Vec::new(),
        hooks: Vec::new(),
        memory: ProfileMemory::default(),
        max_turns: None,
        background: false,
        isolation: ProfileIsolation::None,
    }
}

/// PascalCase / camelCase / kebab-case → snake_case。
fn to_snake(input: &str) -> String {
    let mut out = String::new();
    for (index, c) in input.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else if c == '-' || c == ' ' {
            out.push('_');
        } else {
            out.push(c);
        }
    }
    out
}

/// 外部 hook 事件名 → canonical TriggerPoint；无法安全映射返回 None。
fn map_trigger(event: &str) -> Option<TriggerPoint> {
    match to_snake(event).as_str() {
        "session_start" => Some(TriggerPoint::SessionStart),
        "session_end" => Some(TriggerPoint::SessionEnd),
        "run_started" => Some(TriggerPoint::RunStarted),
        "run_completed" => Some(TriggerPoint::RunCompleted),
        "run_failed" => Some(TriggerPoint::RunFailed),
        "prompt_assembled" => Some(TriggerPoint::PromptAssembled),
        "pre_tool_use" => Some(TriggerPoint::PreToolUse),
        "post_tool_use" => Some(TriggerPoint::PostToolUse),
        "tool_failed" => Some(TriggerPoint::ToolFailed),
        "permission_request" => Some(TriggerPoint::PermissionRequest),
        "subagent_start" => Some(TriggerPoint::SubagentStart),
        "subagent_stop" => Some(TriggerPoint::SubagentStop),
        "task_started" => Some(TriggerPoint::TaskStarted),
        "task_completed" => Some(TriggerPoint::TaskCompleted),
        "pre_compact" => Some(TriggerPoint::PreCompact),
        "post_compact" => Some(TriggerPoint::PostCompact),
        "notification" => Some(TriggerPoint::Notification),
        _ => None,
    }
}

/// env / headers 值映射：`${VAR}` / `$VAR` 变成 SecretRef；字面量视为明文
/// Secret 丢弃并记录占位（只保留名称与位置）；结构异常只出诊断。
fn env_interpolation(text: &str) -> Option<&str> {
    if let Some(rest) = text.strip_prefix("${") {
        rest.strip_suffix('}')
    } else {
        text.strip_prefix('$')
    }
}

struct SecretMapping {
    map: BTreeMap<String, McpSecretRef>,
    pending: Vec<PendingCredential>,
    references: Vec<CredentialReference>,
    issues: Vec<CompatIssue>,
}

fn secret_mapping(
    file: &DetectedFile,
    server: &str,
    value: Option<&Value>,
    section: &str,
) -> SecretMapping {
    let mut mapping = SecretMapping {
        map: BTreeMap::new(),
        pending: Vec::new(),
        references: Vec::new(),
        issues: Vec::new(),
    };
    let Some(entries) = value.and_then(Value::as_object) else {
        return mapping;
    };
    let item_id = format!("mcp:{server}");
    for (key, entry) in entries {
        let location = format!("servers.{server}.{section}.{key}");
        match entry {
            Value::String(text) => {
                if let Some(inner) = env_interpolation(text) {
                    if !inner.is_empty() {
                        mapping.map.insert(
                            key.clone(),
                            McpSecretRef::new("mcp", format!("{server}:{key}")),
                        );
                        mapping.references.push(CredentialReference {
                            source: ImportSource::new(
                                file.primary(),
                                file.tier,
                                file.relative_path.clone(),
                            ),
                            service: "mcp".to_string(),
                            account: format!("{server}:{key}"),
                            location,
                        });
                    }
                } else if !text.is_empty() {
                    mapping.pending.push(PendingCredential {
                        name: key.clone(),
                        location,
                    });
                    mapping.issues.push(
                        CompatIssue::warning(
                            "plaintext_secret_rejected",
                            "literal value dropped; only the name is recorded",
                        )
                        .for_item(
                            ImportCategory::McpServer,
                            item_id.clone(),
                            file.relative_path.clone(),
                        ),
                    );
                }
            }
            Value::Null => {}
            _ => mapping.issues.push(
                CompatIssue::warning(
                    "mcp_secret_shape",
                    format!("unexpected shape for {section} entry; skipped"),
                )
                .for_item(
                    ImportCategory::McpServer,
                    item_id.clone(),
                    file.relative_path.clone(),
                ),
            ),
        }
    }
    mapping
}

// —— Instructions ——

fn instructions(file: &DetectedFile, content: &str, outcomes: &mut Vec<ParseOutcome>) {
    let kind = if file.tier == ConfigTier::Global {
        ResourceInstructionKind::UserGlobalInstructions
    } else {
        ResourceInstructionKind::WorkspaceInstructions
    };
    let depth = file.relative_path.split('/').count() as u32;
    let mut item = base_item(
        file,
        ImportCategory::Instructions,
        format!("instructions:{}", file.relative_path),
    );
    item.payload = Some(CompatPayload::Instructions {
        instruction_kind: kind,
        body: content.to_string(),
        depth,
    });
    push(outcomes, item);
}

// —— Skill / Agent markdown ——

fn skill_markdown(file: &DetectedFile, content: &str, outcomes: &mut Vec<ParseOutcome>) {
    let frontmatter = split_frontmatter(content);
    let raw_id = frontmatter
        .scalars
        .get("name")
        .cloned()
        .unwrap_or_else(|| skill_dir_name(&file.relative_path));
    let id = sanitize_id(&raw_id);
    if id.is_empty() {
        push(
            outcomes,
            unsupported_item(
                file,
                ImportCategory::Skill,
                format!("skill:{}", file.relative_path),
                "skill_id_missing",
                "SKILL.md has no usable name".to_string(),
            ),
        );
        return;
    }
    let version = frontmatter
        .scalars
        .get("version")
        .and_then(|text| Version::parse(text).ok())
        .unwrap_or_else(|| Version::new(0, 1, 0));
    let description = frontmatter
        .scalars
        .get("description")
        .cloned()
        .unwrap_or_default();
    let permissions = frontmatter
        .scalars
        .get("allowed-tools")
        .map(|text| split_tool_list(text))
        .unwrap_or_default();
    let mut item = base_item(file, ImportCategory::Skill, format!("skill:{id}"));
    for key in &frontmatter.complex_keys {
        item.issues.push(
            CompatIssue::warning(
                "skill_key_unmapped",
                format!("skill frontmatter key not imported: {key}"),
            )
            .for_item(
                ImportCategory::Skill,
                item.id.clone(),
                file.relative_path.clone(),
            ),
        );
    }
    item.payload = Some(CompatPayload::Skill {
        manifest: SkillManifest {
            id: id.clone(),
            version,
            description,
            parameters: Vec::new(),
            dependencies: Vec::new(),
            conflicts: Vec::new(),
            scripts: Vec::new(),
            assets: Vec::new(),
            permissions,
        },
        body: frontmatter.body,
    });
    push(outcomes, item);
}

fn agent_markdown(file: &DetectedFile, content: &str, outcomes: &mut Vec<ParseOutcome>) {
    let frontmatter = split_frontmatter(content);
    let raw_name = frontmatter
        .scalars
        .get("name")
        .cloned()
        .unwrap_or_else(|| skill_dir_name(&file.relative_path));
    let name = sanitize_id(&raw_name);
    if name.is_empty() {
        push(
            outcomes,
            unsupported_item(
                file,
                ImportCategory::AgentProfile,
                format!("agent:{}", file.relative_path),
                "agent_name_missing",
                "agent markdown has no usable name".to_string(),
            ),
        );
        return;
    }
    let description = frontmatter
        .scalars
        .get("description")
        .cloned()
        .unwrap_or_default();
    let model = frontmatter.scalars.get("model").cloned();
    let tools = frontmatter
        .scalars
        .get("tools")
        .map(|text| split_tool_list(text))
        .unwrap_or_default();
    let profile = build_profile(
        file,
        &name,
        &description,
        frontmatter.body.trim(),
        model,
        &tools,
    );
    let mut item = base_item(file, ImportCategory::AgentProfile, format!("agent:{name}"));
    for key in &frontmatter.complex_keys {
        item.issues.push(
            CompatIssue::warning(
                "agent_key_unmapped",
                format!("agent frontmatter key not imported: {key}"),
            )
            .for_item(
                ImportCategory::AgentProfile,
                item.id.clone(),
                file.relative_path.clone(),
            ),
        );
    }
    item.payload = Some(CompatPayload::AgentProfile { profile });
    push(outcomes, item);
}

// —— agents.json ——

fn agents_json(
    file: &DetectedFile,
    content: &str,
    issues: &mut Vec<CompatIssue>,
    outcomes: &mut Vec<ParseOutcome>,
) {
    let Some(root) = parse_json(file, content, outcomes) else {
        return;
    };
    let Some(obj) = root.as_object() else {
        push(
            outcomes,
            unsupported_item(
                file,
                ImportCategory::AgentProfile,
                format!("agents:{}", file.relative_path),
                "agents_json_shape",
                "agents.json root is not an object".to_string(),
            ),
        );
        return;
    };
    for key in obj.keys() {
        match key.as_str() {
            "agents" | "recommended" => {}
            _ => warn_file(
                issues,
                file,
                "unknown_key",
                format!("unknown agents.json key: {key}"),
            ),
        }
    }
    if let Some(agents) = obj.get("agents").and_then(Value::as_object) {
        for (name, spec) in agents {
            agent_from_json(file, name, spec, issues, outcomes);
        }
    }
}

fn agent_from_json(
    file: &DetectedFile,
    name: &str,
    spec: &Value,
    issues: &mut Vec<CompatIssue>,
    outcomes: &mut Vec<ParseOutcome>,
) {
    let clean = sanitize_id(name);
    if clean.is_empty() {
        warn_file(
            issues,
            file,
            "agent_name_missing",
            "agent entry has no usable name".to_string(),
        );
        return;
    }
    let Some(obj) = spec.as_object() else {
        push(
            outcomes,
            unsupported_item(
                file,
                ImportCategory::AgentProfile,
                format!("agent:{clean}"),
                "agent_spec_shape",
                "agent entry is not an object".to_string(),
            ),
        );
        return;
    };
    let id = format!("agent:{clean}");
    let mut item = base_item(file, ImportCategory::AgentProfile, id.clone());
    let mut item_issues = Vec::new();
    for key in obj.keys() {
        match key.as_str() {
            "description" | "instructions" | "prompt" | "model" | "tools" => {}
            "hooks" => {
                item.requires_review = true;
                item_issues.push(
                    CompatIssue::warning(
                        "agent_hooks_not_imported",
                        "agent hooks are not imported or executed",
                    )
                    .for_item(
                        ImportCategory::AgentProfile,
                        id.clone(),
                        file.relative_path.clone(),
                    ),
                );
            }
            _ => item_issues.push(
                CompatIssue::warning(
                    "agent_key_unmapped",
                    format!("agent key not imported: {key}"),
                )
                .for_item(
                    ImportCategory::AgentProfile,
                    id.clone(),
                    file.relative_path.clone(),
                ),
            ),
        }
    }
    let description = obj
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let system = obj
        .get("prompt")
        .or_else(|| obj.get("instructions"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let model = obj.get("model").and_then(Value::as_str).map(str::to_string);
    let tools = obj
        .get("tools")
        .map(extract_string_list)
        .unwrap_or_default();
    let profile = build_profile(file, &clean, &description, &system, model, &tools);
    item.payload = Some(CompatPayload::AgentProfile { profile });
    item.issues = item_issues;
    push(outcomes, item);
}

// —— MCP ——

fn mcp_json(
    file: &DetectedFile,
    content: &str,
    issues: &mut Vec<CompatIssue>,
    outcomes: &mut Vec<ParseOutcome>,
) {
    let Some(root) = parse_json(file, content, outcomes) else {
        return;
    };
    let Some(obj) = root.as_object() else {
        push(
            outcomes,
            unsupported_item(
                file,
                ImportCategory::McpServer,
                format!("mcpfile:{}", file.relative_path),
                "mcp_json_shape",
                "mcp json root is not an object".to_string(),
            ),
        );
        return;
    };
    for key in obj.keys() {
        if key != "mcpServers" {
            warn_file(
                issues,
                file,
                "unknown_key",
                format!("unknown mcp json key: {key}"),
            );
        }
    }
    if let Some(servers) = obj.get("mcpServers").and_then(Value::as_object) {
        for (name, spec) in servers {
            mcp_server_entry(file, name, spec, issues, outcomes);
        }
    }
}

fn mcp_server_entry(
    file: &DetectedFile,
    name: &str,
    spec: &Value,
    issues: &mut Vec<CompatIssue>,
    outcomes: &mut Vec<ParseOutcome>,
) {
    let id = format!("mcp:{name}");
    let Some(obj) = spec.as_object() else {
        push(
            outcomes,
            unsupported_item(
                file,
                ImportCategory::McpServer,
                id,
                "mcp_entry_shape",
                "server entry is not an object".to_string(),
            ),
        );
        return;
    };
    for key in obj.keys() {
        if !matches!(
            key.as_str(),
            "command" | "args" | "env" | "url" | "headers" | "type" | "trusted"
        ) {
            warn_file(
                issues,
                file,
                "mcp_key_unmapped",
                format!("server entry key not imported: {key}"),
            );
        }
    }
    let transport_type = obj.get("type").and_then(Value::as_str).unwrap_or("");
    if transport_type == "sse" || transport_type == "streamable-http" {
        push(
            outcomes,
            unsupported_item(
                file,
                ImportCategory::McpServer,
                id,
                "mcp_transport_unsupported",
                format!("transport type not importable: {transport_type}"),
            ),
        );
        return;
    }
    if let Some(command) = obj.get("command").and_then(Value::as_str) {
        let args = obj.get("args").map(extract_string_list).unwrap_or_default();
        let mapping = secret_mapping(file, name, obj.get("env"), "env");
        let mut item = base_item(file, ImportCategory::McpServer, id.clone());
        item.requires_review = true;
        item.issues = mapping.issues;
        item.payload = Some(CompatPayload::McpServer {
            name: name.to_string(),
            server: McpServerConfig {
                transport: TransportSpec::Stdio {
                    command: command.to_string(),
                    args,
                    env: mapping.map,
                },
                auto_start: false,
                timeout_ms: None,
                restart: Default::default(),
                permissions: Default::default(),
                trusted: false,
            },
            pending_credentials: mapping.pending,
        });
        outcomes.push(ParseOutcome {
            item,
            credentials: mapping.references,
        });
        return;
    }
    if let Some(url) = obj.get("url").and_then(Value::as_str) {
        let mapping = secret_mapping(file, name, obj.get("headers"), "headers");
        let mut item = base_item(file, ImportCategory::McpServer, id.clone());
        item.requires_review = true;
        item.issues = mapping.issues;
        item.payload = Some(CompatPayload::McpServer {
            name: name.to_string(),
            server: McpServerConfig {
                transport: TransportSpec::Http {
                    url: url.to_string(),
                    headers: mapping.map,
                },
                auto_start: false,
                timeout_ms: None,
                restart: Default::default(),
                permissions: Default::default(),
                trusted: false,
            },
            pending_credentials: mapping.pending,
        });
        outcomes.push(ParseOutcome {
            item,
            credentials: mapping.references,
        });
        return;
    }
    push(
        outcomes,
        unsupported_item(
            file,
            ImportCategory::McpServer,
            id,
            "mcp_transport_unknown",
            "no importable transport (command/url) found".to_string(),
        ),
    );
}

// —— Codex / Grok config.toml ——

fn config_toml(
    file: &DetectedFile,
    content: &str,
    issues: &mut Vec<CompatIssue>,
    outcomes: &mut Vec<ParseOutcome>,
) {
    let toml_value: toml::Value = match toml::from_str(content) {
        Ok(value) => value,
        Err(_) => {
            push(
                outcomes,
                unsupported_item(
                    file,
                    ImportCategory::Instructions,
                    format!("parse:{}", file.relative_path),
                    "toml_parse_failed",
                    "source file is not valid TOML".to_string(),
                ),
            );
            return;
        }
    };
    let json = match serde_json::to_value(&toml_value) {
        Ok(value) => value,
        Err(_) => {
            warn_file(
                issues,
                file,
                "toml_convert_failed",
                "TOML could not be converted for import".to_string(),
            );
            return;
        }
    };
    let Some(obj) = json.as_object() else {
        warn_file(
            issues,
            file,
            "toml_root_shape",
            "config root is not a table".to_string(),
        );
        return;
    };
    for key in obj.keys() {
        match key.as_str() {
            "mcp_servers" => {}
            "approval_policy" => codex_approval(file, obj.get("approval_policy"), issues, outcomes),
            "hooks" => warn_file(
                issues,
                file,
                "hooks_not_imported",
                "external hooks config is not imported or executed".to_string(),
            ),
            "model" | "model_provider" | "sandbox_mode" | "projects" | "profiles" => {
                warn_file(
                    issues,
                    file,
                    "known_unmapped",
                    format!("config key recognized but not importable: {key}"),
                );
            }
            _ => warn_file(
                issues,
                file,
                "unknown_key",
                format!("unknown config key: {key}"),
            ),
        }
    }
    if let Some(servers) = obj.get("mcp_servers").and_then(Value::as_object) {
        for (name, spec) in servers {
            mcp_server_entry(file, name, spec, issues, outcomes);
        }
    }
}

fn codex_approval(
    file: &DetectedFile,
    policy: Option<&Value>,
    issues: &mut Vec<CompatIssue>,
    outcomes: &mut Vec<ParseOutcome>,
) {
    let Some(text) = policy.and_then(Value::as_str) else {
        warn_file(
            issues,
            file,
            "approval_policy_shape",
            "approval_policy is not a string".to_string(),
        );
        return;
    };
    match text {
        "on-request" => global_permission(file, ApprovalMode::AlwaysAsk, outcomes),
        "on-failure" => global_permission(file, ApprovalMode::OnFailure, outcomes),
        "never" => push(
            outcomes,
            unsupported_item(
                file,
                ImportCategory::PermissionRule,
                "permission:global".to_string(),
                "approval_never_not_imported",
                "policy that never asks is not imported".to_string(),
            ),
        ),
        other => push(
            outcomes,
            unsupported_item(
                file,
                ImportCategory::PermissionRule,
                "permission:global".to_string(),
                "approval_policy_unknown",
                format!("approval policy not importable: {other}"),
            ),
        ),
    }
}

fn global_permission(file: &DetectedFile, mode: ApprovalMode, outcomes: &mut Vec<ParseOutcome>) {
    let decision = match mode {
        ApprovalMode::ReadOnly => PermissionDecision::Deny,
        ApprovalMode::AlwaysAsk | ApprovalMode::AskForWrites | ApprovalMode::AskForDangerous => {
            PermissionDecision::Ask
        }
        // OnFailure 当前等价 NeverAsk（S13-F16 收窄）。引擎自动放行，但导入
        // 不映成 Allow（绝不静默放宽权限）：与 NeverAsk 同为 Ask，并保留 requires_review。
        ApprovalMode::OnFailure | ApprovalMode::NeverAsk => PermissionDecision::Ask,
    };
    let mut item = base_item(
        file,
        ImportCategory::PermissionRule,
        "permission:global".to_string(),
    );
    item.requires_review = true;
    item.payload = Some(CompatPayload::PermissionRule {
        tool: None,
        decision,
        spec: None,
        approval_mode: Some(mode),
    });
    push(outcomes, item);
}

// —— Claude settings.json ——

fn claude_settings(
    file: &DetectedFile,
    content: &str,
    hook_scope: HookScope,
    issues: &mut Vec<CompatIssue>,
    outcomes: &mut Vec<ParseOutcome>,
) {
    let Some(root) = parse_json(file, content, outcomes) else {
        return;
    };
    let Some(obj) = root.as_object() else {
        push(
            outcomes,
            unsupported_item(
                file,
                ImportCategory::Instructions,
                format!("parse:{}", file.relative_path),
                "claude_settings_shape",
                "settings root is not an object".to_string(),
            ),
        );
        return;
    };
    for key in obj.keys() {
        match key.as_str() {
            "$schema" | "permissions" | "hooks" => {}
            "env" => {
                let count = obj
                    .get("env")
                    .and_then(Value::as_object)
                    .map(|entries| entries.len())
                    .unwrap_or(0);
                warn_file(
                    issues,
                    file,
                    "env_not_imported",
                    format!("{count} env entries skipped; values never copied"),
                );
            }
            "enableAllProjectMcpServers" | "enabledMcpjsonServers" | "enabledMcpServers" => {
                warn_file(
                    issues,
                    file,
                    "known_unmapped",
                    format!("settings key recognized but not importable: {key}"),
                );
            }
            _ => warn_file(
                issues,
                file,
                "unknown_key",
                format!("unknown settings key: {key}"),
            ),
        }
    }
    if let Some(permissions) = obj.get("permissions") {
        claude_permissions(file, permissions, issues, outcomes);
    }
    if let Some(hooks) = obj.get("hooks") {
        claude_hooks(file, hooks, hook_scope, issues, outcomes);
    }
}

fn claude_permissions(
    file: &DetectedFile,
    permissions: &Value,
    issues: &mut Vec<CompatIssue>,
    outcomes: &mut Vec<ParseOutcome>,
) {
    let Some(obj) = permissions.as_object() else {
        warn_file(
            issues,
            file,
            "permissions_shape",
            "permissions section is not an object".to_string(),
        );
        return;
    };
    for key in obj.keys() {
        match key.as_str() {
            "allow"
            | "ask"
            | "deny"
            | "defaultMode"
            | "additionalDirectories"
            | "disableBypassPermissionsMode" => {}
            _ => warn_file(
                issues,
                file,
                "permission_key_unmapped",
                format!("permissions key not imported: {key}"),
            ),
        }
    }
    for (list, decision) in [
        ("allow", PermissionDecision::Allow),
        ("ask", PermissionDecision::Ask),
        ("deny", PermissionDecision::Deny),
    ] {
        let Some(rules) = obj.get(list).and_then(Value::as_array) else {
            continue;
        };
        for raw in rules {
            let Some(rule) = raw.as_str() else {
                warn_file(
                    issues,
                    file,
                    "permission_rule_shape",
                    format!("{list} rule is not a string"),
                );
                continue;
            };
            let tool = rule
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .next()
                .unwrap_or("")
                .trim_matches('*');
            if tool.is_empty() {
                warn_file(
                    issues,
                    file,
                    "permission_rule_empty",
                    format!("{list} rule has no tool name"),
                );
                continue;
            }
            let mut item = base_item(
                file,
                ImportCategory::PermissionRule,
                format!("permission:{list}:{tool}"),
            );
            item.requires_review = decision != PermissionDecision::Deny;
            item.payload = Some(CompatPayload::PermissionRule {
                tool: Some(tool.to_string()),
                decision,
                spec: None,
                approval_mode: None,
            });
            push(outcomes, item);
        }
    }
    if let Some(mode) = obj.get("defaultMode").and_then(Value::as_str) {
        match mode {
            "default" | "plan" => global_permission(file, ApprovalMode::AskForWrites, outcomes),
            "acceptEdits" => global_permission(file, ApprovalMode::OnFailure, outcomes),
            "bypassPermissions" => push(
                outcomes,
                unsupported_item(
                    file,
                    ImportCategory::PermissionRule,
                    "permission:global".to_string(),
                    "permission_bypass_not_imported",
                    "bypass mode is never imported".to_string(),
                ),
            ),
            other => push(
                outcomes,
                unsupported_item(
                    file,
                    ImportCategory::PermissionRule,
                    "permission:global".to_string(),
                    "permission_mode_unknown",
                    format!("defaultMode not importable: {other}"),
                ),
            ),
        }
    }
}

fn claude_hooks(
    file: &DetectedFile,
    hooks: &Value,
    hook_scope: HookScope,
    issues: &mut Vec<CompatIssue>,
    outcomes: &mut Vec<ParseOutcome>,
) {
    let Some(obj) = hooks.as_object() else {
        warn_file(
            issues,
            file,
            "hooks_shape",
            "hooks section is not an object".to_string(),
        );
        return;
    };
    for (event, groups) in obj {
        let Some(groups) = groups.as_array() else {
            warn_file(
                issues,
                file,
                "hook_groups_shape",
                format!("hook event groups not importable: {event}"),
            );
            continue;
        };
        let trigger = map_trigger(event);
        for (group_index, group) in groups.iter().enumerate() {
            let has_matcher = group.get("matcher").is_some();
            let Some(entries) = group.get("hooks").and_then(Value::as_array) else {
                warn_file(
                    issues,
                    file,
                    "hook_group_shape",
                    "hook group has no hooks list".to_string(),
                );
                continue;
            };
            for (entry_index, entry) in entries.iter().enumerate() {
                let id = format!("hook:{event}:{group_index}:{entry_index}");
                let Some(trigger) = trigger else {
                    push(
                        outcomes,
                        unsupported_item(
                            file,
                            ImportCategory::UserHook,
                            id,
                            "hook_event_unsupported",
                            format!("hook event not importable: {event}"),
                        ),
                    );
                    continue;
                };
                let Some(entry_obj) = entry.as_object() else {
                    warn_file(
                        issues,
                        file,
                        "hook_entry_shape",
                        "hook entry is not an object".to_string(),
                    );
                    continue;
                };
                let handler_type = entry_obj.get("type").and_then(Value::as_str).unwrap_or("");
                if handler_type == "prompt" {
                    push(
                        outcomes,
                        unsupported_item(
                            file,
                            ImportCategory::UserHook,
                            id,
                            "hook_handler_unsupported",
                            "prompt hooks are not imported".to_string(),
                        ),
                    );
                    continue;
                }
                if handler_type != "command" {
                    push(
                        outcomes,
                        unsupported_item(
                            file,
                            ImportCategory::UserHook,
                            id,
                            "hook_handler_unknown",
                            format!("handler type not importable: {handler_type}"),
                        ),
                    );
                    continue;
                }
                let Some(command) = entry_obj.get("command").and_then(Value::as_str) else {
                    push(
                        outcomes,
                        unsupported_item(
                            file,
                            ImportCategory::UserHook,
                            id,
                            "hook_command_missing",
                            "command hook has no command string".to_string(),
                        ),
                    );
                    continue;
                };
                let mut tokens = command.split_whitespace();
                let Some(program) = tokens.next() else {
                    push(
                        outcomes,
                        unsupported_item(
                            file,
                            ImportCategory::UserHook,
                            id,
                            "hook_command_empty",
                            "command hook has an empty command".to_string(),
                        ),
                    );
                    continue;
                };
                let mut item = base_item(file, ImportCategory::UserHook, id.clone());
                item.requires_review = true;
                let mut item_issues = Vec::new();
                if has_matcher {
                    item_issues.push(
                        CompatIssue::warning(
                            "hook_matcher_not_imported",
                            "hook matcher condition is not imported",
                        )
                        .for_item(
                            ImportCategory::UserHook,
                            id.clone(),
                            file.relative_path.clone(),
                        ),
                    );
                }
                for key in entry_obj.keys() {
                    if !matches!(key.as_str(), "type" | "command" | "timeout") {
                        item_issues.push(
                            CompatIssue::warning(
                                "hook_key_unmapped",
                                format!("hook key not imported: {key}"),
                            )
                            .for_item(
                                ImportCategory::UserHook,
                                id.clone(),
                                file.relative_path.clone(),
                            ),
                        );
                    }
                }
                item.payload = Some(CompatPayload::UserHook {
                    hook: HookConfig {
                        id: id.clone(),
                        trigger,
                        scope: hook_scope.clone(),
                        lifecycle: Some(HandlerLifecycle::Async),
                        enabled: false,
                        handler: HandlerConfig::Command(CommandHandler {
                            program: program.to_string(),
                            args: tokens.map(str::to_string).collect(),
                            allowed_env: Vec::new(),
                            env_secret_refs: Vec::new(),
                            working_directory: None,
                            timeout_ms: None,
                        }),
                    },
                });
                item.issues = item_issues;
                push(outcomes, item);
            }
        }
    }
}

// —— Pi settings.json ——

fn pi_settings(
    file: &DetectedFile,
    content: &str,
    issues: &mut Vec<CompatIssue>,
    outcomes: &mut Vec<ParseOutcome>,
) {
    let Some(root) = parse_json(file, content, outcomes) else {
        return;
    };
    let Some(obj) = root.as_object() else {
        push(
            outcomes,
            unsupported_item(
                file,
                ImportCategory::Instructions,
                format!("parse:{}", file.relative_path),
                "pi_settings_shape",
                "pi settings root is not an object".to_string(),
            ),
        );
        return;
    };
    for key in obj.keys() {
        match key.as_str() {
            "instructions" | "mcpServers" | "mcp" => {}
            "hooks" => warn_file(
                issues,
                file,
                "hooks_not_imported",
                "external hooks config is not imported or executed".to_string(),
            ),
            _ => warn_file(
                issues,
                file,
                "unknown_key",
                format!("unknown pi settings key: {key}"),
            ),
        }
    }
    if let Some(text) = obj.get("instructions").and_then(Value::as_str) {
        let mut item = base_item(
            file,
            ImportCategory::Instructions,
            format!("instructions:{}", file.relative_path),
        );
        item.payload = Some(CompatPayload::Instructions {
            instruction_kind: ResourceInstructionKind::WorkspaceInstructions,
            body: text.to_string(),
            depth: 0,
        });
        push(outcomes, item);
    }
    for key in ["mcpServers", "mcp"] {
        if let Some(servers) = obj.get(key).and_then(Value::as_object) {
            for (name, spec) in servers {
                mcp_server_entry(file, name, spec, issues, outcomes);
            }
        }
    }
}
