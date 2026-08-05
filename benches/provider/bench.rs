//! 计时域：Provider 网络（`pawork` ↔ Mock Provider）。
//!
//! 口径：请求发出到响应收完的传输与协议开销；用 Mock Provider
//! （wiremock）固定延迟，不含真实公网 API。性能目标中的
//! 「Provider 首 Token 的 Core 附加延迟 < 20 ms」只计 Core 段，
//! 在本组内以零网络延迟口径测量（见
//! docs/quality/benchmark-methodology.md「口径要点」）。
//! 真实负载接入后以 `PAWORK_BENCH_PROVIDER=1` 放行。
//!
//! P0-12：空基准占位（`provider/placeholder`），不发起任何网络请求。

use criterion::{criterion_group, criterion_main, Criterion};

fn provider_placeholder(c: &mut Criterion) {
    let mut group = c.benchmark_group("provider");
    group.bench_function("placeholder", |b| {
        b.iter(|| std::hint::black_box(1u64.wrapping_add(1)))
    });
    group.finish();
}

criterion_group!(provider, provider_placeholder);
criterion_main!(provider);
