// ui-ax-dump.swift — Wave C AX 闸门取证 helper（测试工具，不进生产构建）。
//
// 按 owner PID 用 CGWindowList 找窗口，再经 AXUIElement 递归 dump
// role / subrole / title / value / identifier / description / actions，并可按
// identifier 执行 press / focus / set-value。
//
// 用法：
//   ui-ax-dump --pid <pid> [--out <file>] [--wid-out <file>] [--max-depth N]
//              [--press <identifier> | --focus <identifier>
//               | --set-value <identifier> <value>] [--action-only]
//
// 退出码：0 写出 AX 树（含权限不足时的 WARN）；2 参数错误；3 指定 PID 无 CG 窗口。

import ApplicationServices
import CoreGraphics
import Foundation

struct Options {
    var pid: pid_t = 0
    var outPath: String = "-"
    var widOutPath: String? = nil
    var maxDepth: Int = 12
    var maxChildren: Int = 80
    var action: RequestedAction? = nil
    var actionOnly = false
}

enum RequestedAction {
    case press(String)
    case focus(String)
    case setValue(String, String)
}

struct CGWin {
    var id: CGWindowID
    var name: String
    var owner: String
    var layer: Int
    var onscreen: Bool
    var alpha: Double
    var bounds: CGRect
}

func die(_ message: String, code: Int32) -> Never {
    fputs("ui-ax-dump: \(message)\n", stderr)
    exit(code)
}

func parseArgs(_ argv: [String]) -> Options {
    var opts = Options()
    var i = 0
    while i < argv.count {
        let flag = argv[i]
        func needValue() -> String {
            i += 1
            guard i < argv.count else { die("\(flag) 缺少值", code: 2) }
            return argv[i]
        }
        switch flag {
        case "--pid":
            guard let value = Int32(needValue()) else { die("--pid 必须是整数", code: 2) }
            opts.pid = value
        case "--out":
            opts.outPath = needValue()
        case "--wid-out":
            opts.widOutPath = needValue()
        case "--max-depth":
            guard let value = Int(needValue()), value >= 0 else {
                die("--max-depth 必须是非负整数", code: 2)
            }
            opts.maxDepth = value
        case "--max-children":
            guard let value = Int(needValue()), value >= 0 else {
                die("--max-children 必须是非负整数", code: 2)
            }
            opts.maxChildren = value
        case "--press":
            guard opts.action == nil else { die("每次只能请求一个 AX action", code: 2) }
            opts.action = .press(needValue())
        case "--focus":
            guard opts.action == nil else { die("每次只能请求一个 AX action", code: 2) }
            opts.action = .focus(needValue())
        case "--set-value":
            guard opts.action == nil else { die("每次只能请求一个 AX action", code: 2) }
            let identifier = needValue()
            let value = needValue()
            opts.action = .setValue(identifier, value)
        case "--action-only":
            opts.actionOnly = true
        case "-h", "--help":
            fputs(
                "用法：ui-ax-dump --pid <pid> [--out <file>] [--wid-out <file>] [--max-depth N] [--max-children N] [--press <id> | --focus <id> | --set-value <id> <value>] [--action-only]\n",
                stderr)
            exit(0)
        default:
            die("未知参数 \(flag)", code: 2)
        }
        i += 1
    }
    if opts.pid <= 0 { die("必须提供 --pid <pid>", code: 2) }
    if opts.actionOnly && opts.action == nil {
        die("--action-only 必须与 AX action 同用", code: 2)
    }
    return opts
}

func escapeText(_ raw: String) -> String {
    var out = ""
    out.reserveCapacity(raw.count)
    for ch in raw {
        switch ch {
        case "\n": out += "\\n"
        case "\r": out += "\\r"
        case "\t": out += "\\t"
        default: out.append(ch)
        }
    }
    if out.count > 240 {
        return String(out.prefix(237)) + "..."
    }
    return out
}

func stringify(_ value: AnyObject?) -> String? {
    guard let value else { return nil }
    if let text = value as? String { return text }
    if let number = value as? NSNumber { return number.stringValue }
    if CFGetTypeID(value) == CFAttributedStringGetTypeID() {
        return CFAttributedStringGetString((value as! CFAttributedString)) as String
    }
    if CFGetTypeID(value) == CFBooleanGetTypeID() {
        return CFBooleanGetValue((value as! CFBoolean)) ? "true" : "false"
    }
    return String(describing: value)
}

func axCopy(_ element: AXUIElement, _ name: String) -> AnyObject? {
    var value: AnyObject?
    let err = AXUIElementCopyAttributeValue(element, name as CFString, &value)
    guard err == .success else { return nil }
    return value
}

func axString(_ element: AXUIElement, _ name: String) -> String? {
    stringify(axCopy(element, name))
}

func axActions(_ element: AXUIElement) -> [String] {
    var names: CFArray?
    let err = AXUIElementCopyActionNames(element, &names)
    guard err == .success, let names else { return [] }
    return (names as NSArray).compactMap { $0 as? String }
}

func axIsSettable(_ element: AXUIElement, _ name: String) -> Bool {
    var settable = DarwinBoolean(false)
    let err = AXUIElementIsAttributeSettable(element, name as CFString, &settable)
    return err == .success && settable.boolValue
}

func axChildren(_ element: AXUIElement) -> [AXUIElement] {
    guard let value = axCopy(element, kAXChildrenAttribute as String) else { return [] }
    return (value as? NSArray)?.compactMap { child in
        (child as! AXUIElement)
    } ?? []
}

func findElement(
    _ root: AXUIElement, identifier: String, maxDepth: Int = 24
) -> AXUIElement? {
    var pending: [(AXUIElement, Int)] = [(root, 0)]
    while let (element, depth) = pending.popLast() {
        if axString(element, kAXIdentifierAttribute as String) == identifier {
            return element
        }
        if depth < maxDepth {
            for child in axChildren(element).reversed() {
                pending.append((child, depth + 1))
            }
        }
    }
    return nil
}

func performRequestedAction(
    _ action: RequestedAction, application: AXUIElement
) -> (String, AXError) {
    let identifier: String
    switch action {
    case .press(let value), .focus(let value), .setValue(let value, _):
        identifier = value
    }
    guard let element = findElement(application, identifier: identifier) else {
        return ("target=\(identifier) result=not-found", .noValue)
    }
    switch action {
    case .press:
        let error = AXUIElementPerformAction(element, kAXPressAction as CFString)
        return ("kind=press target=\(identifier) result=\(error.rawValue)", error)
    case .focus:
        let error = AXUIElementSetAttributeValue(
            element, kAXFocusedAttribute as CFString, kCFBooleanTrue)
        return ("kind=focus target=\(identifier) result=\(error.rawValue)", error)
    case .setValue(_, let value):
        let error = AXUIElementSetAttributeValue(
            element, kAXValueAttribute as CFString, value as CFTypeRef)
        return ("kind=set-value target=\(identifier) result=\(error.rawValue)", error)
    }
}

func listCGWindows(pid: pid_t) -> [CGWin] {
    let options: CGWindowListOption = [.optionAll, .excludeDesktopElements]
    guard let info = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] else {
        return []
    }
    var wins: [CGWin] = []
    for entry in info {
        guard let ownerPid = entry[kCGWindowOwnerPID as String] as? Int, ownerPid == Int(pid) else {
            continue
        }
        let number = UInt32(entry[kCGWindowNumber as String] as? Int ?? 0)
        let name = entry[kCGWindowName as String] as? String ?? ""
        let owner = entry[kCGWindowOwnerName as String] as? String ?? ""
        let layer = entry[kCGWindowLayer as String] as? Int ?? 0
        let onscreen = (entry[kCGWindowIsOnscreen as String] as? Bool) ?? false
        let alpha = entry[kCGWindowAlpha as String] as? Double ?? 1
        var bounds = CGRect.zero
        if let dict = entry[kCGWindowBounds as String] as? [String: Any] {
            let x = dict["X"] as? CGFloat ?? 0
            let y = dict["Y"] as? CGFloat ?? 0
            let w = dict["Width"] as? CGFloat ?? 0
            let h = dict["Height"] as? CGFloat ?? 0
            bounds = CGRect(x: x, y: y, width: w, height: h)
        }
        wins.append(
            CGWin(
                id: number, name: name, owner: owner, layer: layer, onscreen: onscreen,
                alpha: alpha, bounds: bounds))
    }
    wins.sort { lhs, rhs in
        if lhs.layer != rhs.layer { return lhs.layer < rhs.layer }
        let leftArea = lhs.bounds.width * lhs.bounds.height
        let rightArea = rhs.bounds.width * rhs.bounds.height
        return leftArea > rightArea
    }
    return wins
}

struct DumpStats {
    var nodes = 0
    var truncated = 0
    var roles: [String: Int] = [:]
    var identifiers: [String] = []
    var customHints: [String] = []
}

func dumpNode(
    _ element: AXUIElement,
    depth: Int,
    maxDepth: Int,
    maxChildren: Int,
    into lines: inout [String],
    stats: inout DumpStats
) {
    stats.nodes += 1
    let role = axString(element, kAXRoleAttribute as String) ?? "?"
    let subrole = axString(element, kAXSubroleAttribute as String)
    let title = axString(element, kAXTitleAttribute as String)
    let value = axString(element, kAXValueAttribute as String)
    let identifier = axString(element, kAXIdentifierAttribute as String)
    let description = axString(element, kAXDescriptionAttribute as String)
    let help = axString(element, kAXHelpAttribute as String)
    let enabled = axString(element, kAXEnabledAttribute as String)
    let focused = axString(element, kAXFocusedAttribute as String)
    let selected = axString(element, kAXSelectedAttribute as String)
    let actions = axActions(element)
    let settable = [
        (kAXValueAttribute as String, "AXValue"),
        (kAXFocusedAttribute as String, "AXFocused"),
    ].compactMap { attribute, label in
        axIsSettable(element, attribute) ? label : nil
    }
    stats.roles[role, default: 0] += 1
    if let identifier, !identifier.isEmpty { stats.identifiers.append(identifier) }

    var parts: [String] = ["role=\(role)"]
    if let subrole, !subrole.isEmpty { parts.append("subrole=\(subrole)") }
    if let title, !title.isEmpty { parts.append("title=\"\(escapeText(title))\"") }
    if let value, !value.isEmpty { parts.append("value=\"\(escapeText(value))\"") }
    if let identifier, !identifier.isEmpty {
        parts.append("identifier=\"\(escapeText(identifier))\"")
    }
    if let description, !description.isEmpty {
        parts.append("description=\"\(escapeText(description))\"")
    }
    if let help, !help.isEmpty { parts.append("help=\"\(escapeText(help))\"") }
    if let enabled { parts.append("enabled=\(enabled)") }
    if let focused { parts.append("focused=\(focused)") }
    if let selected { parts.append("selected=\(selected)") }
    if !actions.isEmpty { parts.append("actions=[\(actions.joined(separator: ","))]") }
    if !settable.isEmpty { parts.append("settable=[\(settable.joined(separator: ","))]") }

    let indent = String(repeating: "  ", count: depth)
    lines.append("\(indent)\(parts.joined(separator: " "))")

    let haystack = [title, description, identifier, value]
        .compactMap { $0?.lowercased() }
        .joined(separator: " ")
    if haystack.contains("pawork") || haystack.contains("composer")
        || haystack.contains("timeline") || haystack.contains("taskrail")
        || haystack.contains("inspector")
    {
        stats.customHints.append("depth=\(depth) \(parts.joined(separator: " "))")
    }

    let children = axChildren(element)
    if depth >= maxDepth {
        if !children.isEmpty {
            stats.truncated += 1
            lines.append("\(indent)  … truncated (\(children.count) children, max-depth=\(maxDepth))")
        }
        return
    }
    let shown = min(children.count, maxChildren)
    for index in 0..<shown {
        dumpNode(
            children[index], depth: depth + 1, maxDepth: maxDepth, maxChildren: maxChildren,
            into: &lines, stats: &stats)
    }
    if children.count > shown {
        stats.truncated += 1
        lines.append("\(indent)  … \(children.count - shown) more children omitted")
    }
}

func pickCaptureWindow(_ wins: [CGWin]) -> CGWin? {
    let normal = wins.filter { $0.layer == 0 && $0.bounds.width >= 64 && $0.bounds.height >= 64 }
    if let onscreen = normal.first(where: { $0.onscreen }) { return onscreen }
    if let first = normal.first { return first }
    return wins.first
}

func emit(_ lines: [String], to path: String) {
    let body = lines.joined(separator: "\n") + "\n"
    if path == "-" {
        fputs(body, stdout)
        return
    }
    do {
        try body.write(toFile: path, atomically: true, encoding: .utf8)
    } catch {
        die("写 out 失败：\(error)", code: 1)
    }
}

func main() {
    let opts = parseArgs(Array(CommandLine.arguments.dropFirst()))
    let wins = listCGWindows(pid: opts.pid)
    let trusted = AXIsProcessTrusted()
    var lines: [String] = []
    lines.append("# ui-ax-dump")
    lines.append("# pid=\(opts.pid)")
    lines.append("# ax_trusted=\(trusted)")
    lines.append("# generated_at=\(ISO8601DateFormatter().string(from: Date()))")
    lines.append("# max_depth=\(opts.maxDepth) max_children=\(opts.maxChildren)")
    lines.append("#")
    lines.append("# CGWindowList (owner pid \(opts.pid)), count=\(wins.count)")
    if wins.isEmpty {
        lines.append("#   (none)")
    } else {
        for win in wins {
            let on = win.onscreen ? "onscreen" : "offscreen"
            lines.append(
                "#   wid=\(win.id) owner=\"\(escapeText(win.owner))\" title=\"\(escapeText(win.name))\" layer=\(win.layer) \(on) alpha=\(win.alpha) bounds={\(Int(win.bounds.origin.x)),\(Int(win.bounds.origin.y)),\(Int(win.bounds.width)),\(Int(win.bounds.height))}"
            )
        }
    }

    if let capture = pickCaptureWindow(wins), let path = opts.widOutPath {
        do {
            try "\(capture.id)\n".write(toFile: path, atomically: true, encoding: .utf8)
        } catch {
            die("写 wid-out 失败：\(error)", code: 1)
        }
    }

    lines.append("#")
    if wins.isEmpty {
        lines.append("# ERROR: 指定 PID 在 CGWindowList 中没有任何窗口")
        emit(lines, to: opts.outPath)
        exit(3)
    }

    if !trusted {
        lines.append("# WARN: 当前进程未被授予 Accessibility 权限；AX 树可能为空或只有系统 chrome")
    }
    let application = AXUIElementCreateApplication(opts.pid)
    var actionError: AXError = .success
    if let action = opts.action {
        let (trace, error) = performRequestedAction(action, application: application)
        actionError = error
        lines.append("# action \(trace)")
    }
    if opts.actionOnly {
        emit(lines, to: opts.outPath)
        exit(actionError == .success ? 0 : 4)
    }
    lines.append("# AX tree")
    var stats = DumpStats()
    dumpNode(
        application,
        depth: 0,
        maxDepth: opts.maxDepth,
        maxChildren: opts.maxChildren,
        into: &lines,
        stats: &stats)

    lines.append("#")
    lines.append("# summary nodes=\(stats.nodes) truncated=\(stats.truncated)")
    let roleParts = stats.roles.keys.sorted().map { "\($0)=\(stats.roles[$0] ?? 0)" }
    lines.append("# roles \(roleParts.joined(separator: " "))")
    if stats.identifiers.isEmpty {
        lines.append("# identifiers (none)")
    } else {
        lines.append("# identifiers \(stats.identifiers.joined(separator: " | "))")
    }
    if stats.customHints.isEmpty {
        lines.append("# custom_hints (none)")
    } else {
        for hint in stats.customHints {
            lines.append("# custom_hint \(hint)")
        }
    }
    emit(lines, to: opts.outPath)
    if actionError != .success {
        exit(4)
    }
}

main()
