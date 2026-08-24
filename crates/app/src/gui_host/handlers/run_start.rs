use std::sync::atomic::Ordering;
use std::sync::Arc;

use pawork_domain::{
    CancellationToken, ErrorCategory, ErrorContext, Message, MessageId, MessageRole, RunId,
};
use pawork_engine::now_timestamp;
use pawork_protocol::{AppCommand, AppCommandEnvelope, AppEvent, AppResponse, RunState};
use serde_json::json;

use crate::gui_server::GuiHostError;

use super::super::{ActiveGuiRun, GuiBroadcastSink, GuiHostAdapter};

/// 有 `RunStart.provider` 时按用户所选通道切换，禁止回退 catalog 首项。
pub(crate) fn run_start_requested_provider_switch(
    current_provider: &str,
    current_model: &str,
    requested_provider: Option<&str>,
    requested_model: Option<&str>,
) -> Option<(String, Option<String>)> {
    let provider = requested_provider?;
    let already = current_provider == provider
        && requested_model.is_none_or(|model| current_model == model);
    if already {
        None
    } else {
        Some((provider.to_string(), requested_model.map(str::to_string)))
    }
}

/// 旧客户端兼容：仅有 model 时按 overview 顺序取首个同 id。
#[cfg(test)]
pub(crate) fn run_start_overview_owner<'a, P, M>(
    model: &str,
    overview: impl IntoIterator<Item = &'a (P, M)>,
) -> Option<String>
where
    P: AsRef<str> + 'a,
    M: AsRef<str> + 'a,
{
    overview.into_iter().find_map(|(provider, id)| {
        (id.as_ref() == model).then(|| provider.as_ref().to_string())
    })
}

pub(crate) async fn run_start(
    adapter: &GuiHostAdapter,
    envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::RunStart {
        session_id,
        user_message,
        model,
        provider,
        profile: _,
    } = command
    else {
        unreachable!("run_start handler receives RunStart")
    };
    let history = {
        let core = adapter.core.read().await;
        core.get_session(session_id)
            .await
            .map_err(GuiHostAdapter::app_error)?;
        if core.provider_pending() {
            return Ok(AppResponse::Error(ErrorContext {
                category: ErrorCategory::Authentication,
                message: format!(
                    "provider {} 未装配凭证：先 pawork auth set-key {} 或 pawork auth login {}",
                    core.provider_id().as_str(),
                    core.provider_id().as_str(),
                    core.provider_id().as_str()
                ),
                retryable: false,
                retry_after_ms: None,
                diagnostics: Default::default(),
            }));
        }
        core.resume_messages_keep_pending(session_id)
            .await
            .map_err(GuiHostAdapter::app_error)?
    };
    let current = {
        let core = adapter.core.read().await;
        (
            core.provider_id().as_str().to_string(),
            core.model().as_str().to_string(),
        )
    };
    if let Some((requested_provider, requested_model)) = run_start_requested_provider_switch(
        &current.0,
        &current.1,
        provider.as_ref().map(|id| id.as_str()),
        model.as_ref().map(|id| id.as_str()),
    ) {
        let mut core = adapter.core.write().await;
        core.switch_provider(
            Some(session_id),
            &requested_provider,
            requested_model.as_deref(),
        )
        .await
        .map_err(GuiHostAdapter::app_error)?;
        let confirmed = (
            core.provider_id().as_str().to_string(),
            core.model().as_str().to_string(),
        );
        drop(core);
        if confirmed != current {
            adapter.bus.publish_diagnostic(
                adapter.instance.clone(),
                session_id,
                "model.switched",
                json!({
                    "from": {
                        "provider": current.0,
                        "model": current.1
                    },
                    "to": {
                        "provider": confirmed.0,
                        "model": confirmed.1,
                    }
                }),
            );
        }
    } else if provider.is_none() {
        if let Some(model) = model {
            if model.as_str() != current.1 {
                let mut core = adapter.core.write().await;
                let switched = match core
                    .switch_model(Some(session_id), model.as_str())
                    .await
                {
                    Ok(()) => Ok(()),
                    Err(crate::AppError::ModelBelongsToProvider { owner, .. }) => {
                        core.switch_provider(
                            Some(session_id),
                            &owner,
                            Some(model.as_str()),
                        )
                        .await
                    }
                    Err(error @ crate::AppError::UnknownModel { .. }) => {
                        let owner = core
                            .models_overview()
                            .await
                            .into_iter()
                            .find(|entry| entry.id.as_str() == model.as_str())
                            .map(|entry| entry.provider.as_str().to_string());
                        match owner {
                            Some(owner) if owner != current.0 => {
                                match core
                                    .switch_provider(
                                        Some(session_id),
                                        &owner,
                                        Some(model.as_str()),
                                    )
                                    .await
                                {
                                    Ok(()) => Ok(()),
                                    Err(crate::AppError::UnknownModel { .. }) => {
                                        match core
                                            .switch_provider(Some(session_id), &owner, None)
                                            .await
                                        {
                                            Ok(()) => {
                                                core.switch_model(
                                                    Some(session_id),
                                                    model.as_str(),
                                                )
                                                .await
                                            }
                                            Err(other) => Err(other),
                                        }
                                    }
                                    Err(other) => Err(other),
                                }
                            }
                            _ => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                };
                switched.map_err(GuiHostAdapter::app_error)?;
                let confirmed = (
                    core.provider_id().as_str().to_string(),
                    core.model().as_str().to_string(),
                );
                drop(core);
                adapter.bus.publish_diagnostic(
                    adapter.instance.clone(),
                    session_id,
                    "model.switched",
                    json!({
                        "from": {
                            "provider": current.0,
                            "model": current.1
                        },
                        "to": {
                            "provider": confirmed.0,
                            "model": confirmed.1,
                        }
                    }),
                );
            }
        }
    }
    // 与 CLI run_one_turn 同一语义：user text 原样为首 part，`@token` 命中的
    // file-index 附件作为独立 Text part 追加；无 `@` 或未命中时零行为变化。
    // 解析失败按 fail-closed 上抛，禁止把未展开文本静默发给模型。
    // 必须在登记 ActiveGuiRun 之前完成：失败路径不能留下幽灵 run。
    let content = {
        let core = adapter.core.read().await;
        core.expand_at_refs(user_message)
            .map_err(GuiHostAdapter::app_error)?
    };
    let n = adapter.next_gui_run.fetch_add(1, Ordering::Relaxed);
    let run_id = RunId::from(format!(
        "run-gui-{}-{n}",
        now_timestamp().as_unix_millis()
    ));
    let token = CancellationToken::new();
    adapter.runs.register(
        ActiveGuiRun {
            run_id: run_id.clone(),
            session_id: session_id.clone(),
            started_at_ms: now_timestamp().as_unix_millis(),
        },
        token.clone(),
    );
    let core = Arc::clone(&adapter.core);
    let bus = Arc::clone(&adapter.bus);
    let runs = Arc::clone(&adapter.runs);
    let approvals = Arc::clone(&adapter.approvals);
    let instance = adapter.instance.clone();
    let session = session_id.clone();
    let run = run_id.clone();
    let mut messages = history;
    messages.push(Message {
        id: MessageId::from("pending"),
        role: MessageRole::User,
        content,
        metadata: Default::default(),
    });
    tokio::spawn(async move {
        let sink = GuiBroadcastSink::new(Arc::clone(&bus), instance.clone());
        let outcome = {
            let core = core.read().await;
            core.chat_turn_with_run_id(
                run.clone(),
                &session,
                messages,
                &sink,
                token,
            )
            .await
        };
        if let Err(error) = outcome {
            bus.publish_raw(
                instance.clone(),
                &session,
                AppEvent::RunChanged {
                    run_id: run.clone(),
                    state: RunState::Failed,
                },
            );
            bus.publish_diagnostic(
                instance,
                &session,
                "run.failed",
                json!({ "message": error.to_string() }),
            );
        }
        approvals.clear_run(&run);
        runs.remove(&run);
    });
    Ok(AppResponse::Accepted {
        command_id: envelope.command_id.clone(),
        run_id: Some(run_id),
    })
}
