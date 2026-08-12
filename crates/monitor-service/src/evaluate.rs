//! 确定性命中判定（P16-6 核心）。
//!
//! [`evaluate`] 是纯函数：给定一条 [`crate::MonitorConfig`] 与一条
//! [`crate::Observation`]，命中返回 detail 字符串。不访问文件系统 / 网络 /
//! 进程表，全部确定性可单测。真实 driver 把外部事件归一为 Observation 后
//! 调用本函数，保证「核心判定逻辑可独立单测」。

use std::path::Path;

use regex::Regex;

use crate::config::{MonitorConfig, Observation};

/// 纯函数判定：配置与观测样本来源一致且满足条件时返回 `Some(detail)`。
pub fn evaluate(config: &MonitorConfig, observation: &Observation) -> Option<String> {
    match (config, observation) {
        (
            MonitorConfig::FileChange { paths, pattern },
            Observation::FileChange { paths: changed },
        ) => evaluate_file_change(paths, pattern.as_deref(), changed),
        (
            MonitorConfig::ProcessExit { pid, task_id },
            Observation::ProcessExit {
                pid: obs_pid,
                task_id: obs_task,
                code,
            },
        ) => evaluate_process_exit(
            *pid,
            task_id.as_deref(),
            *obs_pid,
            obs_task.as_deref(),
            *code,
        ),
        (
            MonitorConfig::RegexMatch { stream, pattern },
            Observation::RegexMatch {
                stream: obs_stream,
                text,
            },
        ) => evaluate_regex_match(stream, pattern, obs_stream, text),
        (
            MonitorConfig::PortState { host, port },
            Observation::PortState {
                host: obs_host,
                port: obs_port,
                open,
            },
        ) => evaluate_port_state(host, *port, obs_host, *obs_port, *open),
        // 配置与观测来源不一致，永不命中。
        _ => None,
    }
}

fn evaluate_file_change(
    watched: &[String],
    pattern: Option<&str>,
    changed: &[String],
) -> Option<String> {
    let regex = pattern.and_then(|p| Regex::new(p).ok());
    for path in changed {
        if !watched.iter().any(|root| under(root, path)) {
            continue;
        }
        if let Some(regex) = &regex {
            if !regex.is_match(path) {
                continue;
            }
        }
        return Some(format!("file changed: {path}"));
    }
    None
}

/// `changed` 是否在 `root` 目录之下（或与 root 相等）。
fn under(root: &str, changed: &str) -> bool {
    let root = Path::new(root);
    let changed = Path::new(changed);
    changed == root || changed.starts_with(root)
}

fn evaluate_process_exit(
    pid: Option<u32>,
    task_id: Option<&str>,
    obs_pid: Option<u32>,
    obs_task: Option<&str>,
    code: Option<i32>,
) -> Option<String> {
    let pid_match = pid.is_some_and(|p| Some(p) == obs_pid);
    let task_match = task_id.is_some_and(|t| Some(t) == obs_task);
    if pid_match || task_match {
        Some(format!("process exited (code={code:?})"))
    } else {
        None
    }
}

fn evaluate_regex_match(
    stream: &str,
    pattern: &str,
    obs_stream: &str,
    text: &str,
) -> Option<String> {
    if stream != obs_stream {
        return None;
    }
    let regex = Regex::new(pattern).ok()?;
    let found = regex.find(text)?;
    Some(format!("regex matched: {}", found.as_str()))
}

fn evaluate_port_state(
    host: &str,
    port: u16,
    obs_host: &str,
    obs_port: u16,
    open: bool,
) -> Option<String> {
    if host == obs_host && port == obs_port {
        Some(format!(
            "port {port} {}",
            if open { "open" } else { "closed" }
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_change_matches_watched_path() {
        let cfg = MonitorConfig::FileChange {
            paths: vec!["/repo".into()],
            pattern: None,
        };
        let obs = Observation::FileChange {
            paths: vec!["/repo/src/a.rs".into()],
        };
        assert_eq!(
            evaluate(&cfg, &obs).as_deref(),
            Some("file changed: /repo/src/a.rs")
        );
    }

    #[test]
    fn file_change_ignores_out_of_tree() {
        let cfg = MonitorConfig::FileChange {
            paths: vec!["/repo".into()],
            pattern: None,
        };
        let obs = Observation::FileChange {
            paths: vec!["/etc/passwd".into()],
        };
        assert_eq!(evaluate(&cfg, &obs), None);
    }

    #[test]
    fn file_change_pattern_filters() {
        let cfg = MonitorConfig::FileChange {
            paths: vec!["/repo".into()],
            pattern: Some(r"\.rs$".into()),
        };
        let miss = Observation::FileChange {
            paths: vec!["/repo/a.txt".into()],
        };
        let hit = Observation::FileChange {
            paths: vec!["/repo/a.rs".into()],
        };
        assert_eq!(evaluate(&cfg, &miss), None);
        assert_eq!(
            evaluate(&cfg, &hit).as_deref(),
            Some("file changed: /repo/a.rs")
        );
    }

    #[test]
    fn process_exit_matches_by_pid_or_task() {
        let by_pid = MonitorConfig::ProcessExit {
            pid: Some(42),
            task_id: None,
        };
        let by_task = MonitorConfig::ProcessExit {
            pid: None,
            task_id: Some("t-1".into()),
        };
        let obs =
            |pid: Option<u32>, task: Option<&str>, code: Option<i32>| Observation::ProcessExit {
                pid,
                task_id: task.map(str::to_string),
                code,
            };
        assert!(evaluate(&by_pid, &obs(Some(42), None, Some(0))).is_some());
        assert!(evaluate(&by_pid, &obs(Some(7), None, Some(0))).is_none());
        assert!(evaluate(&by_task, &obs(None, Some("t-1"), None)).is_some());
        assert!(evaluate(&by_task, &obs(None, Some("t-2"), None)).is_none());
    }

    #[test]
    fn regex_match_returns_matched_substring() {
        let cfg = MonitorConfig::RegexMatch {
            stream: "stdout".into(),
            pattern: r"error: \w+".into(),
        };
        let obs = Observation::RegexMatch {
            stream: "stdout".into(),
            text: "boot ok\nerror: boom\ndone".into(),
        };
        assert_eq!(
            evaluate(&cfg, &obs).as_deref(),
            Some("regex matched: error: boom")
        );
        let wrong_stream = Observation::RegexMatch {
            stream: "stderr".into(),
            text: "error: boom".into(),
        };
        assert_eq!(evaluate(&cfg, &wrong_stream), None);
    }

    #[test]
    fn port_state_reports_open_closed() {
        let cfg = MonitorConfig::PortState {
            host: "127.0.0.1".into(),
            port: 8080,
        };
        let open = Observation::PortState {
            host: "127.0.0.1".into(),
            port: 8080,
            open: true,
        };
        let closed = Observation::PortState {
            host: "127.0.0.1".into(),
            port: 8080,
            open: false,
        };
        let other = Observation::PortState {
            host: "127.0.0.1".into(),
            port: 9090,
            open: true,
        };
        assert_eq!(evaluate(&cfg, &open).as_deref(), Some("port 8080 open"));
        assert_eq!(evaluate(&cfg, &closed).as_deref(), Some("port 8080 closed"));
        assert_eq!(evaluate(&cfg, &other), None);
    }

    #[test]
    fn mismatched_source_kind_never_matches() {
        let cfg = MonitorConfig::PortState {
            host: "h".into(),
            port: 80,
        };
        let obs = Observation::FileChange {
            paths: vec!["/x".into()],
        };
        assert_eq!(evaluate(&cfg, &obs), None);
    }
}
