//! Shell 命令高风险识别。
//!
//! [`classify_command`] 接收程序名与参数，判定是否属于高风险命令：
//! - 直接的危险程序（`sudo`/`dd`/`mkfs`/`shutdown`...）；
//! - 递归删除/改权（`rm -rf`、`chmod -R`、`chown -R`）；
//! - 危险 git 操作（`git push --force`、`git branch -D`）；
//! - 经 `sh -c "..."` 或字符串拼接（含 `&&`/`||`/`|`/`;`/换行）的复合命令，
//!   逐段拆分判定，并对嵌套 `bash -c '...'` 做兜底正则匹配。

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use crate::decision::CommandRisk;

/// 判定一条命令的风险等级。
pub fn classify_command(program: &str, args: &[String]) -> CommandRisk {
    // 1) `sh -c "<script>"`：对脚本逐段判定。
    if let Some(script) = extract_shell_script(program, args) {
        return classify_snippet(&script);
    }
    // 2) 程序名本身即含命令分隔符（整串当作脚本）。
    if contains_separator(program) {
        return classify_snippet(program);
    }
    // 3) 程序名含空白（如 `"rm -rf"`）：拆成 token 后按单条命令判定。
    let (prog, extra) = split_program(program);
    let mut all = extra;
    all.extend(args.iter().cloned());
    if classify_single(&prog, &all) {
        CommandRisk::Dangerous
    } else {
        CommandRisk::Safe
    }
}

/// 命中无论审批模式多宽松都不能静默执行的灾难命令地板。
pub(crate) fn hits_danger_floor(program: &str, args: &[String]) -> bool {
    if let Some(script) = extract_shell_script(program, args) {
        return snippet_hits_danger_floor(&script);
    }
    if contains_separator(program) {
        return snippet_hits_danger_floor(program);
    }
    let (prog, mut extra) = split_program(program);
    extra.extend(args.iter().cloned());
    catastrophic_single(&prog, &extra)
}

fn snippet_hits_danger_floor(text: &str) -> bool {
    let segments: Vec<&str> = match separators_regex() {
        Some(re) => re.split(text).collect(),
        None => vec![text],
    };
    segments.into_iter().any(|segment| {
        let tokens: Vec<String> = segment
            .split_whitespace()
            .map(|token| token.trim_matches(['\'', '"']).to_string())
            .collect();
        if tokens.is_empty() {
            return false;
        }
        if catastrophic_single(&tokens[0], &tokens[1..]) {
            return true;
        }
        if is_shell_program(&tokens[0]) {
            if let Some(index) = tokens.iter().position(|token| token == "-c") {
                return snippet_hits_danger_floor(&tokens[index + 1..].join(" "));
            }
        }
        false
    })
}

fn catastrophic_single(program: &str, args: &[String]) -> bool {
    let base = basename(program);
    match base.as_str() {
        "mkfs" => true,
        name if name.starts_with("mkfs.") => true,
        "dd" => args.iter().any(|arg| {
            let arg = arg.trim_matches(['\'', '"']);
            arg == "of=/dev" || arg.starts_with("of=/dev/")
        }),
        "rm" => {
            let recursive = args.iter().any(|arg| is_recursive_flag(arg));
            let force = args.iter().any(|arg| is_force_flag(arg));
            let root = args.iter().any(|arg| arg.trim_matches(['\'', '"']) == "/");
            recursive && force && root
        }
        _ => false,
    }
}

/// 判定一段 shell 脚本（可能含多条命令）的风险。
fn classify_snippet(text: &str) -> CommandRisk {
    let segments: Vec<&str> = match separators_regex() {
        Some(re) => re.split(text).collect(),
        None => vec![text],
    };
    for raw in segments {
        let segment = raw.trim();
        if segment.is_empty() {
            continue;
        }
        let tokens: Vec<String> = segment.split_whitespace().map(String::from).collect();
        if tokens.is_empty() {
            continue;
        }
        if classify_single(&tokens[0], &tokens[1..]) {
            return CommandRisk::Dangerous;
        }
        if redirection_dangerous(&tokens) {
            return CommandRisk::Dangerous;
        }
        // 兜底：捕获嵌套引用（如 `bash -c 'rm -rf /'`）。
        if text_contains_dangerous(segment) {
            return CommandRisk::Dangerous;
        }
    }
    CommandRisk::Safe
}

/// 判定单条命令（程序 + 参数）是否危险。
fn classify_single(program: &str, args: &[String]) -> bool {
    let base = basename(program);
    if is_dangerous_program(&base) {
        return true;
    }
    match base.as_str() {
        "rm" => rm_dangerous(args),
        "chmod" | "chown" => has_recursive_flag(args),
        "git" => git_push_force(args) || git_branch_delete(args),
        _ => false,
    }
}

fn split_program(program: &str) -> (String, Vec<String>) {
    if !program.chars().any(|c| c.is_whitespace()) {
        return (program.to_string(), Vec::new());
    }
    let mut iter = program.split_whitespace();
    let prog = iter.next().unwrap_or("").to_string();
    let rest: Vec<String> = iter.map(String::from).collect();
    (prog, rest)
}

fn basename(program: &str) -> String {
    Path::new(program)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.to_string())
}

fn is_dangerous_program(base: &str) -> bool {
    matches!(
        base,
        "sudo" | "su" | "dd" | "shutdown" | "reboot" | "halt" | "poweroff" | "format" | "reg"
    ) || base == "mkfs"
        || base.starts_with("mkfs.")
}

fn rm_dangerous(args: &[String]) -> bool {
    let recursive = args.iter().any(|a| is_recursive_flag(a));
    let broad = args.iter().any(|a| {
        matches!(
            a.as_str(),
            "/" | "~" | "$HOME" | "*" | "." | ".." | "/etc" | "/usr" | "/var" | "/home" | "/boot"
        )
    });
    recursive || broad
}

fn has_recursive_flag(args: &[String]) -> bool {
    args.iter().any(|a| is_recursive_flag(a))
}

fn is_recursive_flag(arg: &str) -> bool {
    if arg == "--recursive" || arg == "-R" || arg == "-r" {
        return true;
    }
    // 形如 `-rf` / `-fr` / `-Rv` 的组合短选项。
    if let Some(rest) = arg.strip_prefix('-') {
        if !rest.is_empty() && !rest.starts_with('-') {
            return rest.chars().any(|c| c == 'r' || c == 'R');
        }
    }
    false
}

fn is_force_flag(arg: &str) -> bool {
    if arg == "--force" || arg == "-f" {
        return true;
    }
    arg.strip_prefix('-')
        .is_some_and(|rest| !rest.starts_with('-') && rest.contains('f'))
}

fn git_push_force(args: &[String]) -> bool {
    let has_push = args.iter().any(|a| a == "push");
    let has_force = args.iter().any(|a| a == "--force" || a == "-f");
    has_push && has_force
}

fn git_branch_delete(args: &[String]) -> bool {
    let has_branch = args.iter().any(|a| a == "branch");
    let has_delete = args
        .iter()
        .any(|a| a == "-D" || a == "-d" || a == "--delete");
    has_branch && has_delete
}

fn contains_separator(s: &str) -> bool {
    s.contains("&&") || s.contains("||") || s.contains('|') || s.contains(';') || s.contains('\n')
}

fn is_shell_program(program: &str) -> bool {
    matches!(
        basename(program).as_str(),
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "ash" | "fish" | "csh" | "tcsh"
    )
}

/// 从 `shell -c "<script>"` 中提取脚本字符串。
fn extract_shell_script(program: &str, args: &[String]) -> Option<String> {
    if !is_shell_program(program) {
        return None;
    }
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-c" {
            return iter.next().cloned();
        }
    }
    None
}

fn redirection_dangerous(tokens: &[String]) -> bool {
    let mut i = 0;
    while i < tokens.len() {
        let target: Option<&str> = if tokens[i] == ">" || tokens[i] == ">>" || tokens[i] == "&>" {
            tokens.get(i + 1).map(String::as_str)
        } else {
            tokens[i]
                .strip_prefix('>')
                .map(|rest| rest.trim_start_matches('&'))
        };
        if let Some(t) = target {
            if !t.is_empty() && is_dangerous_redirect_target(t) {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn is_dangerous_redirect_target(target: &str) -> bool {
    target == "/"
        || target == "/etc"
        || target.starts_with("/dev/")
        || target.starts_with("/etc/")
        || target.starts_with("/usr/")
        || target.starts_with("/proc/")
        || target.starts_with("/sys/")
        || target.starts_with("/boot/")
        || target.starts_with("/var/")
}

fn separators_regex() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"&&|\|\||\||;|\n|\r").ok())
        .as_ref()
}

fn danger_regexes() -> &'static [Regex] {
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        // 左边界用 `[^A-Za-z0-9_]`（兼容引号/斜杠），右边界用 `\b`
        // 防止把 `rmdir` 这类前缀误判为 `rm`。
        [
            r"(?:^|[^A-Za-z0-9_])(?:sudo|su|dd|mkfs(?:\.\w+)?|shutdown|reboot|halt|poweroff|format)\b",
            r"(?:^|[^A-Za-z0-9_])rm\b[^&|;\n]*-[A-Za-z]*[rR]",
            r"(?:^|[^A-Za-z0-9_])rm\b\s+(?:/|~|\$HOME|\*|\.\.?)",
            r"(?:^|[^A-Za-z0-9_])chmod\b[^&|;\n]*-[A-Za-z]*R",
            r"(?:^|[^A-Za-z0-9_])chown\b[^&|;\n]*-[A-Za-z]*R",
            r"(?:^|[^A-Za-z0-9_])git\s+push\b[^&|;\n]*(?:--force\b|-f\b)",
            r"(?:^|[^A-Za-z0-9_])git\s+branch\b[^&|;\n]*(?:-D\b|-d\b|--delete\b)",
        ]
        .iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect()
    })
}

fn text_contains_dangerous(text: &str) -> bool {
    danger_regexes().iter().any(|re| re.is_match(text))
}

#[cfg(test)]
mod tests {
    use super::{classify_command, hits_danger_floor};
    use crate::decision::CommandRisk;

    fn danger(program: &str, args: &[&str]) -> CommandRisk {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        classify_command(program, &args)
    }

    fn floor(program: &str, args: &[&str]) -> bool {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        hits_danger_floor(program, &args)
    }

    #[test]
    fn rm_recursive_force_is_dangerous() {
        assert_eq!(danger("rm", &["-rf", "/"]), CommandRisk::Dangerous);
        assert_eq!(
            danger("rm", &["-r", "-f", "/tmp/x"]),
            CommandRisk::Dangerous
        );
        assert_eq!(danger("rm", &["--recursive", "."]), CommandRisk::Dangerous);
    }

    #[test]
    fn rm_single_file_is_safe() {
        assert_eq!(danger("rm", &["file.txt"]), CommandRisk::Safe);
    }

    #[test]
    fn rm_broad_target_is_dangerous() {
        assert_eq!(danger("rm", &["/"]), CommandRisk::Dangerous);
        assert_eq!(danger("rm", &["*"]), CommandRisk::Dangerous);
    }

    #[test]
    fn sudo_is_dangerous() {
        assert_eq!(danger("sudo", &["apt", "update"]), CommandRisk::Dangerous);
        assert_eq!(danger("/usr/bin/sudo", &["ls"]), CommandRisk::Dangerous);
    }

    #[test]
    fn dd_and_mkfs_are_dangerous() {
        assert_eq!(
            danger("dd", &["if=/dev/zero", "of=/dev/sda"]),
            CommandRisk::Dangerous
        );
        assert_eq!(danger("mkfs.ext4", &["/dev/sda1"]), CommandRisk::Dangerous);
        assert_eq!(danger("mkfs", &["/dev/sda1"]), CommandRisk::Dangerous);
    }

    #[test]
    fn danger_floor_only_matches_catastrophic_forms() {
        assert!(floor("rm", &["-rf", "/"]));
        assert!(!floor("rm", &["-rf", "/tmp/project"]));
        assert!(floor("mkfs.ext4", &["/dev/sda1"]));
        assert!(floor("dd", &["if=image", "of=/dev/sda"]));
        assert!(!floor("dd", &["if=image", "of=local.img"]));
        assert!(floor("sh", &["-c", "echo ok && rm -rf /"]));
        assert!(floor("bash", &["-c", "bash -c 'rm -rf /'"]));
    }

    #[test]
    fn shutdown_family_is_dangerous() {
        for prog in ["shutdown", "reboot", "halt", "poweroff"] {
            assert_eq!(danger(prog, &[]), CommandRisk::Dangerous, "{prog}");
        }
    }

    #[test]
    fn chmod_recursive_is_dangerous() {
        assert_eq!(danger("chmod", &["-R", "777", "."]), CommandRisk::Dangerous);
        assert_eq!(
            danger("chown", &["-R", "root", "/"]),
            CommandRisk::Dangerous
        );
        assert_eq!(danger("chmod", &["+x", "file"]), CommandRisk::Safe);
    }

    #[test]
    fn git_push_force_is_dangerous() {
        assert_eq!(
            danger("git", &["push", "--force", "origin"]),
            CommandRisk::Dangerous
        );
        assert_eq!(danger("git", &["push", "-f"]), CommandRisk::Dangerous);
        // force-with-lease 更安全，且不是精确的 --force。
        assert_eq!(
            danger("git", &["push", "--force-with-lease"]),
            CommandRisk::Safe
        );
        assert_eq!(
            danger("git", &["push", "origin", "main"]),
            CommandRisk::Safe
        );
    }

    #[test]
    fn git_branch_delete_is_dangerous() {
        assert_eq!(
            danger("git", &["branch", "-D", "main"]),
            CommandRisk::Dangerous
        );
        assert_eq!(
            danger("git", &["branch", "-d", "feature"]),
            CommandRisk::Dangerous
        );
        assert_eq!(danger("git", &["branch", "list"]), CommandRisk::Safe);
    }

    #[test]
    fn benign_commands_are_safe() {
        assert_eq!(danger("ls", &[]), CommandRisk::Safe);
        assert_eq!(danger("echo", &["hello"]), CommandRisk::Safe);
        assert_eq!(danger("cargo", &["build"]), CommandRisk::Safe);
    }

    #[test]
    fn shell_script_with_separator_is_split() {
        assert_eq!(
            danger("sh", &["-c", "echo hi && rm -rf /"]),
            CommandRisk::Dangerous
        );
        assert_eq!(danger("sh", &["-c", "echo hi"]), CommandRisk::Safe);
        assert_eq!(danger("sh", &["-c", "sudo ls"]), CommandRisk::Dangerous);
    }

    #[test]
    fn nested_shell_invocation_is_caught() {
        // token 解析看不到内层命令，靠兜底正则捕获。
        assert_eq!(
            danger("bash", &["-c", "bash -c 'rm -rf /'"]),
            CommandRisk::Dangerous
        );
    }

    #[test]
    fn command_string_with_spaces_is_split() {
        assert_eq!(danger("rm -rf /", &[]), CommandRisk::Dangerous);
        assert_eq!(danger("echo hello", &[]), CommandRisk::Safe);
    }

    #[test]
    fn dangerous_redirect_to_system_path() {
        assert_eq!(
            danger("sh", &["-c", "echo x > /etc/passwd"]),
            CommandRisk::Dangerous
        );
        assert_eq!(
            danger("sh", &["-c", "cat y > /dev/sda"]),
            CommandRisk::Dangerous
        );
        // 重定向到工作区内文件名视为安全。
        assert_eq!(danger("sh", &["-c", "echo z > out.txt"]), CommandRisk::Safe);
    }

    #[test]
    fn rmdir_is_not_rm() {
        assert_eq!(danger("rmdir", &["emptydir"]), CommandRisk::Safe);
    }
}
