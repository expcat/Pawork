use pawork_protocol::app::registry::{command_entry, query_entry};
use pawork_protocol::{ApiVersion, AppCommand, AppQuery, SubagentListData, SubagentSettingsData};

#[test]
fn subagent_wire_roundtrip_and_gui_version_gate() {
    for source in [
        include_str!("golden/subagent_settings.json"),
        include_str!("golden/subagent_list.json"),
    ] {
        let value: serde_json::Value = serde_json::from_str(source).unwrap();
        let query: AppQuery = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&query).unwrap(), value);
        let entry = query_entry(&query);
        assert_eq!(entry.since, ApiVersion::new(1, 20));
        assert!(entry.gui.available && entry.headless.is_none() && !entry.acp);
        assert!(entry.since > ApiVersion::new(1, 19));
    }
    for source in [
        include_str!("golden/set_subagent_settings.json"),
        include_str!("golden/subagent_cancel.json"),
    ] {
        let value: serde_json::Value = serde_json::from_str(source).unwrap();
        let command: AppCommand = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&command).unwrap(), value);
        let entry = command_entry(&command);
        assert_eq!(entry.since, ApiVersion::new(1, 20));
        assert!(entry.gui.available && entry.headless.is_none() && !entry.acp);
        assert!(entry.since > ApiVersion::new(1, 19));
    }
}

#[test]
fn subagent_data_shapes_roundtrip_and_keep_old_minors() {
    let settings_value: serde_json::Value =
        serde_json::from_str(include_str!("golden/set_subagent_settings.json")).unwrap();
    let settings: SubagentSettingsData =
        serde_json::from_value(settings_value["params"]["settings"].clone()).unwrap();
    assert_eq!(settings.enabled, true);
    assert_eq!(settings.max_concurrent, 4);
    assert_eq!(settings.models.len(), 1);
    assert_eq!(settings.models[0].permissions.len(), 7);

    let list_value: serde_json::Value =
        serde_json::from_str(include_str!("golden/subagent_list_data.json")).unwrap();
    let list: SubagentListData = serde_json::from_value(list_value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&list).unwrap(), list_value);
    assert_eq!(list.agents[0].agent_id, "agent-1");
    assert!(list.agents[0].result.is_none());

    // 空规则设置也必须输出 `models` 键：GUI 按协议类型消费，丢键即 undefined。
    let empty = serde_json::to_value(SubagentSettingsData::default()).unwrap();
    assert_eq!(
        empty,
        serde_json::json!({"enabled": true, "max_concurrent": 4, "models": []})
    );

    // Old 1.19 workspace-file entries stay on their original minor.
    let files: AppQuery =
        serde_json::from_str(include_str!("golden/workspace_files.json")).unwrap();
    assert_eq!(query_entry(&files).since, ApiVersion::new(1, 19));
    let write: AppCommand =
        serde_json::from_str(include_str!("golden/workspace_file_write.json")).unwrap();
    assert_eq!(command_entry(&write).since, ApiVersion::new(1, 19));
}
