//! 从 V1 `app-database` 收回的控制面 / lease 投影 schema。
//!
//! 始终可用（不依赖 `account-control-v1`）。SQL、版本号与账本表名保持 V1：
//! - [`control_plane`]：`control_plane_schema_migrations`，CURRENT = 2，
//!   表 `provider_accounts` / `credentials`（无 secret 列）；
//! - [`lease`]：`credential_leases_schema_migrations`，DDL 轴 CURRENT = 3，
//!   表 `credential_leases` / `credential_lease_events`。
//!
//! 不迁 `identity` schema（属 A1 / `pawork-control-plane`）。
//! 不创建 `session_bindings` 表（已在 `pawork-session` v9）。
//! lease 记录版本轴（[`crate::LEASE_SCHEMA_VERSION`] = 2）与 DDL 轴
//! （[`lease::CURRENT_LEASE_SCHEMA_VERSION`] = 3）仍是两套数字，不要合并。

pub mod control_plane;
pub mod lease;
