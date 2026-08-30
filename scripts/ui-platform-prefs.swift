// ui-platform-prefs.swift — R7 Wave C 平台显示偏好只读快照。
//
// 只读取当前 macOS 会话公开的 Accessibility display preferences；不改
// 系统设置。快照本身不声称应用已响应偏好，需与真实窗口 palette/结构门禁
// 一起使用。

import AppKit
import Foundation

let workspace = NSWorkspace.shared
let process = ProcessInfo.processInfo
let payload: [String: Any] = [
    "generated_at": ISO8601DateFormatter().string(from: Date()),
    "platform": [
        "operating_system": process.operatingSystemVersionString,
        "locale": Locale.current.identifier,
    ],
    "accessibility_display": [
        "reduce_motion": workspace.accessibilityDisplayShouldReduceMotion,
        "increase_contrast": workspace.accessibilityDisplayShouldIncreaseContrast,
        "reduce_transparency": workspace.accessibilityDisplayShouldReduceTransparency,
        "differentiate_without_color": workspace.accessibilityDisplayShouldDifferentiateWithoutColor,
    ],
    "scope": "read_only_platform_snapshot",
]

do {
    let data = try JSONSerialization.data(
        withJSONObject: payload,
        options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
    )
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data([0x0a]))
} catch {
    fputs("ui-platform-prefs: JSON encode failed: \(error)\n", stderr)
    exit(2)
}
