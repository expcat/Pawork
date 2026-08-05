//! 计时域：模型生成（`pawork` ↔ Mock Provider 的脚本化 token 流）。
//!
//! 口径：Core 消费/解析/分发 token 流的开销；用 Mock Provider 的
//! 脚本化 token 流（testing.md 的 `MockScript`）模拟生成速率，
//! 不含真实模型推理时间——无真实模型时模型生成不可直接测量。
//! 真实负载接入后以 `PAWORK_BENCH_MODEL=1` 放行。
//!
//! P0-12：空基准占位（`model/placeholder`），不连接任何 Provider。

use criterion::{criterion_group, criterion_main, Criterion};

fn model_placeholder(c: &mut Criterion) {
    let mut group = c.benchmark_group("model");
    group.bench_function("placeholder", |b| {
        b.iter(|| std::hint::black_box(1u64.wrapping_add(1)))
    });
    group.finish();
}

criterion_group!(model, model_placeholder);
criterion_main!(model);
