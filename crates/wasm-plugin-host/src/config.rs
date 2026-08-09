//! 资源与安全策略的静态配置。
//!
//! [`HostConfig`] 集中描述了 P10-2/P10-5 的全部 host 侧限额：
//! - Wasmtime fuel（确定性计算预算）
//! - `StoreLimits` 内存/实例/表上限
//! - epoch 驱动的 wall-clock 超时与协作取消粒度
//! - 组件字节、invoke 输入/输出字节的硬上限
//! - 插件状态存储的大小与配额（见 [`crate::state`]）
//!
//! 默认值遵循 ADR-012「WASM-first 插件」与 P10-5「默认无文件/网络/进程」：
//! 不在配置中开放任何 capability 直通——所有越权访问由 Linker 不注入对应
//! import 实现（见 [`crate::host`]）。

use std::time::Duration;

use thiserror::Error;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum HostConfigError {
    #[error("host config field must be greater than zero: {0}")]
    Zero(&'static str),
}

/// WASM 插件宿主的资源与安全配置。
#[derive(Clone, Debug)]
pub struct HostConfig {
    /// 单次 invoke 的 fuel 上限；耗尽返回 [`PluginErrorKind::FuelExhausted`]。
    ///
    /// [`PluginErrorKind::FuelExhausted`]: plugin_api::PluginErrorKind::FuelExhausted
    pub fuel: u64,
    /// 单个 Store 内所有线性内存的字节上限（`StoreLimits::memory_size`）。
    pub max_memory_bytes: usize,
    /// 单个 Store 内 core 实例数上限。
    pub max_instances: usize,
    /// 单个 Store 内表数上限。
    pub max_tables: usize,
    /// 单个 Store 内线性内存数上限。
    pub max_memories: usize,
    /// 单个表的元素数上限。
    pub max_table_elements: usize,
    /// 允许加载的组件字节上限（签名验证与编译前检查）。
    pub max_component_bytes: usize,
    /// 单次 invoke 输入 JSON 的字节上限。
    pub max_input_bytes: usize,
    /// 单次 invoke 输出字符串的字节上限（解析前检查）。
    pub max_output_bytes: usize,
    /// 单次 invoke 的 wall-clock 超时（epoch 驱动的协作中断）。
    pub invoke_timeout: Duration,
    /// epoch ticker 周期：每隔多久 `Engine::increment_epoch` 一次。
    /// 越小取消/超时响应越快，CPU 开销略高。
    pub epoch_tick: Duration,
    /// 单个状态值的字节上限（序列化后）。
    pub state_max_value_bytes: usize,
    /// 单个 (plugin, scope) 内最大键数。
    pub state_max_keys_per_scope: usize,
    /// 单个 (plugin, scope) 的总字节数上限。
    pub state_max_bytes_per_scope: usize,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            // 默认计算预算：足够完成常规工具/命令调用，但不足以穷举搜索。
            fuel: 10_000_000,
            // 8 MiB 线性内存：覆盖典型插件，远低于宿主进程预算。
            max_memory_bytes: 8 * 1024 * 1024,
            // 每个插件 Store 只允许其自身组件实例化（崩溃隔离边界）。
            max_instances: 1,
            max_tables: 1,
            max_memories: 1,
            max_table_elements: 64,
            max_component_bytes: 8 * 1024 * 1024,
            max_input_bytes: 256 * 1024,
            max_output_bytes: 256 * 1024,
            invoke_timeout: Duration::from_secs(5),
            epoch_tick: Duration::from_millis(10),
            state_max_value_bytes: 16 * 1024,
            state_max_keys_per_scope: 256,
            state_max_bytes_per_scope: 256 * 1024,
        }
    }
}

impl HostConfig {
    pub fn validate(&self) -> Result<(), HostConfigError> {
        let numeric = [
            ("fuel", self.fuel),
            ("max_memory_bytes", self.max_memory_bytes as u64),
            ("max_instances", self.max_instances as u64),
            ("max_tables", self.max_tables as u64),
            ("max_memories", self.max_memories as u64),
            ("max_table_elements", self.max_table_elements as u64),
            ("max_component_bytes", self.max_component_bytes as u64),
            ("max_input_bytes", self.max_input_bytes as u64),
            ("max_output_bytes", self.max_output_bytes as u64),
        ];
        if let Some((field, _)) = numeric.into_iter().find(|(_, value)| *value == 0) {
            return Err(HostConfigError::Zero(field));
        }
        if self.invoke_timeout.is_zero() {
            return Err(HostConfigError::Zero("invoke_timeout"));
        }
        if self.epoch_tick.is_zero() {
            return Err(HostConfigError::Zero("epoch_tick"));
        }
        Ok(())
    }

    /// 限制更宽松的测试预设：高 fuel、长超时、放宽内存，用于聚焦非限额路径的测试。
    pub fn permissive() -> Self {
        Self {
            fuel: u64::MAX / 4,
            invoke_timeout: Duration::from_secs(30),
            max_memory_bytes: 32 * 1024 * 1024,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_zero_epoch_is_rejected() {
        HostConfig::default().validate().expect("valid defaults");

        let invalid = HostConfig {
            epoch_tick: Duration::ZERO,
            ..HostConfig::default()
        };
        assert_eq!(invalid.validate(), Err(HostConfigError::Zero("epoch_tick")));
    }
}
