use std::collections::BTreeMap;

use agent_domain::{PluginId, SessionId, WorkspaceId};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tool_api::{ToolExecutionContext, ToolRequest};

use crate::{PluginContext, PluginError, PluginLifecycleEvent};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginInvocation {
    pub api_version: Version,
    pub plugin_id: PluginId,
    pub operation: PluginOperation,
    #[serde(default)]
    pub state: PluginStateSnapshot,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginOperation {
    Tool {
        name: String,
        request: ToolRequest,
        context: ToolExecutionContext,
    },
    Command(PluginCommandInvocation),
    Lifecycle {
        event: PluginLifecycleEvent,
        context: PluginContext,
    },
}

impl PluginOperation {
    pub fn state_scope(&self) -> PluginStateScope {
        match self {
            Self::Tool { context, .. } => PluginStateScope::Workspace(context.workspace_id.clone()),
            Self::Command(invocation) => invocation.context.state_scope(),
            Self::Lifecycle { context, .. } => context.state_scope(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginCommandInvocation {
    pub name: String,
    #[serde(default)]
    pub input: Value,
    pub context: PluginContext,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", content = "data", rename_all = "snake_case")]
pub enum PluginInvocationOutput {
    Success(PluginInvocationResponse),
    Error { error: PluginError },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginInvocationResponse {
    #[serde(default)]
    pub result: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub state_mutations: Vec<PluginStateMutation>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum PluginStateScope {
    Global,
    Workspace(WorkspaceId),
    Session(SessionId),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginStateSnapshot {
    #[serde(default)]
    pub revision: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginStateMutation {
    Set { key: String, value: Value },
    Remove { key: String },
}

#[cfg(test)]
mod tests {
    use agent_domain::{RunId, ToolCallId, WorkspaceId};

    use super::*;
    use crate::plugin_api_version;

    #[test]
    fn invocation_round_trip_is_stable_json() {
        let invocation = PluginInvocation {
            api_version: plugin_api_version(),
            plugin_id: PluginId::from("example.plugin"),
            operation: PluginOperation::Tool {
                name: "echo".into(),
                request: ToolRequest {
                    tool_call_id: ToolCallId::from("call"),
                    input: serde_json::json!({"value": "hello"}),
                },
                context: ToolExecutionContext {
                    workspace_id: WorkspaceId::from("workspace"),
                    run_id: RunId::from("run"),
                    working_directory: None,
                },
            },
            state: PluginStateSnapshot::default(),
        };

        let bytes = serde_json::to_vec(&invocation).expect("serialize invocation");
        let decoded: PluginInvocation =
            serde_json::from_slice(&bytes).expect("deserialize invocation");
        assert_eq!(decoded, invocation);
    }

    #[test]
    fn tool_state_is_scoped_to_workspace() {
        let operation = PluginOperation::Tool {
            name: "echo".into(),
            request: ToolRequest {
                tool_call_id: ToolCallId::from("call"),
                input: Value::Null,
            },
            context: ToolExecutionContext {
                workspace_id: WorkspaceId::from("workspace"),
                run_id: RunId::from("run"),
                working_directory: None,
            },
        };

        assert_eq!(
            operation.state_scope(),
            PluginStateScope::Workspace(WorkspaceId::from("workspace"))
        );
    }
}
