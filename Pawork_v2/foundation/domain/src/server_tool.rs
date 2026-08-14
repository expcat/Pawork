//! Server Tool 事件与 Citation / Source（P15-5）。
//!
//! 为 `ProviderHosted` / `ProviderExtension` 两种非本地执行位点定义统一的
//! canonical 事件与引用类型。本模块是纯领域数据：不执行 IO，不携带 Provider
//! 名称，不触发任何本地执行。大型 screenshot / program output 只存
//! [`ArtifactId`] 引用（ADR-018），避免整段 payload 进入事件流。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ArtifactId, ToolCallId};

/// 引用来源类别。三家口径对不上时保持 `Unknown`，不做猜测。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CitationSourceKind {
    /// 直接 URL 引用（OpenAI `url_citation` annotation 等）。
    Url,
    /// 网络搜索结果引用。
    WebSearch,
    /// 文档 / 文件内引用（Anthropic citation 等）。
    Document,
    /// 文件系统引用。
    File,
    /// 无法归类的引用来源。
    #[default]
    Unknown,
}

/// 统一 Citation：覆盖 OpenAI（url / index）、Anthropic（text / document_index）、
/// xAI（url / title / snippet）字段。缺省字段为空而非猜值。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Citation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_index: Option<u64>,
    #[serde(default)]
    pub source_kind: CitationSourceKind,
}

impl Citation {
    /// 空引用（全部字段缺省）。
    pub const fn empty() -> Self {
        Self {
            index: None,
            url: None,
            title: None,
            snippet: None,
            text: None,
            document_index: None,
            source_kind: CitationSourceKind::Unknown,
        }
    }
}

/// 原始引用元数据（Source）：三家 source / citation 的原始字段统一口径。
/// `raw_metadata` 保留无法归一化的原始负载，持久化时由 Event Store 脱敏。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_metadata: Option<Value>,
}

/// program output 的流通道。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgramStream {
    Stdout,
    Stderr,
}

/// Server Tool 生命周期事件（P15-5）。
///
/// 与本地 `ToolCall*` 事件并列但语义分离：server tool 由 Provider 服务端执行，
/// 不走 scheduler 本地执行，也不会产生本地 `ToolResult`。所有变体都携带
/// `tool_call_id`，按 sequence 落入可持久化事件流，可被 Projection 重建。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ServerToolEvent {
    /// Provider 已开始执行 server tool。
    Started {
        tool_call_id: ToolCallId,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arguments: Option<Value>,
    },
    /// arguments JSON 增量（跨 chunk 拼接）。
    ArgumentsDelta {
        tool_call_id: ToolCallId,
        json_delta: String,
    },
    /// 执行进度（允许增量消息，缺省为空）。
    Progress {
        tool_call_id: ToolCallId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// 执行完成；大结果只以 Artifact 引用表达。
    Completed {
        tool_call_id: ToolCallId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        artifacts: Vec<ArtifactId>,
    },
    /// 执行失败（状态错误）。
    Failed {
        tool_call_id: ToolCallId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    /// 新增一条 Citation。
    CitationAdded {
        tool_call_id: ToolCallId,
        citation: Citation,
    },
    /// 新增一条原始 Source。
    SourceAdded {
        tool_call_id: ToolCallId,
        source: Source,
    },
    /// Provider 请求计算机操作（截图、点击、输入等）。
    ComputerActionRequested {
        tool_call_id: ToolCallId,
        action: Value,
    },
    /// 计算机截图完成；只存 Artifact 引用（ADR-018）。
    ComputerScreenshot {
        tool_call_id: ToolCallId,
        artifact: ArtifactId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
    },
    /// Provider 端程序开始执行。
    ProgramStarted {
        tool_call_id: ToolCallId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
    /// 程序输出增量；大输出只存 Artifact 引用（ADR-018），与 `delta` 互斥。
    ProgramOutput {
        tool_call_id: ToolCallId,
        stream: ProgramStream,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delta: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifact: Option<ArtifactId>,
    },
}

impl ServerToolEvent {
    /// 该事件归属的 server tool 调用。
    pub const fn tool_call_id(&self) -> &ToolCallId {
        match self {
            Self::Started { tool_call_id, .. }
            | Self::ArgumentsDelta { tool_call_id, .. }
            | Self::Progress { tool_call_id, .. }
            | Self::Completed { tool_call_id, .. }
            | Self::Failed { tool_call_id, .. }
            | Self::CitationAdded { tool_call_id, .. }
            | Self::SourceAdded { tool_call_id, .. }
            | Self::ComputerActionRequested { tool_call_id, .. }
            | Self::ComputerScreenshot { tool_call_id, .. }
            | Self::ProgramStarted { tool_call_id, .. }
            | Self::ProgramOutput { tool_call_id, .. } => tool_call_id,
        }
    }

    /// 事件类型名（与 serde 变体名一致，供持久化 event_type 使用）。
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Started { .. } => "server_tool_started",
            Self::ArgumentsDelta { .. } => "server_tool_arguments_delta",
            Self::Progress { .. } => "server_tool_progress",
            Self::Completed { .. } => "server_tool_completed",
            Self::Failed { .. } => "server_tool_failed",
            Self::CitationAdded { .. } => "citation_added",
            Self::SourceAdded { .. } => "source_added",
            Self::ComputerActionRequested { .. } => "computer_action_requested",
            Self::ComputerScreenshot { .. } => "computer_screenshot",
            Self::ProgramStarted { .. } => "program_started",
            Self::ProgramOutput { .. } => "program_output",
        }
    }
}

/// Provider transcript 的归一化输出条目（provider-neutral）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum TranscriptItem {
    /// 归一后的 server tool 事件。
    ServerTool(ServerToolEvent),
    /// 文本增量。
    Text(String),
}

/// Provider transcript 续传信封（P15-5）。
///
/// 仅供 `ContinuationMode::ProviderTranscript`（`ProviderHosted` /
/// `ProviderExtension`）使用；不携带 Provider 名称与 Secret，具体协议翻译封装
/// 在 provider adapter 内。Core 持久化脱敏后的 envelope，适配器凭
/// `cursor` / `continuation_reference` 按原协议续接。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderTranscriptEnvelope {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<TranscriptItem>,
    /// 原生输出游标（opaque）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// 续接引用（opaque continuation reference）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_reference: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tool_call_id() -> ToolCallId {
        ToolCallId::from("server-tool-1")
    }

    #[test]
    fn citation_defaults_are_empty_not_guessed() {
        let citation = Citation {
            url: Some("https://example.com/doc".into()),
            ..Citation::empty()
        };
        let value = serde_json::to_value(&citation).expect("serialize citation");
        assert_eq!(value["url"], "https://example.com/doc");
        assert!(value.get("title").is_none());
        assert!(value.get("text").is_none());
        assert!(value.get("document_index").is_none());
        assert_eq!(value["source_kind"], "unknown");
        let decoded: Citation = serde_json::from_value(value).expect("deserialize citation");
        assert_eq!(decoded, citation);
    }

    #[test]
    fn every_server_tool_event_round_trips_through_json() {
        let events = vec![
            ServerToolEvent::Started {
                tool_call_id: tool_call_id(),
                name: "web_search".into(),
                arguments: Some(json!({"query": "pawork"})),
            },
            ServerToolEvent::ArgumentsDelta {
                tool_call_id: tool_call_id(),
                json_delta: r#"{"query":"pawork"}"#.into(),
            },
            ServerToolEvent::Progress {
                tool_call_id: tool_call_id(),
                message: Some("searching".into()),
            },
            ServerToolEvent::CitationAdded {
                tool_call_id: tool_call_id(),
                citation: Citation {
                    url: Some("https://example.com".into()),
                    title: Some("Example".into()),
                    source_kind: CitationSourceKind::WebSearch,
                    ..Citation::empty()
                },
            },
            ServerToolEvent::SourceAdded {
                tool_call_id: tool_call_id(),
                source: Source {
                    url: Some("https://example.com".into()),
                    title: Some("Example".into()),
                    snippet: Some("snippet".into()),
                    ..Default::default()
                },
            },
            ServerToolEvent::ComputerActionRequested {
                tool_call_id: tool_call_id(),
                action: json!({"type": "click", "x": 10, "y": 20}),
            },
            ServerToolEvent::ComputerScreenshot {
                tool_call_id: tool_call_id(),
                artifact: ArtifactId::from("artifact-shot-1"),
                media_type: Some("image/png".into()),
            },
            ServerToolEvent::ProgramStarted {
                tool_call_id: tool_call_id(),
                command: Some("run_tests.sh".into()),
            },
            ServerToolEvent::ProgramOutput {
                tool_call_id: tool_call_id(),
                stream: ProgramStream::Stdout,
                delta: Some("ok".into()),
                artifact: None,
            },
            ServerToolEvent::ProgramOutput {
                tool_call_id: tool_call_id(),
                stream: ProgramStream::Stderr,
                delta: None,
                artifact: Some(ArtifactId::from("artifact-log-1")),
            },
            ServerToolEvent::Completed {
                tool_call_id: tool_call_id(),
                summary: Some("found 3 results".into()),
                artifacts: vec![ArtifactId::from("artifact-1")],
            },
            ServerToolEvent::Failed {
                tool_call_id: tool_call_id(),
                message: Some("search failed".into()),
                code: Some("ECONNREFUSED".into()),
            },
        ];

        for event in events {
            let value = serde_json::to_value(&event).expect("serialize server tool event");
            let decoded: ServerToolEvent =
                serde_json::from_value(value).expect("deserialize server tool event");
            assert_eq!(decoded, event);
            assert_eq!(decoded.tool_call_id(), &tool_call_id());
        }
    }

    #[test]
    fn large_output_and_screenshot_use_artifact_reference_only() {
        let screenshot = ServerToolEvent::ComputerScreenshot {
            tool_call_id: tool_call_id(),
            artifact: ArtifactId::from("artifact-shot-1"),
            media_type: None,
        };
        let value = serde_json::to_value(&screenshot).expect("serialize screenshot");
        assert_eq!(value["kind"], "computer_screenshot");
        assert_eq!(value["data"]["artifact"], "artifact-shot-1");
        assert!(value["data"].get("media_type").is_none());

        let output = ServerToolEvent::ProgramOutput {
            tool_call_id: tool_call_id(),
            stream: ProgramStream::Stdout,
            delta: None,
            artifact: Some(ArtifactId::from("artifact-log-1")),
        };
        let value = serde_json::to_value(&output).expect("serialize program output");
        assert_eq!(value["data"]["artifact"], "artifact-log-1");
        assert!(value["data"].get("delta").is_none());
    }

    #[test]
    fn transcript_envelope_is_provider_neutral() {
        let envelope = ProviderTranscriptEnvelope {
            items: vec![
                TranscriptItem::ServerTool(ServerToolEvent::Completed {
                    tool_call_id: tool_call_id(),
                    summary: Some("done".into()),
                    artifacts: Vec::new(),
                }),
                TranscriptItem::Text("final".into()),
            ],
            cursor: Some("cursor-1".into()),
            continuation_reference: Some("ref-1".into()),
        };
        let json = serde_json::to_string(&envelope).expect("serialize envelope");
        for forbidden in [
            "provider",
            "openai",
            "anthropic",
            "xai",
            "api_key",
            "secret",
        ] {
            assert!(
                !json.contains(forbidden),
                "transcript envelope must not carry `{forbidden}`"
            );
        }
        let decoded: ProviderTranscriptEnvelope =
            serde_json::from_str(&json).expect("deserialize envelope");
        assert_eq!(decoded, envelope);
    }
}
