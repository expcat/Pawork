// ui-ax-frames.swift — AX frame probe helper（Wave D/B 测试工具，不进生产构建）。
//
// 按 owner PID 枚举应用 AX 树中带 identifier 的元素 frame；可选动作：
//   --place-main  把第一个窗口居中到主屏（kAXPositionAttribute + 收敛轮询）
//   --resize WxH  设置第一个窗口尺寸（kAXSizeAttribute + 收敛轮询）
//
// 用法：ui-ax-frames <pid> [--place-main] [--resize WxH]
// 退出码：0 成功（动作收敛 + frame dump）；2 参数错误；
//         3 窗口/frame 不可用或动作未收敛。

import ApplicationServices
import CoreGraphics
import Foundation

func die(_ message: String, code: Int32) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(code)
}

var pid: Int32 = 0
var placeMain = false
var resizeTarget: CGSize? = nil

let rawArgs = Array(CommandLine.arguments.dropFirst())
var i = 0
while i < rawArgs.count {
    let flag = rawArgs[i]
    func needValue() -> String {
        i += 1
        guard i < rawArgs.count else {
            die("ui-ax-frames: \(flag) 缺少值", code: 2)
        }
        return rawArgs[i]
    }
    switch flag {
    case "--place-main":
        placeMain = true
    case "--resize":
        let value = needValue()
        let parts = value.split(separator: "x").map(String.init)
        guard parts.count == 2,
              let w = Double(parts[0]), let h = Double(parts[1]),
              w > 0, h > 0 else {
            die("ui-ax-frames: --resize 需要 WxH（正数），收到 \(value)", code: 2)
        }
        resizeTarget = CGSize(width: w, height: h)
    case "-h", "--help":
        print("用法：ui-ax-frames <pid> [--place-main] [--resize WxH]")
        exit(0)
    default:
        guard pid == 0, let parsed = Int32(flag), parsed > 0 else {
            die("ui-ax-frames: 未知参数 \(flag)", code: 2)
        }
        pid = parsed
    }
    i += 1
}
guard pid > 0 else { die("ui-ax-frames: 必须提供 <pid>", code: 2) }

let app = AXUIElementCreateApplication(pid)

func attr(_ e: AXUIElement, _ name: String) -> AnyObject? {
    var v: AnyObject?
    guard AXUIElementCopyAttributeValue(e, name as CFString, &v) == .success else { return nil }
    return v
}

func str(_ e: AXUIElement, _ name: String) -> String? {
    guard let v = attr(e, name) else { return nil }
    if let s = v as? String { return s }
    if let n = v as? NSNumber { return n.stringValue }
    return nil
}

func frame(_ e: AXUIElement) -> (CGPoint, CGSize)? {
    guard let pv = attr(e, kAXPositionAttribute as String),
          let sv = attr(e, kAXSizeAttribute as String) else { return nil }
    var p = CGPoint()
    var s = CGSize()
    guard AXValueGetValue(pv as! AXValue, .cgPoint, &p),
          AXValueGetValue(sv as! AXValue, .cgSize, &s) else { return nil }
    return (p, s)
}

func firstWindow() -> AXUIElement? {
    guard let windows = attr(app, kAXWindowsAttribute as String) as? [AXUIElement] else {
        return nil
    }
    return windows.first
}

if placeMain {
    guard let window = firstWindow(),
          let (_, size) = frame(window) else {
        die("ui-ax-frames: place-main AX window/frame 不可用", code: 3)
    }
    let display = CGDisplayBounds(CGMainDisplayID())
    var target = CGPoint(
        x: display.minX + max(0, (display.width - size.width) / 2),
        y: display.minY + max(0, (display.height - size.height) / 2)
    )
    guard let value = AXValueCreate(.cgPoint, &target) else {
        die("ui-ax-frames: place-main 无法创建 AX position", code: 3)
    }
    let result = AXUIElementSetAttributeValue(
        window,
        kAXPositionAttribute as CFString,
        value
    )
    var actual = CGPoint(x: CGFloat.nan, y: CGFloat.nan)
    if result == .success {
        for _ in 0..<50 {
            if let (point, _) = frame(window) {
                actual = point
                if abs(point.x - target.x) <= 1 && abs(point.y - target.y) <= 1 {
                    break
                }
            }
            usleep(20_000)
        }
    }
    print("# place-main result=" + String(result.rawValue)
        + " display=" + String(CGMainDisplayID())
        + " target={" + String(describing: target.x) + "," + String(describing: target.y) + "}"
        + " actual={" + String(describing: actual.x) + "," + String(describing: actual.y) + "}")
    if result != .success || abs(actual.x - target.x) > 1 || abs(actual.y - target.y) > 1 {
        exit(3)
    }
}

if let target = resizeTarget {
    guard let window = firstWindow() else {
        die("ui-ax-frames: resize AX window 不可用", code: 3)
    }
    var value = target
    guard let axValue = AXValueCreate(.cgSize, &value) else {
        die("ui-ax-frames: resize 无法创建 AX size", code: 3)
    }
    let result = AXUIElementSetAttributeValue(
        window,
        kAXSizeAttribute as CFString,
        axValue
    )
    var actual = CGSize(width: CGFloat.nan, height: CGFloat.nan)
    if result == .success {
        // 与断言层 root ±0.5 合同对齐的收敛窗口（~8s @40ms）。
        for _ in 0..<200 {
            if let (_, s) = frame(window) {
                actual = s
                if abs(s.width - target.width) <= 0.5 && abs(s.height - target.height) <= 0.5 {
                    break
                }
            }
            usleep(40_000)
        }
    }
    print("# resize result=" + String(result.rawValue)
        + " target={" + String(describing: target.width) + "," + String(describing: target.height) + "}"
        + " actual={" + String(describing: actual.width) + "," + String(describing: actual.height) + "}")
    if result != .success
        || abs(actual.width - target.width) > 0.5
        || abs(actual.height - target.height) > 0.5 {
        exit(3)
    }
}

var out: [String] = []
var queue: [AXUIElement] = [app]
var seen = 0
while !queue.isEmpty {
    let e = queue.removeFirst()
    seen += 1
    if seen > 600 { break }
    let role = str(e, kAXRoleAttribute as String) ?? "?"
    if let id = str(e, kAXIdentifierAttribute as String), !id.isEmpty {
        if let (p, s) = frame(e) {
            out.append("id=" + id + " role=" + role
                + " x=" + String(describing: p.x)
                + " y=" + String(describing: p.y)
                + " w=" + String(describing: s.width)
                + " h=" + String(describing: s.height))
        } else {
            out.append("id=" + id + " role=" + role + " x=? y=? w=? h=?")
        }
    }
    if let kids = attr(e, kAXChildrenAttribute as String) as? [AXUIElement] {
        queue.append(contentsOf: kids)
    }
}
print("# ax-frames pid=" + String(describing: pid) + " nodes=" + String(describing: seen))
for line in out { print(line) }
