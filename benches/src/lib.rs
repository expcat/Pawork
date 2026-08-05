//! Pawork 性能基准共享库（P0-12 骨架）。
//!
//! 本 crate 当前只包含六类计时域的空基准占位与共享辅助约定；
//! 计时口径（Rust Core / Git 子进程 / Provider 网络 / 模型生成 /
//! 外部命令 / GUI 渲染）的完整定义见
//! `docs/quality/benchmark-methodology.md`。

/// 六类计时域分组名，与 `benches/<group>/bench.rs` 一一对应，
/// 也与 ADR-020 要求的耗时来源划分一一对应。
pub const GROUPS: [&str; 6] = ["core", "git", "provider", "model", "command", "gui"];

/// 判断某计时域的真实负载基准是否被环境变量放行。
///
/// 约定：`PAWORK_BENCH_<组名大写>=1`（如 `PAWORK_BENCH_GIT=1`、
/// `PAWORK_BENCH_PROVIDER=1`）。P0-12 阶段的空基准占位不需要门禁；
/// 该函数供后续接入真实负载（Git 仓库、Mock Provider、子进程等）时使用，
/// 保证默认 `cargo bench` 不触碰任何外部系统。
pub fn group_enabled(group: &str) -> bool {
    let var = format!("PAWORK_BENCH_{}", group.to_ascii_uppercase());
    std::env::var_os(&var).is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}
