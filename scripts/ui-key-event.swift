// ui-key-event.swift — R3 Wave B 键盘注入 helper（测试工具，不进生产构建）。
//
// 按按键名或虚拟键码构造 CGEvent（可带修饰键标志），发 key down + key up。
// 投递路径：CGEventPost(.cghidEventTap)（CGEventPostToPid 的等价 working 路径——
// 实测 CGEventPostToPid 对 GPUI/AppKit 键盘分发不可达；全局 HID tap 投递经
// 正常事件路由进前台应用，--pid 用于存活校验与调用方契约）。键盘事件源用
// .hidSystemState（物理输入源）：实测 combinedSessionState 源的合成键到不了
// GPUI 元素级 on_key_down（action binding 除外）；hid 源的 ↓/Enter/字母键
// 可稳定驱动菜单高亮、选择与 rail 焦点链。Escape 例外：合成 Escape 在任何
// 源下都会被 AppKit 路由到 cancelOperation: 而非 keyDown:，调用方需改用
// System Events keystroke（key code 53）。要求目标应用
// 已被置前（调用方先用 ui-focus-switch.sh activate --pid 收敛）。同步语义
// 交给调用方：本工具不做同步等待/轮询；key down 与 key up 之间仅保留
// 事件合成所需的物理按压间隔（80ms，AppKit/GPUI 对零间隔 down-up 会
// 丢事件，与真键盘按压时长同语义，不属于同步猜测）。方向键 / Home /
// End / PageUp / PageDown 合成事件必须附带 SecondaryFn 标志（AppKit 对
// 缺该标志的 fn 类按键按无键处理，实测被静默丢弃），一并在此注入。
// 标志位强制赋值：hidSystemState 源创建的事件会继承源当前 flags（探针
// 实证 cmd-alt-n 投递后 cmd|alt 粘滞在 HID 状态，后续裸 Tab 以 0x180000
// 到达，被 Tab 本地监听器按"带修饰键组合"放行后遭 AppKit 吞掉），故
// event.flags 无条件赋成请求值，合成键永不携带继承的粘滞修饰键。
//
// 用法：
//   ui-key-event --pid <pid> --key <name|code> [--modifiers cmd,alt,shift,ctrl]
//                [--down-only | --up-only]
//   ui-key-event --pid <pid> --click-at <x>,<y>
//
// 按键名（大小写不敏感）：tab return enter escape esc space delete
//   up down left right home end pageup pagedown help a-z 0-9。
// 修饰键别名：cmd/command/super、alt/option/opt、shift、ctrl/control。
//
// 退出码：0 全部事件投递成功；2 参数错误；3 事件构造/投递失败；4 PID 不在运行。

import CoreGraphics
import Foundation

struct Options {
    var pid: pid_t = 0
    var key: String = ""
    var modifiers: CGEventFlags = []
    var downOnly = false
    var upOnly = false
    var clickAt: String = ""
}

func die(_ message: String, code: Int32) -> Never {
    fputs("ui-key-event: \(message)\n", stderr)
    exit(code)
}

let keyCodes: [String: CGKeyCode] = [
    "tab": 48,
    "return": 36,
    "enter": 36,
    "escape": 53,
    "esc": 53,
    "space": 49,
    "delete": 51,
    "up": 126,
    "down": 125,
    "left": 123,
    "right": 124,
    "home": 115,
    "end": 119,
    "pageup": 116,
    "pagedown": 121,
    "help": 114,
]

// 字母 / 数字虚拟键码（kVK_ANSI_*；测试所需的常用键子集）。
let letterCodes: [String: CGKeyCode] = [
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8,
    "v": 9, "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17,
    "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23, "9": 25,
    "7": 26, "o": 31, "u": 32, "i": 34, "p": 35, "l": 37,
    "j": 38, "k": 40, "n": 45,
    "m": 46, "0": 29, "8": 28,
]

let modifierFlags: [String: CGEventFlags] = [
    "cmd": .maskCommand,
    "command": .maskCommand,
    "super": .maskCommand,
    "alt": .maskAlternate,
    "option": .maskAlternate,
    "opt": .maskAlternate,
    "shift": .maskShift,
    "ctrl": .maskControl,
    "control": .maskControl,
]

// fn 类按键：合成事件需附带 SecondaryFn 才会被 AppKit 识别为方向键。
let fnClassKeys: Set<String> = [
    "up", "down", "left", "right", "home", "end", "pageup", "pagedown", "help",
]

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
            guard let value = Int32(needValue()), value > 0 else {
                die("--pid 必须是正整数", code: 2)
            }
            opts.pid = value
        case "--key":
            opts.key = needValue().lowercased()
        case "--modifiers":
            for name in needValue().split(separator: ",").map(String.init) {
                let lowered = name.lowercased()
                guard let flags = modifierFlags[lowered] else {
                    die("未知修饰键 \(name)（可用：cmd,alt,shift,ctrl）", code: 2)
                }
                opts.modifiers.insert(flags)
            }
        case "--down-only":
            opts.downOnly = true
        case "--up-only":
            opts.upOnly = true
        case "--click-at":
            opts.clickAt = needValue()
        case "-h", "--help":
            fputs(
                "用法：ui-key-event --pid <pid> --key <name|code> [--modifiers cmd,alt,shift,ctrl] [--down-only | --up-only]\n"
                    + "      ui-key-event --pid <pid> --click-at <x>,<y>\n",
                stderr)
            exit(0)
        default:
            die("未知参数 \(flag)", code: 2)
        }
        i += 1
    }
    if opts.pid <= 0 { die("必须提供 --pid <pid>", code: 2) }
    if opts.downOnly && opts.upOnly {
        die("--down-only 与 --up-only 互斥", code: 2)
    }
    if !opts.clickAt.isEmpty {
        if !opts.key.isEmpty || opts.downOnly || opts.upOnly {
            die("--click-at 与键盘参数互斥", code: 2)
        }
    } else if opts.key.isEmpty {
        die("必须提供 --key <name|code> 或 --click-at <x>,<y>", code: 2)
    }
    return opts
}

func resolveKeyCode(_ raw: String) -> CGKeyCode? {
    if let code = keyCodes[raw] { return code }
    if raw.count == 1, let code = letterCodes[raw] { return code }
    if let code = UInt16(raw), code != 0 { return code }
    return nil
}

func eventFlags(key: String, modifiers: CGEventFlags) -> CGEventFlags {
    var flags = modifiers
    if fnClassKeys.contains(key) {
        flags.insert(.maskSecondaryFn)
    }
    return flags
}

func postEvent(
    _ source: CGEventSource, _ pid: pid_t, keyCode: CGKeyCode,
    modifiers: CGEventFlags, keyDown: Bool
) {
    guard
        let event = CGEvent(
            keyboardEventSource: source, virtualKey: keyCode, keyDown: keyDown)
    else {
        die("CGEvent 构造失败 keyCode=\(keyCode)", code: 3)
    }
    let flags = eventFlags(key: opts.key, modifiers: modifiers)
    // 无条件赋值：见文件头"标志位强制赋值"说明。
    event.flags = flags
    // 全局 HID tap：经窗口服务器正常路由到前台应用（等价 CGEventPostToPid
// 的可达路径；pid 参数用于存活校验，投递目标由前台状态决定）。
    event.post(tap: .cghidEventTap)
}

let opts = parseArgs(Array(CommandLine.arguments.dropFirst()))
guard kill(opts.pid, 0) == 0 else {
    die("目标 PID 不在运行：\(opts.pid)", code: 4)
}
if !opts.clickAt.isEmpty {
    let parts = opts.clickAt.split(separator: ",").map { Double($0) }
    guard parts.count == 2, let x = parts[0], let y = parts[1] else {
        die("--click-at 格式必须是 <x>,<y>", code: 2)
    }
    guard let source = CGEventSource(stateID: .combinedSessionState) else {
        die("CGEventSource 创建失败", code: 3)
    }
    let point = CGPoint(x: x, y: y)
    guard
        let moved = CGEvent(
            mouseEventSource: source, mouseType: .mouseMoved,
            mouseCursorPosition: point, mouseButton: .left),
        let down = CGEvent(
            mouseEventSource: source, mouseType: .leftMouseDown,
            mouseCursorPosition: point, mouseButton: .left),
        let up = CGEvent(
            mouseEventSource: source, mouseType: .leftMouseUp,
            mouseCursorPosition: point, mouseButton: .left)
    else {
        die("鼠标事件构造失败 click-at=\(opts.clickAt)", code: 3)
    }
    // 与键盘注入同路径：全局 HID tap，由前台窗口接收；moved 先落点再压放，
    // down-up 间保留与键盘一致的 80ms 物理按压间隔。
    moved.post(tap: .cghidEventTap)
    usleep(10_000)
    down.post(tap: .cghidEventTap)
    usleep(80_000)
    up.post(tap: .cghidEventTap)
    fputs("ui-key-event pid=\(opts.pid) click-at=\(opts.clickAt) posted\n", stderr)
    exit(0)
}
guard let keyCode = resolveKeyCode(opts.key) else {
    die("未知按键 \(opts.key)", code: 2)
}
guard let source = CGEventSource(stateID: .hidSystemState) else {
    die("CGEventSource 创建失败", code: 3)
}
if !opts.upOnly {
    postEvent(source, opts.pid, keyCode: keyCode, modifiers: opts.modifiers, keyDown: true)
}
usleep(80_000)
if !opts.downOnly {
    postEvent(source, opts.pid, keyCode: keyCode, modifiers: opts.modifiers, keyDown: false)
}
fputs(
    "ui-key-event pid=\(opts.pid) key=\(opts.key) code=\(keyCode) modifiers=\(opts.modifiers.rawValue) posted\n",
    stderr)
