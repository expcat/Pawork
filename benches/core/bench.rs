//! 计时域：Rust Core（`pawork` 单二进制进程内）。
//!
//! 口径：仅进程内逻辑的墙钟时间——状态机、事件分发、Session 存取、
//! Diff/Patch 解析、Token 预算等；不含任何子进程、网络、模型与 GUI 耗时。
//! 未来对应性能目标：Core 冷初始化 P50/P95、空闲 RSS、列出 10,000 Session、
//! 打开大型 Session 尾部、Agent Event 分发开销、已缓存 Diff 切换、
//! 100,000 行 Diff 解析、崩溃后 Session 恢复（冷测量类目标需专用
//! 多进程 harness，见 docs/quality/benchmark-methodology.md）。
//!
//! P0-12：空基准占位（`core/placeholder`），只测 black_box no-op，
//! 用于打通基准管道与 CI。

use criterion::{criterion_group, criterion_main, Criterion};

fn core_placeholder(c: &mut Criterion) {
    let mut group = c.benchmark_group("core");
    group.bench_function("placeholder", |b| {
        b.iter(|| std::hint::black_box(1u64.wrapping_add(1)))
    });
    group.finish();
}

criterion_group!(core, core_placeholder);
criterion_main!(core);
