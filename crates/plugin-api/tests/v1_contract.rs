use std::collections::BTreeMap;

use agent_domain::{PluginId, RunId, ToolCallId, WorkspaceId};
use plugin_api::{
    plugin_api_version, PluginInvocation, PluginInvocationOutput, PluginInvocationResponse,
    PluginOperation, PluginStateMutation, PluginStateSnapshot,
};
use tool_api::{ToolExecutionContext, ToolRequest};

const WIT_SOURCE: &str = include_str!("../../../schemas/plugin-api/pawork-plugin-v1.wit");
const FROZEN_V1_WIT: &str = r#"package pawork:plugin@1.0.0;

/// Pawork Plugin API v1. The payloads are versioned JSON values described by
/// `plugin-api::{PluginInvocation, PluginInvocationOutput}`.
world plugin {
    export invoke: func(request: string) -> string;
}
"#;

mod guest_v1 {
    wit_bindgen::generate!({
        path: "../../schemas/plugin-api",
        world: "plugin",
    });

    pub struct FixtureGuest;

    impl Guest for FixtureGuest {
        fn invoke(request: String) -> String {
            request
        }
    }

    export!(FixtureGuest);
}

#[test]
fn wit_v1_is_frozen_and_generates_guest_bindings() {
    assert_eq!(WIT_SOURCE, FROZEN_V1_WIT);
    assert_eq!(
        <guest_v1::FixtureGuest as guest_v1::Guest>::invoke("probe".into()),
        "probe"
    );
}

#[test]
fn invocation_v1_json_payloads_match_frozen_golden() {
    let invocation = PluginInvocation {
        api_version: plugin_api_version(),
        plugin_id: PluginId::from("fixture.plugin"),
        operation: PluginOperation::Tool {
            name: "echo".into(),
            request: ToolRequest {
                tool_call_id: ToolCallId::from("call-1"),
                input: serde_json::json!({"text": "hello"}),
            },
            context: ToolExecutionContext {
                workspace_id: WorkspaceId::from("workspace-1"),
                run_id: RunId::from("run-1"),
                working_directory: None,
            },
        },
        state: PluginStateSnapshot {
            revision: 7,
            values: BTreeMap::from([("counter".into(), serde_json::json!(2))]),
        },
    };
    let encoded = serde_json::to_string(&invocation).expect("serialize v1 invocation");
    assert_eq!(
        encoded,
        r#"{"api_version":"1.0.0","plugin_id":"fixture.plugin","operation":{"type":"tool","name":"echo","request":{"tool_call_id":"call-1","input":{"text":"hello"}},"context":{"workspace_id":"workspace-1","run_id":"run-1"}},"state":{"revision":7,"values":{"counter":2}}}"#
    );
    let decoded: PluginInvocation =
        serde_json::from_str(&encoded).expect("deserialize frozen v1 invocation");
    assert_eq!(decoded, invocation);

    let output = PluginInvocationOutput::Success(PluginInvocationResponse {
        result: serde_json::json!({"ok": true}),
        state_mutations: vec![PluginStateMutation::Set {
            key: "counter".into(),
            value: serde_json::json!(3),
        }],
    });
    assert_eq!(
        serde_json::to_string(&output).expect("serialize v1 output"),
        r#"{"status":"success","data":{"result":{"ok":true},"state_mutations":[{"type":"set","key":"counter","value":3}]}}"#
    );
}
