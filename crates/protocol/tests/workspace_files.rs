use pawork_protocol::app::registry::{command_entry, query_entry};
use pawork_protocol::{ApiVersion, AppCommand, AppQuery};
#[test]
fn workspace_file_wire_roundtrip_and_gui_version_gate() {
    for source in [
        include_str!("golden/workspace_files.json"),
        include_str!("golden/workspace_file_read.json"),
    ] {
        let value: serde_json::Value = serde_json::from_str(source).unwrap();
        let query: AppQuery = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&query).unwrap(), value);
        let entry = query_entry(&query);
        assert_eq!(entry.since, ApiVersion::new(1, 19));
        assert!(entry.gui.available && entry.headless.is_none() && !entry.acp);
    }
    let value: serde_json::Value =
        serde_json::from_str(include_str!("golden/workspace_file_write.json")).unwrap();
    let command: AppCommand = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&command).unwrap(), value);
    let entry = command_entry(&command);
    assert_eq!(entry.since, ApiVersion::new(1, 19));
    assert!(entry.gui.available && entry.headless.is_none() && !entry.acp);
}
