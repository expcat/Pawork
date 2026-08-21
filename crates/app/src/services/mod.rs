//! R4 波 A：AppCore 领域服务拆分（纯代码组织，行为零变化）。

pub(crate) mod usage;
pub(crate) mod tasks;
pub(crate) mod import;
pub(crate) mod extension;
pub(crate) mod session;
pub(crate) mod run;
pub(crate) mod approval;
