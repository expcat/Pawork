# Pawork Browser Bridge（CU-15 外部浏览器控制扩展）

自有 MV3 扩展 + Rust native messaging 桥：Pawork 经 Desktop 在后台控制
本浏览器的标签页。固定动作集（tabs / attach / new_tab / navigate / read /
click / type / screenshot / close_tab / close），不提供任意脚本执行、下载、
密码与文件输入。

## 安装（需要用户在浏览器里确认）

1. 构建桥进程：`cargo build -p pawork-browser --bin pawork_browser_bridge`。
2. 写 native messaging manifest（Chrome 与 Edge 各一份，`PATH_TO_BIN` 换成
   上面产物的绝对路径）：

```json
{
  "name": "dev.pawork.browser_bridge",
  "description": "Pawork Browser Bridge",
  "path": "PATH_TO_BIN",
  "type": "stdio",
  "allowed_origins": ["chrome-extension://hgalnnlhankacinahkjmflinpobpanjh/"]
}
```

- Chrome：`~/Library/Application Support/Google/Chrome/NativeMessagingHosts/dev.pawork.browser_bridge.json`
- Edge：`~/Library/Application Support/Microsoft Edge/NativeMessagingHosts/dev.pawork.browser_bridge.json`

3. 浏览器打开 `chrome://extensions`（Edge 为 `edge://extensions`），开「开发者
   模式」，「加载已解压的扩展程序」选本目录。扩展 ID 必须是
   `hgalnnlhankacinahkjmflinpobpanjh`（manifest `key` 已钉死）。
4. 点扩展工具栏图标，勾选「允许 Pawork 在后台控制此浏览器」。同一时刻只
   允许一个浏览器的桥处于启用状态；多个启用时动作会被拒绝。

移除 = 扩展页删扩展 + 删除上面的 manifest JSON，无其它残留（发现目录
`~/.pawork/external-browser/` 由桥进程退出时自清）。
