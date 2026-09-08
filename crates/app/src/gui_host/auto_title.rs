//! ADR-054 D4/D5：Run 成功终态后的自动会话标题编排。
//!
//! 只有「标题仍是占位名 + naming_provider/naming_model 已配置」才调用命名
//! 模型；未配置 / 失败 / 超时一律保留占位名，不启发式、不向用户报错。
//! 写回前复核标题仍为占位名（防命名期间用户已改名），成功后经 EventHub
//! 广播 SessionMetaChanged（写后状态）。

use std::sync::Arc;

use pawork_domain::{MessageRole, SessionId};
use pawork_engine::now_timestamp;
use pawork_protocol::AppEvent;

use super::bus::GuiEventBus;
use crate::app_core::PLACEHOLDER_SESSION_TITLE;
use crate::AppCore;

pub(crate) async fn auto_title_after_successful_run(
    core: Arc<tokio::sync::RwLock<AppCore>>,
    bus: Arc<GuiEventBus>,
    instance: pawork_domain::CoreInstanceId,
    session_id: SessionId,
) {
    // 仅快照配置、读取消息与写回时持锁，网络请求不阻塞用户操作。
    let locked = core.read().await;
    match locked.get_session(&session_id).await {
        Ok(record) if record.title == PLACEHOLDER_SESSION_TITLE => {}
        other => {
            tracing::debug!(title = ?other.map(|record| record.title), "session auto naming skipped: title not placeholder");
            return;
        }
    }
    let config = locked.config();
    match (
        config.naming_provider.as_deref(),
        config.naming_model.as_deref(),
    ) {
        (Some(provider), Some(model)) if !config.is_model_enabled(provider, model) => {
            // ADR-055 D4：命名模型被禁用时跳过命名，保留占位名。
            tracing::debug!(
                naming_provider = provider,
                naming_model = model,
                "session auto naming skipped: naming model disabled"
            );
            return;
        }
        (Some(_), Some(_)) => {}
        _ => {
            tracing::debug!(
                naming_provider = ?config.naming_provider,
                "session auto naming skipped: naming model not configured"
            );
            return;
        }
    }
    let Some(first_user_text) = first_user_text(&locked, &session_id).await else {
        tracing::debug!("session auto naming skipped: no first user text");
        return;
    };
    let naming_pair = (config.naming_provider.clone(), config.naming_model.clone());
    let generate = locked.generate_session_title(&session_id, &first_user_text);
    drop(locked);
    let title = match generate.await {
        Ok(Some(title)) => title,
        Ok(None) => return,
        Err(error) => {
            tracing::debug!(error = %error, "session auto naming skipped");
            return;
        }
    };
    let locked = core.read().await;
    let config = locked.config();
    if (config.naming_provider.clone(), config.naming_model.clone()) != naming_pair
        || !config.is_model_enabled(
            naming_pair.0.as_deref().expect("configured provider"),
            naming_pair.1.as_deref().expect("configured model"),
        )
    {
        return;
    }
    let Ok(store) = locked.store() else {
        return;
    };
    // 条件判断与 UPDATE 同一条 SQL：用户改名与多个命名任务只允许匹配者写回。
    match store
        .rename_session_if_title(
            &session_id,
            PLACEHOLDER_SESSION_TITLE,
            &title,
            now_timestamp().as_unix_millis() as i64,
        )
        .await
    {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            tracing::debug!(error = %error, "session auto naming rename failed");
            return;
        }
    }
    let Ok(record) = locked.get_session(&session_id).await else {
        return;
    };
    bus.publish_raw(
        instance,
        &session_id,
        AppEvent::SessionMetaChanged {
            session_id: session_id.clone(),
            title: record.title,
            archived: record.archived,
        },
    );
}

/// 会话首条用户消息的正文 Text part（@附件等独立 part 不参与命名输入）。
async fn first_user_text(core: &AppCore, session_id: &SessionId) -> Option<String> {
    let messages = core.resume_messages_keep_pending(session_id).await.ok()?;
    messages
        .iter()
        .find(|message| message.role == MessageRole::User)
        .and_then(|message| {
            message.content.iter().find_map(|part| match part {
                pawork_domain::ContentPart::Text(text) => Some(text.text.clone()),
                _ => None,
            })
        })
}
