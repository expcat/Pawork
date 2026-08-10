# P19-15：打包、签名与自动更新

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-1、P19-3、P19-14

**最终目的**：建立 Windows/macOS/Linux 可复现、可验证、可回退的 Desktop 发布链，使安装包、平台签名/notarization、SBOM/provenance 与强制验签更新成为发布前置，而不是手工收尾。

**涉及范围**：GPUI Desktop bundle metadata、打包工具选型、icons/licenses、release workflows、code signing/notarization、独立 updater metadata/signature、rollback/compatibility policy

## 细分步骤

1. **版本与产物矩阵** —— 统一 app/protocol/min-host version，定义 Windows installer、macOS app/DMG 与 Linux 首轮格式；RPM 只有在真实构建链验证后才纳入承诺。目的：版本兼容和实际支持范围可判断。
2. **打包工具决策** —— 用 P19-1 产物比较 `cargo-packager` 等 Rust 工具与必要的平台原生工具，记录格式覆盖、签名、updater、维护性和缺口；GPUI 本身不被视为 bundler。目的：不把候选工具误写成既定能力。
3. **可复现构建** —— 精确锁定 Rust toolchain、GPUI revision 与 Cargo 依赖，执行 clean build、artifact checksum、SBOM/license/provenance；不引入 Node package lock。目的：供应链可审计。
4. **平台签名** —— Windows code sign、macOS code sign/notarize、Linux checksum/signature；私钥只在受控 CI secret。目的：用户可验证来源。
5. **Updater** —— 在 `DesktopPlatform` 后实现独立签名更新：公钥 pin、manifest/channel/rollout、download/install/restart progress、key rotation policy；不假定 GPUI 提供 updater。目的：更新不可降级为未验签下载。
6. **兼容与回退** —— 更新前检查 GUI/Host protocol range，失败保留旧版本；Desktop 只迁移可丢弃偏好，业务数据不随 GUI framework 迁移。目的：Desktop 更新不破坏 Host。
7. **许可证与发布演练** —— 区分 GPUI 依赖和 Zed GPL 源码，证明未复制后者；运行 unsigned PR build、signed staging、升级/中断/篡改/回退 fixtures 并留档。目的：发布流程与法律来源可复核。

## 主要产出物

- 三平台 GPUI Desktop bundle/release workflows 与打包工具决策记录
- code signing/notarization/updater key 与 secret 操作手册（不含私钥）
- SBOM/checksum/provenance、upgrade/tamper/rollback tests

## 验收标准

- [ ] clean checkout 可生成声明矩阵中的安装包与 checksum/SBOM/provenance
- [ ] Windows/macOS 发布包验证平台签名/notarization；Linux 发布 checksum/signature
- [ ] updater 拒绝缺失/错误签名、未知 channel、协议不兼容与非显式 downgrade
- [ ] signing/updater private key 不进入仓库、日志、artifact 或 fork PR
- [ ] 更新中断/失败后旧版本仍可启动并连接兼容 Host
- [ ] 产物清单只声明真实 runner 已生成的格式；未验证 RPM 等格式明确标为不支持/待验证
- [ ] license inventory 区分 GPUI 与 Zed，仓库中无未经许可复制的 Zed GPL UI 代码

**相关文档**：[Desktop GUI](../docs/features/desktop-gui.md) · [security acceptance](../docs/quality/security-acceptance.md) · [P19-16](P19-16-desktop-gate.md)

**依赖建议（2026-08）**：`cargo-packager` 仅作为候选，须与所需平台格式、签名和 updater 路线一起在 P19-1 Gate 实测；缺口可由 WiX/NSIS、Apple 原生签名工具或 Linux 原生打包补齐。最终工具及精确版本在决策记录中锁定，updater 验签不可关闭。
