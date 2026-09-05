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
    // 与 chat turn 相同口径：全程持读锁；只阻塞 provider/默认模型切换等
    // 写者，读侧并发不受影响。
    let core = core.read().await;
    match core.get_session(&session_id).await {
        Ok(record) if record.title == PLACEHOLDER_SESSION_TITLE => {}
        _ => return,
    }
    if core.config().naming_provider.is_none() || core.config().naming_model.is_none() {
        return;
    }
    let Some(first_user_text) = first_user_text(&core, &session_id).await else {
        return;
    };
    let title = match core.generate_session_title(&first_user_text).await {
        Ok(Some(title)) => title,
        Ok(None) => return,
        Err(error) => {
            tracing::debug!(error = %error, "session auto naming skipped");
            return;
        }
    };
    // 写回前复核：命名补全期间标题可能已被用户改动，此时放弃写回。
    if let Ok(record) = core.get_session(&session_id).await {
        if record.title != PLACEHOLDER_SESSION_TITLE {
            return;
        }
    }
    if let Err(error) = core
        .rename_session(
            &session_id,
            &title,
            now_timestamp().as_unix_millis() as i64,
        )
        .await
    {
        tracing::debug!(error = %error, "session auto naming rename failed");
        return;
    }
    let Ok(record) = core.get_session(&session_id).await else {
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
    let messages = core.resume_messages(session_id).await.ok()?;
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
