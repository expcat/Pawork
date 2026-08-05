# benches — Pawork 性能基准（P0-12 骨架）

独立基准 crate `pawork-benches`，包含六类计时域的空基准占位：

| 基准组 | 计时域 | 入口 |
| --- | --- | --- |
| `core` | Rust Core（进程内） | [core/bench.rs](core/bench.rs) |
| `git` | Git 子进程 | [git/bench.rs](git/bench.rs) |
| `provider` | Provider 网络 | [provider/bench.rs](provider/bench.rs) |
| `model` | 模型生成 | [model/bench.rs](model/bench.rs) |
| `command` | 外部命令 | [command/bench.rs](command/bench.rs) |
| `gui` | GUI 渲染（占位，不经 criterion 测量） | [gui/bench.rs](gui/bench.rs) |

从仓库根运行：

```powershell
cargo bench -p pawork-benches                    # 全部六组
cargo bench -p pawork-benches --bench core       # 单组
cargo bench -p pawork-benches -- core            # criterion 过滤器
```

计时口径与冷/热测量约定见
[docs/quality/benchmark-methodology.md](../docs/quality/benchmark-methodology.md)。
