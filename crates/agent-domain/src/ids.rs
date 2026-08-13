use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($($name:ident),+ $(,)?) => {
        $(
            #[doc = concat!("类型安全的 `", stringify!($name), "`。")]
            #[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
            #[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
            pub struct $name(String);

            impl $name {
                pub fn new(value: impl Into<String>) -> Self {
                    Self(value.into())
                }

                pub fn as_str(&self) -> &str {
                    &self.0
                }

                pub fn into_inner(self) -> String {
                    self.0
                }
            }

            impl From<String> for $name {
                fn from(value: String) -> Self {
                    Self::new(value)
                }
            }

            impl From<&str> for $name {
                fn from(value: &str) -> Self {
                    Self::new(value)
                }
            }

            impl AsRef<str> for $name {
                fn as_ref(&self) -> &str {
                    self.as_str()
                }
            }

            impl fmt::Display for $name {
                fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                    formatter.write_str(self.as_str())
                }
            }
        )+
    };
}

string_id!(
    ActorId,
    AgentId,
    ArtifactId,
    AccountId,
    CheckpointId,
    CredentialId,
    CommandId,
    ConnectionId,
    CoreInstanceId,
    EventId,
    GuiClientId,
    MessageId,
    ModelId,
    PluginId,
    PrincipalId,
    ProtectedBlobRef,
    ProviderId,
    QueryId,
    ReasoningItemId,
    RequestId,
    RunId,
    SessionId,
    TenantId,
    TerminalSessionId,
    ToolCallId,
    ToolExecutionId,
    WorkspaceId,
);
// Phase 16 Modern Agent Workflow 领域 ID（P16-1～P16-8）。
string_id!(
    PlanId,
    PlanStepId,
    PlanVersionId,
    GoalId,
    BackgroundTaskId,
    AutomationId,
    MonitorId,
    MemoryId,
    ReviewSessionId,
    ReviewFindingId,
);

/// Unix epoch 起的毫秒数。使用整数可保证跨语言无损序列化。
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
pub struct Timestamp(pub u64);

impl Timestamp {
    pub const fn from_unix_millis(value: u64) -> Self {
        Self(value)
    }

    pub const fn as_unix_millis(self) -> u64 {
        self.0
    }
}
