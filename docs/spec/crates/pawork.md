# pawork（apps/pawork，二进制）

> `pawork` CLI 的唯一正式宿主二进制：composition root，安装全局脱敏 tracing 后把控制权交给 [pawork-cli](cli.md)。装配链 `pawork → cli → app`（依赖方向见 [../../architecture.md](../../architecture.md)）。

## 1. 职责与边界

- **职责**：进程入口。只做两件事——`install_logging()`（带脱敏的全局 tracing 装配）与 `pawork_cli::run().await`。
- **边界**：CLI 与 Core 同进程同二进制，不存在独立 daemon / rpc 入口；本包不得直接依赖 `pawork-app` / engine / providers（Cargo 依赖仅 `pawork-cli`）。纯 Rust，禁止引入 JS runtime。
- `redact.rs` 是 R1 波 A 自已删除的 `pawork-diagnostics` 迁入的全局脱敏日志基础设施（crate-private，无库表面）。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/main.rs` | ~30 | `#[tokio::main] main() -> ExitCode`：`install_logging()`（`EnvFilter` + `RedactingFmtLayer` → stderr）后调 `pawork_cli::run()` |
| `src/redact.rs` | ~295 | `Redactor`（字段名 / 字段值双通道脱敏）与 `RedactingFmtLayer`（全局 tracing fmt 层）；`REDACTED` 常量；内嵌单元测试 |

无 `tests/` 目录，无 `pub` API（二进制 crate）。

## 3. 命令行为与启动装配

- 进程入口 `main`（`#[tokio::main]`，多线程 runtime）：安装日志 → `pawork_cli::run()`，其 `ExitCode` 即进程退出码——成功 `SUCCESS`，任何 CLI 错误在 cli 层打印到 stderr 后返回 `FAILURE`（子命令全集与行为见 [cli.md](cli.md)）。
- 本包不定义任何自有命令行参数：clap 解析、六运行模式与运维子命令全部在 `pawork-cli`；这里只负责"进程外壳"。
- 日志级别由 `RUST_LOG` 控制（`EnvFilter::try_from_default_env`，缺省 `warn`）；输出固定走 **stderr**，stdout 留给协议 / JSON（`--json`、`headless --json-stdio`、`acp serve` 的 stdout 纪律在 cli 层承载）。
- `RedactingFmtLayer` 持 `Arc<Mutex<dyn Write + Send>>` 注入 writer（生产为 `io::stderr()`），不做全局可变状态；`install_logging` 在 `run()` 之前完成，保证 CLI 全生命周期的 tracing 输出都过脱敏层。

## 4. 核心行为与数据流

1. **启动顺序**：tokio 多线程 runtime → `install_logging()` 把 `registry().with(EnvFilter).with(RedactingFmtLayer)` 设为全局 subscriber → `pawork_cli::run()`。
2. **字段名通道**：`Redactor::redact_field(name, value)` 先按敏感键正则匹配字段名（`authorization` / `cookie` / `set-cookie` / `api_key` / `*_token` / `*_secret` / `*_password` / `oauth(_code)` 等，大小写不敏感），命中则整值替换为 `[REDACTED]`；未命中再走值通道。
3. **字段值通道**：`Redactor::redact(value)` 依次套用内置模式——`Authorization` / `Cookie` 头行、`Bearer <token>`（保留 `Bearer ` 前缀）、JWT（`eyJ…`.`…`.`…`）、`sk-` / `rk-` / `pk-` / `api-` 前缀密钥（不要求单词边界，可命中嵌在 `load_failed_sk-…` 中的泄漏）、URL query 中的 token / key / secret / password / oauth_code（保留 `?`/`&` 与键名，仅遮值）、普通与转义 JSON 的 `key=value` / `key:value` 形态（含自定义 `*Token` header）；`Redactor::new(custom_patterns)` 支持追加自定义正则。
4. **事件格式化**：`RedactingFmtLayer::on_event` 用 `FieldVisitor` 收集事件全部字段（str / u64 / i64 / bool / Debug），**逐字段**过 `redact_field` 后拼为单行 `<level> <target> k=v …` 写入 writer；写失败静默丢弃（与 tracing fmt 层惯例一致），锁中毒时 `into_inner` 恢复继续写。

## 5. 契约与不变量

- **Secret 不进终端与日志**：所有 tracing 字段（含 message）必须先经 `Redactor` 再输出；绕过 `RedactingFmtLayer` 直接装 fmt 层即违规（安全红线口径见 [../security.md](../security.md)）。
- stdout 永不承载日志：日志固定 stderr，stdout 属于协议输出。
- 替换串固定 `[REDACTED]`；Bearer 形态保留 `Bearer ` 前缀、URL query 形态保留分隔符与键名（可观测性与脱敏并存：如 `context_tokens=128` 这类非敏感度量不受影响）。
- 内置正则在 `Default` 构造中 `expect` 校验——内置模式必须永远合法。

## 6. 依赖关系

- **Cargo 依赖**：`pawork-cli`（唯一内部依赖）；`tracing` / `tracing-subscriber` / `regex` / `tokio`（macros + rt-multi-thread）。
- **被依赖**：无 Cargo 依赖方。运行时消费者：`pawork-client` headless spawn（`PAWORK_BIN` 或 PATH 上的 `pawork`，见 [client.md](client.md)）、desktop 的 `--probe*` 探测与 `pawork gui serve` GUI 宿主进程（见 [desktop.md](desktop.md)）。

## 7. 测试与验证资产

默认验证命令：`cargo test -p pawork --offline --lib --tests`

- `src/redact.rs` 内嵌测试 ×2：
  - `redacts_headers_tokens_cookies_oauth_jwt_and_custom_patterns`：14 组泄漏样例（头行 / Basic / Cookie / oauth_code / `sk-` 前缀与嵌入形态 / JWT / 自定义模式 / URL query 双键 / 转义 JSON 嵌套 / 自定义 Token header）断言 Secret 不外泄，且 `context_tokens=128` 等非敏感字段保持可观测。
  - `redacting_fmt_layer_masks_secrets_and_keeps_plain_fields`：以捕获 writer 装全局层发真实 tracing 事件，断言四类 Secret 字段被遮蔽、target 与普通字段（`component` / `retries`）保留。
- `main.rs` 无测试（纯装配）；端到端行为由 cli 层与冒烟验证覆盖（见 [../verification.md](../verification.md)）。

## 8. 注意事项与已知限制

- 不要新增第二个宿主二进制或独立 daemon 入口；新通道一律挂在 `pawork-cli` 子命令下。
- 脱敏是最后防线而非唯一防线：上游（auth / provider / storage）仍须避免把明文 Secret 放进 tracing 字段；正则集合覆盖已知形态，新增凭证形态时须同批补 `redact.rs` 模式与测试。
- `sk-`/`api-` 前缀模式要求后随至少 12 个字符，过短的合成串不会命中；敏感键匹配的是**完整字段名**（锚定 `^…$`），形如 `context_tokens` 的度量字段不会被误遮。
- `RedactingFmtLayer` 输出为简单单行格式（无时间戳 / span 上下文），面向人读与冒烟排查，非结构化日志管道。
- 状态与任务登记见 [../../../ROADMAP.md](../../../ROADMAP.md)；设计事实源见 [../../design.md](../../design.md)；Spec 汇总见 [../README.md](../README.md)。
