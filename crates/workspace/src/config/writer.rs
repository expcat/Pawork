//! Global 层配置写盘（SET-2 / SET-6a / SET-6c / SET-6 终端页）。
//!
//! 读取现有 Global 层文件（缺失视为空配置），以 TOML Table 保留全部未知
//! 字段，仅改目标键，最后经同目录临时文件 + rename 原子写回。六层合并
//! 语义、schema 与加载路径均不变。

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::config::error::{ConfigError, ConfigParseError};

/// 同进程内的临时文件唯一后缀：仅按 pid 区分的临时名会让同进程并发写
/// 互相覆盖临时文件（GUI 快速双击即可触发）。
static TEMP_SUFFIX: AtomicU64 = AtomicU64::new(0);

/// 同进程跨键写串行化：四个公开入口都经 [`rmw_global_config`] 持此锁，
/// 包住 read_table → 改 → atomic_write_table 全程，避免交错读写造成
/// lost update。跨进程仍靠 atomic_write_table 的 rename 原子性。
static CONFIG_WRITE_LOCK: Mutex<()> = Mutex::new(());

fn read_table(path: &Path) -> Result<toml::Table, ConfigError> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(ConfigError::Io {
                path: path.to_path_buf(),
                source: Box::new(error),
            })
        }
    };
    toml::from_str(&content).map_err(|source| {
        ConfigError::Parse(ConfigParseError::Toml {
            path: path.to_path_buf(),
            source: Box::new(source),
        })
    })
}

fn atomic_write_table(path: &Path, table: &toml::Table) -> Result<(), ConfigError> {
    let serialized = toml::to_string(table).map_err(|source| ConfigError::Write {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            path: parent.to_path_buf(),
            source: Box::new(source),
        })?;
    }
    let temp = path.with_file_name(format!(
        "{}.{}.{}.tmp",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "config.toml".into()),
        std::process::id(),
        TEMP_SUFFIX.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&temp, serialized).map_err(|source| ConfigError::Io {
        path: temp.clone(),
        source: Box::new(source),
    })?;
    std::fs::rename(&temp, path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;
    Ok(())
}

/// Global 配置的唯一 read-modify-write 内核。
///
/// `mutate` 返回 `(should_write, value)`：`should_write == false` 时不调用
/// [`atomic_write_table`]（`write_mcp_server_remove` 在键缺失时走此路径）。
fn rmw_global_config<T>(
    path: &Path,
    mutate: impl FnOnce(&mut toml::Table) -> Result<(bool, T), ConfigError>,
) -> Result<T, ConfigError> {
    let _guard = CONFIG_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut table = read_table(path)?;
    let (should_write, value) = mutate(&mut table)?;
    if should_write {
        atomic_write_table(path, &table)?;
    }
    Ok(value)
}

/// 将 default_provider/default_model 原子写入指定（Global 层）配置文件。
///
/// 幂等：重复写入同一对值为最终覆盖语义。文件不存在时创建（含父目录）。
pub fn write_default_model_pair(
    path: &Path,
    provider_id: &str,
    model_id: &str,
) -> Result<(), ConfigError> {
    rmw_global_config(path, |table| {
        table.insert(
            "default_provider".into(),
            toml::Value::String(provider_id.to_string()),
        );
        table.insert(
            "default_model".into(),
            toml::Value::String(model_id.to_string()),
        );
        Ok((true, ()))
    })
}

/// 将 `proxy_url` 原子写入指定（Global 层）配置文件（SET-6a，ADR-047 D2）。
///
/// `Some` 覆盖该键；`None` 移除该键。其余未知字段原样保留。文件不存在时
/// 视为空配置（`Some` 时创建；`None` 时写回无该键的空表）。
pub fn write_proxy_url(path: &Path, proxy_url: Option<&str>) -> Result<(), ConfigError> {
    rmw_global_config(path, |table| {
        match proxy_url {
            Some(url) => {
                table.insert("proxy_url".into(), toml::Value::String(url.to_string()));
            }
            None => {
                table.remove("proxy_url");
            }
        }
        Ok((true, ()))
    })
}

/// 将指定 provider 的代理开关原子写入（Global 层）配置文件的
/// `[[providers]]` 条目（ADR-052 SET-6h）。
///
/// 命中同 `id` 条目则更新其 `use_proxy`；无该条目则追加仅含
/// `id` + `use_proxy` 的新条目。其余条目与未知字段原样保留。
/// `providers` 已存在但非数组时（schema 加载同样不容忍）fail-closed 报错。
pub fn write_provider_use_proxy(
    path: &Path,
    provider_id: &str,
    use_proxy: bool,
) -> Result<(), ConfigError> {
    rmw_global_config(path, |table| {
        let providers = match table
            .entry("providers")
            .or_insert_with(|| toml::Value::Array(Vec::new()))
        {
            toml::Value::Array(array) => array,
            _ => {
                return Err(ConfigError::Io {
                    path: path.to_path_buf(),
                    source: Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "providers is not an array",
                    )),
                })
            }
        };
        let entry = providers.iter_mut().find(|item| {
            item.as_table()
                .and_then(|table| table.get("id"))
                .and_then(toml::Value::as_str)
                .is_some_and(|id| id == provider_id)
        });
        match entry {
            Some(toml::Value::Table(entry)) => {
                entry.insert("use_proxy".into(), toml::Value::Boolean(use_proxy));
            }
            _ => {
                let mut created = toml::Table::new();
                created.insert("id".into(), toml::Value::String(provider_id.to_string()));
                created.insert("use_proxy".into(), toml::Value::Boolean(use_proxy));
                providers.push(toml::Value::Table(created));
            }
        }
        Ok((true, ()))
    })
}

/// 从指定（Global 层）配置文件原子移除 `mcp.servers.<name>`
/// （SET-6c，ADR-049 D2）。
///
/// 其余未知字段（含其它 server 条目）原样保留。键不存在时不写盘并返回
/// `Ok(false)`（fail-closed 保旧，由调用方如实回执）；存在且移除成功返回
/// `Ok(true)`。
pub fn write_mcp_server_remove(path: &Path, name: &str) -> Result<bool, ConfigError> {
    rmw_global_config(path, |table| {
        let removed = table
            .get_mut("mcp")
            .and_then(|mcp| mcp.as_table_mut())
            .and_then(|mcp| mcp.get_mut("servers"))
            .and_then(|servers| servers.as_table_mut())
            .is_some_and(|servers| servers.remove(name).is_some());
        Ok((removed, removed))
    })
}

/// 将终端默认设置全态原子写入指定（Global 层）配置文件的 `[terminal]` 段
/// （SET-6 终端页 / ADR-050 D1、D3）。
///
/// 全态写：`shell` 为 `None` 时移除该键（回平台默认），columns/rows 总是
/// 写入。`[terminal]` 段内其余未知字段与文件顶层未知字段原样保留；既有
/// `terminal` 键为非 table 的旧值本就无法通过 schema 加载，重建为空表。
pub fn write_terminal_settings(
    path: &Path,
    shell: Option<&str>,
    columns: u16,
    rows: u16,
) -> Result<(), ConfigError> {
    rmw_global_config(path, |table| {
        let mut terminal = match table.remove("terminal") {
            Some(toml::Value::Table(existing)) => existing,
            Some(_) | None => toml::Table::new(),
        };
        match shell {
            Some(shell) => {
                terminal.insert("shell".into(), toml::Value::String(shell.to_string()));
            }
            None => {
                terminal.remove("shell");
            }
        }
        terminal.insert("columns".into(), toml::Value::Integer(i64::from(columns)));
        terminal.insert("rows".into(), toml::Value::Integer(i64::from(rows)));
        table.insert("terminal".into(), toml::Value::Table(terminal));
        Ok((true, ()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pawork-config-writer-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create dir");
        dir.join("config.toml")
    }

    #[test]
    fn writes_pair_and_preserves_unknown_fields() {
        let path = temp_path("preserve");
        std::fs::write(
            &path,
            "trust_workspaces = true\n[extra_section]\nkey = \"v\"\n",
        )
        .expect("seed config");
        write_default_model_pair(&path, "glm-coding", "glm-5.2").expect("write");
        let content = std::fs::read_to_string(&path).expect("read back");
        assert!(content.contains("default_provider = \"glm-coding\""));
        assert!(content.contains("default_model = \"glm-5.2\""));
        assert!(content.contains("trust_workspaces = true"));
        assert!(content.contains("[extra_section]"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn creates_missing_file_and_rewrites_atomically() {
        let path = temp_path("create");
        write_default_model_pair(&path, "deepseek", "deepseek-chat").expect("create write");
        write_default_model_pair(&path, "glm-coding", "glm-5.2").expect("overwrite");
        let table: toml::Table =
            toml::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
        assert_eq!(
            table.get("default_provider").and_then(|v| v.as_str()),
            Some("glm-coding")
        );
        assert_eq!(
            table.get("default_model").and_then(|v| v.as_str()),
            Some("glm-5.2")
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn writes_proxy_url_and_preserves_unknown_fields() {
        let path = temp_path("proxy-set");
        std::fs::write(
            &path,
            "trust_workspaces = true\n[extra_section]\nkey = \"v\"\n",
        )
        .expect("seed config");
        write_proxy_url(&path, Some("http://127.0.0.1:7890")).expect("write");
        let table: toml::Table =
            toml::from_str(&std::fs::read_to_string(&path).expect("read back")).expect("parse");
        assert_eq!(
            table.get("proxy_url").and_then(|v| v.as_str()),
            Some("http://127.0.0.1:7890")
        );
        assert_eq!(
            table.get("trust_workspaces").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            table
                .get("extra_section")
                .and_then(|v| v.as_table())
                .and_then(|section| section.get("key"))
                .and_then(|v| v.as_str()),
            Some("v")
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn clears_proxy_url_and_leaves_other_fields() {
        let path = temp_path("proxy-clear");
        std::fs::write(
            &path,
            "proxy_url = \"http://127.0.0.1:7890\"\ntrust_workspaces = true\n[extra_section]\nkey = \"v\"\n",
        )
        .expect("seed config");
        write_proxy_url(&path, None).expect("clear");
        let content = std::fs::read_to_string(&path).expect("read back");
        let table: toml::Table = toml::from_str(&content).expect("parse");
        assert!(table.get("proxy_url").is_none());
        assert!(!content.contains("proxy_url"));
        assert_eq!(
            table.get("trust_workspaces").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            table
                .get("extra_section")
                .and_then(|v| v.as_table())
                .and_then(|section| section.get("key"))
                .and_then(|v| v.as_str()),
            Some("v")
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn writes_provider_use_proxy_and_preserves_other_entries() {
        let path = temp_path("provider-use-proxy");
        std::fs::write(
            &path,
            "proxy_url = \"http://127.0.0.1:7890\"\n[[providers]]\nid = \"glm-coding\"\nbase_url = \"https://example.test/v1\"\n[[providers]]\nid = \"other\"\n",
        )
        .expect("seed config");
        // 命中既有 id：只加 use_proxy，不动 base_url 与其它条目。
        write_provider_use_proxy(&path, "glm-coding", false).expect("write existing");
        let table: toml::Table =
            toml::from_str(&std::fs::read_to_string(&path).expect("read back")).expect("parse");
        let providers = table
            .get("providers")
            .and_then(|v| v.as_array())
            .expect("providers");
        assert_eq!(providers.len(), 2);
        let first = providers[0].as_table().expect("entry table");
        assert_eq!(
            first.get("base_url").and_then(|v| v.as_str()),
            Some("https://example.test/v1")
        );
        assert_eq!(
            first.get("use_proxy").and_then(|v| v.as_bool()),
            Some(false)
        );
        assert!(providers[1]
            .as_table()
            .expect("entry")
            .get("use_proxy")
            .is_none());
        // 无该 id：追加新条目。
        write_provider_use_proxy(&path, "new-provider", true).expect("write missing");
        let table: toml::Table =
            toml::from_str(&std::fs::read_to_string(&path).expect("read back")).expect("parse");
        let providers = table
            .get("providers")
            .and_then(|v| v.as_array())
            .expect("providers");
        assert_eq!(providers.len(), 3);
        let created = providers[2].as_table().expect("created entry");
        assert_eq!(
            created.get("id").and_then(|v| v.as_str()),
            Some("new-provider")
        );
        assert_eq!(
            created.get("use_proxy").and_then(|v| v.as_bool()),
            Some(true)
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_mcp_server_remove_drops_named_server_and_skips_missing() {
        let path = temp_path("mcp-remove");
        let seeded = [
            "trust_workspaces = true",
            "",
            "[mcp.servers.demo]",
            r#"transport = { kind = "http", url = "https://mcp.example.com/mcp" }"#,
            "",
            "[mcp.servers.keep]",
            r#"transport = { kind = "http", url = "https://keep.example.com/mcp" }"#,
            "",
        ]
        .join("\n");
        std::fs::write(&path, seeded).expect("seed config");
        assert!(write_mcp_server_remove(&path, "demo").expect("remove demo"));
        let content = std::fs::read_to_string(&path).expect("read back");
        let table: toml::Table = toml::from_str(&content).expect("parse");
        assert_eq!(
            table.get("trust_workspaces").and_then(|v| v.as_bool()),
            Some(true)
        );
        let servers = table
            .get("mcp")
            .and_then(|mcp| mcp.as_table())
            .and_then(|mcp| mcp.get("servers"))
            .and_then(|servers| servers.as_table())
            .expect("servers table");
        assert!(servers.get("demo").is_none());
        assert!(servers.get("keep").is_some());
        assert!(!write_mcp_server_remove(&path, "demo").expect("missing is no-op"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("unchanged after missing"),
            content
        );
        std::fs::remove_file(&path).ok();
    }
}
