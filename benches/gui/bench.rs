//! 计时域：GUI 渲染（独立 GPUI Desktop 进程）。
//!
//! 口径：渲染、帧率与交互延迟。GUI 是经 GUI Connection Protocol 连接
//! CLI 的独立进程，不嵌入 Core，因此本组不经 criterion 测量；
//! 未来使用专用 harness（帧计时/埋点），当前阶段仅保留占位组
//! （见 docs/quality/benchmark-methodology.md「六类计时域」）。
//! 性能目标仅约束 Rust Core，不含 GPUI/窗口/GPU 与模型网络时间。
//!
//! P0-12：空基准占位（`gui/placeholder`），不启动任何 GUI 进程。

use criterion::{criterion_group, criterion_main, Criterion};

fn gui_placeholder(c: &mut Criterion) {
    let mut group = c.benchmark_group("gui");
    group.bench_function("placeholder", |b| {
        b.iter(|| std::hint::black_box(1u64.wrapping_add(1)))
    });
    group.finish();
}

criterion_group!(gui, gui_placeholder);
criterion_main!(gui);
