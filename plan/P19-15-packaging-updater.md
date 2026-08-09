# P19-15：打包、签名与自动更新

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-1、P19-3、P19-14

**最终目的**：建立 Windows/macOS/Linux 可复现、可验证、可回退的 Desktop 发布链，使安装包、平台签名/notarization、SBOM/provenance 与强制验签更新成为发布前置，而不是手工收尾。

**涉及范围**：Tauri bundle config、icons/metadata/licenses、release workflows、code signing/notarization、updater metadata/signature、rollback/compatibility policy

## 细分步骤

1. **版本与产物矩阵** —— 统一 app/protocol/min-host version，定义 Windows installer、macOS app/DMG、Linux AppImage/deb/rpm 的首轮矩阵。目的：版本兼容可判断。
2. **可复现构建** —— pinned Rust/Node/package lock、clean build、artifact checksum、SBOM/license/provenance。目的：供应链可审计。
3. **平台签名** —— Windows code sign、macOS code sign/notarize、Linux checksum/signature；私钥只在受控 CI secret。目的：用户可验证来源。
4. **Updater** —— 官方 updater、独立更新签名、公钥 pin、channel/rollout、download/install/restart progress。目的：更新不可降级为未验签下载。
5. **兼容与回退** —— 更新前检查 GUI/Host protocol range，失败保留旧版本，数据/偏好迁移可回滚。目的：Desktop 更新不破坏 Host。
6. **发布演练** —— unsigned PR build、signed staging、升级/中断/篡改/回退 fixtures 与产物留档。目的：发布流程可重复。

## 主要产出物

- 三平台 Tauri bundle/release workflows
- code signing/notarization/updater key 与 secret 操作手册（不含私钥）
- SBOM/checksum/provenance、upgrade/tamper/rollback tests

## 验收标准

- [ ] clean checkout 可生成声明矩阵中的安装包与 checksum/SBOM/provenance
- [ ] Windows/macOS 发布包验证平台签名/notarization；Linux 发布 checksum/signature
- [ ] updater 拒绝缺失/错误签名、未知 channel、协议不兼容与非显式 downgrade
- [ ] signing/updater private key 不进入仓库、日志、artifact 或 fork PR
- [ ] 更新中断/失败后旧版本仍可启动并连接兼容 Host

**相关文档**：[Desktop GUI](../docs/features/desktop-gui.md) · [security acceptance](../docs/quality/security-acceptance.md) · [P19-16](P19-16-desktop-gate.md)

**依赖建议（2026-08）**：采用 Tauri 官方 bundler/updater；updater 验签不可关闭。参考 [Tauri Distribution](https://v2.tauri.app/distribute/) 与 [Updater API](https://v2.tauri.app/reference/javascript/updater/)。
