//! `pawork` —— Pawork 的唯一正式可执行宿主（CLI 与 Core 同进程同二进制）。
//!
//! 当前为 P0-1 的占位入口：仅打印版本，确保 workspace 可构建。
//! 子命令与 Core 装配（serve/run/shell/watch/service）将在 P1-12 实现。

fn main() {
    println!("pawork {}", env!("CARGO_PKG_VERSION"));
}
