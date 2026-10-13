//! Mock transport：脚本化响应队列 + 发送记录，用于契约测试与下游集成。
//!
//! 与真实 [`StdioTransport`](crate::headless::transport::StdioTransport) 实现同一个
//! [`Transport`] 契约。`read_line` 在队列为空时**阻塞等待**（等价真实管道），
//! 测试可随时 `push_response` 推送事件/响应；关闭后返回
//! [`SdkError::Closed`]。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::Notify;

use crate::headless::error::SdkError;
use crate::headless::transport::Transport;

#[derive(Default)]
struct State {
    /// 按序返回的脚本化响应行（仅当已发送对应数量的请求行后才被服务，
    /// 模拟请求-响应协议；事件走 [`State::events`]）。
    responses: VecDeque<String>,
    /// 事件行：不依赖请求，随时可读（模拟 Host 主动推送）。
    events: VecDeque<String>,
    /// 客户端发出的所有行（按序）。
    sent: Vec<String>,
    /// 已服务的响应数。
    served: usize,
    /// 下一次 `read_line` 注入的错误（用于 IO/关闭路径测试）。
    fail_next_read: Option<SdkError>,
    closed: bool,
}

/// 进程无关的脚本化传输。
#[derive(Clone, Default)]
pub struct MockTransport {
    state: Arc<Mutex<State>>,
    notify: Arc<Notify>,
}

impl MockTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一条脚本化响应行（按 FIFO 返回；通知等待中的 reader）。
    pub fn push_response(self, line: impl Into<String>) -> Self {
        self.state.lock().unwrap().responses.push_back(line.into());
        self.notify.notify_one();
        self
    }

    /// 追加一条事件行（不依赖请求，立即投递）。
    pub fn push_event(self, line: impl Into<String>) -> Self {
        self.state.lock().unwrap().events.push_back(line.into());
        self.notify.notify_one();
        self
    }

    /// 追加多条响应行。
    pub fn push_responses<I, S>(self, lines: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        {
            let mut state = self.state.lock().unwrap();
            state.responses.extend(lines.into_iter().map(Into::into));
        }
        self.notify.notify_one();
        self
    }

    /// 注入下一次读取错误（IO 关闭路径测试）。
    pub fn fail_next_read(self, error: SdkError) -> Self {
        self.state.lock().unwrap().fail_next_read = Some(error);
        self.notify.notify_waiters();
        self
    }

    /// 已发送的行（测试断言请求内容用）。
    pub fn sent_lines(&self) -> Vec<String> {
        self.state.lock().unwrap().sent.clone()
    }

    /// 已发送行数。
    pub fn sent_count(&self) -> usize {
        self.state.lock().unwrap().sent.len()
    }

    /// 断言第 `index` 条发送行与预期 JSON 相等（结构化比较，忽略键序）。
    pub fn assert_sent_json(&self, index: usize, expected: &serde_json::Value) {
        let sent = &self.state.lock().unwrap().sent[index];
        let actual: serde_json::Value = serde_json::from_str(sent).expect("sent line is JSON");
        assert_eq!(&actual, expected, "sent line #{index} mismatch");
    }
}

#[async_trait]
impl Transport for MockTransport {
    async fn write_line(&self, line: &str) -> Result<(), SdkError> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err(SdkError::Closed("mock transport is closed".into()));
        }
        state.sent.push(line.to_string());
        // 请求已写：唤醒可能正等待响应门控的 reader。
        self.notify.notify_one();
        Ok(())
    }

    async fn read_line(&self) -> Result<String, SdkError> {
        loop {
            {
                let mut state = self.state.lock().unwrap();
                if let Some(error) = state.fail_next_read.take() {
                    return Err(error);
                }
                if state.closed {
                    return Err(SdkError::Closed("mock transport is closed".into()));
                }
                if let Some(line) = state.events.pop_front() {
                    return Ok(line);
                }
                // 请求-响应门控：第 N 条响应须等第 N 条请求已发出。
                // 响应里已有的 request_id 回显这条请求（command_id 或 request_id）。
                if !state.responses.is_empty() && state.sent.len() > state.served {
                    let sent = state.sent[state.served].clone();
                    state.served += 1;
                    let line = state.responses.pop_front().expect("served < len");
                    return Ok(echo_request_id(&line, &sent));
                }
            }
            self.notify.notified().await;
        }
    }

    async fn flush(&self) -> Result<(), SdkError> {
        Ok(())
    }

    async fn close(&self) -> Result<(), SdkError> {
        self.state.lock().unwrap().closed = true;
        self.notify.notify_one();
        Ok(())
    }

    fn is_open(&self) -> bool {
        !self.state.lock().unwrap().closed
    }
}

/// 脚本响应无法预写带命名空间的请求 id。只改写响应里已经存在的 request_id，
/// 缺字段的帧（hello_ack、无 id 的 error）保持原样。
fn echo_request_id(response: &str, sent: &str) -> String {
    let Ok(mut response_value) = serde_json::from_str::<serde_json::Value>(response) else {
        return response.to_string();
    };
    let Ok(sent_value) = serde_json::from_str::<serde_json::Value>(sent) else {
        return response.to_string();
    };
    let Some(id) = sent_request_id(&sent_value) else {
        return response.to_string();
    };
    let mut changed = false;
    if response_value.get("request_id").is_some() {
        response_value["request_id"] = serde_json::Value::String(id.clone());
        changed = true;
    }
    if let Some(envelope) = response_value.get_mut("envelope") {
        if envelope.get("request_id").is_some() {
            envelope["request_id"] = serde_json::Value::String(id);
            changed = true;
        }
    }
    if changed {
        response_value.to_string()
    } else {
        response.to_string()
    }
}

fn sent_request_id(sent: &serde_json::Value) -> Option<String> {
    if let Some(id) = sent.get("request_id").and_then(serde_json::Value::as_str) {
        return Some(id.to_string());
    }
    let envelope = sent.get("envelope")?;
    envelope
        .get("command_id")
        .or_else(|| envelope.get("request_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}
