use crate::gui_host::GuiHostAdapter;
use crate::gui_server::GuiHostError;
use pawork_protocol::{
    AppCommand, AppCommandEnvelope, AppQuery, AppResponse, SubagentModelRule, SubagentSettingsData,
};
use pawork_workspace::config::{SubagentConfig, SubagentModelConfig};
use std::collections::BTreeSet;

/// workspace 配置类型 → 协议设置载荷（字段一一对应，协议侧无省略键）。
fn settings_data_of(config: &SubagentConfig) -> SubagentSettingsData {
    SubagentSettingsData {
        enabled: config.enabled,
        max_concurrent: config.max_concurrent,
        models: config
            .models
            .iter()
            .map(|rule| SubagentModelRule {
                provider_id: rule.provider_id.clone(),
                model_id: rule.model_id.clone(),
                allow_spawn: rule.allow_spawn,
                allow_as_subagent: rule.allow_as_subagent,
                permissions: rule.permissions.clone(),
            })
            .collect(),
    }
}

/// 协议设置载荷 → workspace 配置类型（显式映射，不依赖 serde 默认值对齐）。
fn to_config(settings: &SubagentSettingsData) -> SubagentConfig {
    SubagentConfig {
        enabled: settings.enabled,
        max_concurrent: settings.max_concurrent,
        models: settings
            .models
            .iter()
            .map(|rule| SubagentModelConfig {
                provider_id: rule.provider_id.clone(),
                model_id: rule.model_id.clone(),
                allow_spawn: rule.allow_spawn,
                allow_as_subagent: rule.allow_as_subagent,
                permissions: rule.permissions.clone(),
            })
            .collect(),
    }
}

pub(crate) async fn settings(
    adapter: &GuiHostAdapter,
    _: &AppQuery,
) -> Result<AppResponse, GuiHostError> {
    let core = adapter.core.read().await;
    let config = core.config().subagents.clone().unwrap_or_default();
    // 协议类型始终输出 models（含空数组）；不经 workspace 配置类型序列化
    //（其 skip_serializing_if 会丢键，破坏 wire 形状）。
    Ok(AppResponse::Data(
        serde_json::to_value(settings_data_of(&config)).unwrap(),
    ))
}
pub(crate) async fn set_settings(
    adapter: &GuiHostAdapter,
    _: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::SetSubagentSettings { settings } = command else {
        unreachable!()
    };
    validate(settings)?;
    let path = pawork_workspace::config::global_config_path().ok_or_else(|| {
        GuiHostAdapter::host_error("config_unavailable", "global configuration unavailable")
    })?;
    // 与 set_terminal_settings 同序：先原子写盘，再短写锁同步内存；写后状态
    // 从下一轮 Run 生效（在途 Run 的工具集与权限已按启动时快照固定）。
    let config = to_config(settings);
    pawork_workspace::config::write_subagent_settings(&path, &config)
        .map_err(|e| GuiHostAdapter::host_error("config_write", e.to_string()))?;
    let mut core = adapter.core.write().await;
    core.config.subagents = Some(config);
    Ok(AppResponse::Data(serde_json::to_value(settings).unwrap()))
}
fn validate(settings: &SubagentSettingsData) -> Result<(), GuiHostError> {
    let invalid = || {
        GuiHostAdapter::host_error(
            "invalid_subagent_settings",
            "并发数须为 1–16；模型不可重复，权限必须来自可用功能列表。",
        )
    };
    if !(1..=16).contains(&settings.max_concurrent) || settings.models.len() > 512 {
        return Err(invalid());
    }
    let mut pairs = BTreeSet::new();
    for rule in &settings.models {
        if rule.provider_id.trim().is_empty()
            || rule.model_id.trim().is_empty()
            || rule.provider_id.len() > 256
            || rule.model_id.len() > 256
            || !pairs.insert((&rule.provider_id, &rule.model_id))
            || rule
                .permissions
                .iter()
                .any(|p| !crate::subagents::PERMISSIONS.contains(&p.as_str()))
        {
            return Err(invalid());
        }
    }
    Ok(())
}
pub(crate) async fn list(
    adapter: &GuiHostAdapter,
    query: &AppQuery,
) -> Result<AppResponse, GuiHostError> {
    let AppQuery::SubagentList { session_id } = query else {
        unreachable!()
    };
    let core = adapter.core.read().await;
    let list = core
        .list_subagents(session_id)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    Ok(AppResponse::Data(serde_json::to_value(list).unwrap()))
}
pub(crate) async fn cancel(
    adapter: &GuiHostAdapter,
    envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::SubagentCancel {
        session_id,
        agent_id,
    } = command
    else {
        unreachable!()
    };
    let core = adapter.core.read().await;
    core.cancel_subagent(session_id, agent_id)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    Ok(AppResponse::Accepted {
        command_id: envelope.command_id.clone(),
        run_id: None,
    })
}
