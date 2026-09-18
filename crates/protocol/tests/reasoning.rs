//! ADR-063 reasoning effort wire shapes (GUI API 1.21)。

use pawork_protocol::app::registry::command_entry;
use pawork_protocol::{ApiVersion, AppCommand, SubagentListData, SubagentSettingsData};

#[test]
fn set_model_reasoning_wire_roundtrip_and_version_gate() {
    let value: serde_json::Value =
        serde_json::from_str(include_str!("golden/set_model_reasoning.json")).unwrap();
    let command: AppCommand = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&command).unwrap(), value);
    let entry = command_entry(&command);
    assert_eq!(entry.since, ApiVersion::new(1, 21));
    assert!(entry.gui.available && entry.headless.is_none() && !entry.acp);
    assert!(entry.since > ApiVersion::new(1, 20));
}
#[test]
fn run_start_effort_is_additive_and_optional() {
    // 带 effort 的新客户端形状。
    let with_effort: AppCommand = serde_json::from_value(serde_json::json!({
        "method": "run_start",
        "params": {
            "session_id": "session-1",
            "user_message": "hi",
            "effort": "x_high"
        }
    }))
    .unwrap();
    let value = serde_json::to_value(&with_effort).unwrap();
    assert_eq!(value["params"]["effort"], "x_high");
    // 旧形状（无 effort 键）继续解析，且序列化不补键。
    let without: AppCommand = serde_json::from_value(serde_json::json!({
        "method": "run_start",
        "params": { "session_id": "session-1", "user_message": "hi" }
    }))
    .unwrap();
    let value = serde_json::to_value(&without).unwrap();
    assert!(value["params"].get("effort").is_none());
}

#[test]
fn subagent_effort_fields_are_additive() {
    // 规则与列表条目的新字段缺省不序列化（1.20 golden 不回漂）。
    let rule_default = serde_json::to_value(SubagentSettingsData::default()).unwrap();
    assert_eq!(
        rule_default,
        serde_json::json!({"enabled": true, "max_concurrent": 4, "models": []})
    );
    let list: SubagentListData = serde_json::from_value(serde_json::json!({
        "agents": [{
            "agent_id": "agent-1",
            "session_id": "session-1",
            "parent_run_id": "run-1",
            "title": "Research",
            "provider_id": "glm-coding",
            "model_id": "glm-5.3-flash",
            "status": "running",
            "effort": "high"
        }]
    }))
    .unwrap();
    assert_eq!(list.agents[0].effort.as_deref(), Some("high"));
    let encoded = serde_json::to_value(&list).unwrap();
    assert_eq!(encoded["agents"][0]["effort"], "high");

    // 带强度字段的规则 roundtrip。
    let settings: SubagentSettingsData = serde_json::from_value(serde_json::json!({
        "enabled": true,
        "max_concurrent": 4,
        "models": [{
            "provider_id": "glm-coding",
            "model_id": "glm-5.3-flash",
            "allow_spawn": true,
            "allow_as_subagent": true,
            "permissions": ["read"],
            "default_effort": "low",
            "allowed_efforts": ["low", "medium"]
        }]
    }))
    .unwrap();
    assert_eq!(settings.models[0].default_effort.as_deref(), Some("low"));
    assert_eq!(settings.models[0].allowed_efforts.len(), 2);
    let encoded = serde_json::to_value(&settings).unwrap();
    assert_eq!(encoded["models"][0]["default_effort"], "low");
}
