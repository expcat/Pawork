//! P18-15：跨 crate 控制面 schema 版本一致性。
//!
//! `core-api` / `provider-control` / `app-database` 必须锁定同一常量（当前为 2）。
//! 这是独立于 `session-store::CURRENT_SCHEMA_VERSION`（9）的 schema 族，不得混比。

#[test]
fn control_plane_schema_version_is_aligned_across_crates() {
    assert_eq!(core_api::CONTROL_PLANE_SCHEMA_VERSION, 2);
    assert_eq!(
        core_api::CONTROL_PLANE_SCHEMA_VERSION,
        provider_control::CONTROL_PLANE_SCHEMA_VERSION
    );
    assert_eq!(
        core_api::CONTROL_PLANE_SCHEMA_VERSION,
        app_database::CURRENT_CONTROL_PLANE_SCHEMA_VERSION
    );
    // session-store 的 CURRENT_SCHEMA_VERSION = 9，属于另一 schema 族，
    // 此处刻意不引入该 crate、不与控制面版本做等式断言。
}
