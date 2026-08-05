//! 计时域：外部命令（`pawork` 派生的 Tool 子进程）。
//!
//! 口径：子进程 spawn/exec 与调度开销（spawn → 进程就绪/退出），
//! 含 fork/exec 与 PTY/管道建立；外部命令自身的业务耗时不计入
//! 平台性能目标。未来对应性能目标：Built-in Tool 调度开销（< 5 ms）。
//! 真实负载接入后以 `PAWORK_BENCH_COMMAND=1` 放行。
//!
//! P0-12：空基准占位（`command/placeholder`），不派生任何子进程。

use criterion::{criterion_group, criterion_main, Criterion};

fn command_placeholder(c: &mut Criterion) {
    let mut group = c.benchmark_group("command");
    group.bench_function("placeholder", |b| {
        b.iter(|| std::hint::black_box(1u64.wrapping_add(1)))
    });
    group.finish();
}

criterion_group!(command, command_placeholder);
criterion_main!(command);
