//! 计时域：Git 子进程（`pawork` 派生的 `git` 子进程）。
//!
//! 口径：从 spawn 到进程退出的墙钟时间，含 fork/exec 与 git 自身执行；
//! 不含走网络的操作段（`git fetch`/`clone` 的远端交互需单列或排除）。
//! 未来对应性能目标：中型仓库 Git status（< 300 ms）。
//! 真实负载接入后以 `PAWORK_BENCH_GIT=1` 放行（见
//! docs/quality/benchmark-methodology.md「门禁开关约定」）。
//!
//! P0-12：空基准占位（`git/placeholder`），不派生任何子进程。

use criterion::{criterion_group, criterion_main, Criterion};

fn git_placeholder(c: &mut Criterion) {
    let mut group = c.benchmark_group("git");
    group.bench_function("placeholder", |b| {
        b.iter(|| std::hint::black_box(1u64.wrapping_add(1)))
    });
    group.finish();
}

criterion_group!(git, git_placeholder);
criterion_main!(git);
