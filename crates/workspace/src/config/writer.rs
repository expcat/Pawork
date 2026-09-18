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
            });
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

/// ADR-053：只写 Global 审批默认，共用串行 RMW。
pub fn write_approval_mode(
    path: &Path,
    mode: pawork_policy::ApprovalMode,
) -> Result<(), ConfigError> {
    rmw_global_config(path, |table| {
        let value = toml::Value::try_from(mode).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;
        table.insert("approval_mode".into(), value);
        Ok((true, ()))
    })
}

/// ADR-053：根路径由 Host 解析；一个项目的选择不得覆盖其他项目。
pub fn write_workspace_trust(path: &Path, root: &str, trusted: bool) -> Result<(), ConfigError> {
    rmw_global_config(path, |table| {
        let entries = table
            .entry("workspace_trust")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let entries = entries.as_table_mut().ok_or_else(|| ConfigError::Io {
            path: path.to_path_buf(),
            source: Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "workspace_trust is not a table",
            )),
        })?;
        entries.insert(root.into(), toml::Value::Boolean(trusted));
        Ok((true, ()))
    })
}

/// 将 default_provider/default_model 原子写入指定（Global 层）配置文件。
///
/// 幂等：重复写入同一对值为最终覆盖语义。文件不存在时创建（含父目录）。
pub fn write_default_model_pair(
    path: &Path,
    provider_id: &str,
    model_id: &str,
) -> Result<(), ConfigError> {
    write_model_pair(
        path,
        "default_provider",
        "default_model",
        Some((provider_id, model_id)),
    )
}

/// 将 naming_provider/naming_model 原子写入指定（Global 层）配置文件
/// （ADR-054 D4：与 default 对相同的写入口径，凭证仍只进 auth backend）。
///
/// 幂等：重复写入同一对值为最终覆盖语义。文件不存在时创建（含父目录）。
pub fn write_naming_model_pair(
    path: &Path,
    provider_id: &str,
    model_id: &str,
) -> Result<(), ConfigError> {
    write_model_pair(
        path,
        "naming_provider",
        "naming_model",
        Some((provider_id, model_id)),
    )
}

/// 将 provider/model 键对原子写入指定（Global 层）配置文件的通用入口
/// （ADR-055 D5：conversation/naming/vision/search 四角色同口径）。
///
/// `Some` 写两键（最终覆盖语义，文件不存在时创建含父目录）；`None` 移除
/// 已存在的键（半配对只移除存在的那一半；两键都不存在时不写盘返回
/// `Ok`）。其余未知字段原样保留。
pub fn write_model_pair(
    path: &Path,
    provider_key: &str,
    model_key: &str,
    pair: Option<(&str, &str)>,
) -> Result<(), ConfigError> {
    rmw_global_config(path, |table| match pair {
        Some((provider_id, model_id)) => {
            table.insert(
                provider_key.to_string(),
                toml::Value::String(provider_id.to_string()),
            );
            table.insert(
                model_key.to_string(),
                toml::Value::String(model_id.to_string()),
            );
            Ok((true, ()))
        }
        None => {
            let removed_provider = table.remove(provider_key).is_some();
            let removed_model = table.remove(model_key).is_some();
            Ok((removed_provider || removed_model, ()))
        }
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
                });
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

/// 将指定 provider 的禁用模型 denylist 原子写入（Global 层）配置文件的
/// `[[providers]]` 条目（ADR-055 D1）。
///
/// 命中同 `id` 条目：`disabled` 非空则覆盖该数组，空则移除该键（键本就
/// 缺失时不写盘）；无该条目：非空则追加仅含 `id` + `disabled_models` 的
/// 新条目，空则不写盘返回 `Ok`。其余条目与未知字段原样保留；`providers`
/// 已存在但非数组时 fail-closed 报错（同 [`write_provider_use_proxy`]）。
pub fn write_provider_disabled_models(
    path: &Path,
    provider_id: &str,
    disabled: &[String],
) -> Result<(), ConfigError> {
    write_provider_model_preferences(path, provider_id, disabled, &[])
}

/// 同一次原子写保存禁用集并清除命中的角色键对；失败时全部保旧。
/// `clear_pairs` 由 Host 按持久化配置判定，不接受运行时默认覆盖值。
pub fn write_provider_model_preferences(
    path: &Path,
    provider_id: &str,
    disabled: &[String],
    clear_pairs: &[(&str, &str)],
) -> Result<(), ConfigError> {
    rmw_global_config(path, |table| {
        let mut cleared = false;
        for (provider_key, model_key) in clear_pairs {
            cleared |= table.remove(*provider_key).is_some();
            cleared |= table.remove(*model_key).is_some();
        }
        let disabled_value = || {
            toml::Value::try_from(disabled).map_err(|source| ConfigError::Write {
                path: path.to_path_buf(),
                source: Box::new(source),
            })
        };
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
                });
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
                if disabled.is_empty() {
                    let removed = entry.remove("disabled_models").is_some();
                    Ok((removed || cleared, ()))
                } else {
                    entry.insert("disabled_models".into(), disabled_value()?);
                    Ok((true, ()))
                }
            }
            _ => {
                if disabled.is_empty() {
                    return Ok((cleared, ()));
                }
                let mut created = toml::Table::new();
                created.insert("id".into(), toml::Value::String(provider_id.to_string()));
                created.insert("disabled_models".into(), disabled_value()?);
                providers.push(toml::Value::Table(created));
                Ok((true, ()))
            }
        }
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

/// Atomically write Global `[subagents]`, preserving unknown fields.
///
/// Full-state replace of the known keys; a non-table `subagents` value is
/// rebuilt as an empty table. Model rows keep unknown fields when the same
/// `provider_id` + `model_id` pair already exists.
pub fn write_subagent_settings(
    path: &Path,
    settings: &crate::config::SubagentConfig,
) -> Result<(), ConfigError> {
    rmw_global_config(path, |table| {
        let mut subagents = match table.remove("subagents") {
            Some(toml::Value::Table(existing)) => existing,
            Some(_) | None => toml::Table::new(),
        };
        subagents.insert("enabled".into(), toml::Value::Boolean(settings.enabled));
        subagents.insert(
            "max_concurrent".into(),
            toml::Value::Integer(i64::from(settings.max_concurrent)),
        );
        let models = match subagents.remove("models") {
            Some(toml::Value::Array(existing)) => existing,
            Some(_) | None => Vec::new(),
        };
        let mut rewritten = Vec::with_capacity(settings.models.len());
        for model in &settings.models {
            let mut entry = models
                .iter()
                .find(|item| {
                    item.as_table().is_some_and(|table| {
                        table.get("provider_id").and_then(toml::Value::as_str)
                            == Some(model.provider_id.as_str())
                            && table.get("model_id").and_then(toml::Value::as_str)
                                == Some(model.model_id.as_str())
                    })
                })
                .and_then(|item| item.as_table().cloned())
                .unwrap_or_default();
            entry.insert(
                "provider_id".into(),
                toml::Value::String(model.provider_id.clone()),
            );
            entry.insert(
                "model_id".into(),
                toml::Value::String(model.model_id.clone()),
            );
            entry.insert(
                "allow_spawn".into(),
                toml::Value::Boolean(model.allow_spawn),
            );
            entry.insert(
                "allow_as_subagent".into(),
                toml::Value::Boolean(model.allow_as_subagent),
            );
            let permissions = model
                .permissions
                .iter()
                .cloned()
                .map(toml::Value::String)
                .collect();
            entry.insert("permissions".into(), toml::Value::Array(permissions));
            // ADR-063：None / 空数组清除既有键，保持盘上配置最小。
            match &model.default_effort {
                Some(effort) => {
                    entry.insert(
                        "default_effort".into(),
                        toml::Value::String(effort.clone()),
                    );
                }
                None => {
                    entry.remove("default_effort");
                }
            }
            if model.allowed_efforts.is_empty() {
                entry.remove("allowed_efforts");
            } else {
                let efforts = model
                    .allowed_efforts
                    .iter()
                    .cloned()
                    .map(toml::Value::String)
                    .collect();
                entry.insert("allowed_efforts".into(), toml::Value::Array(efforts));
            }
            rewritten.push(toml::Value::Table(entry));
        }
        subagents.insert("models".into(), toml::Value::Array(rewritten));
        table.insert("subagents".into(), toml::Value::Table(subagents));
        Ok((true, ()))
    })
}

/// ADR-063：写单模型的推理强度偏好（`[reasoning]` 的 `[[reasoning.models]]`
/// 条目，Global-only）。
///
/// 全态语义：`default_effort` / `supported_efforts` 为 None 即清除该键；
/// 两者皆 None 时移除整条目。条目内未知键保留（与 `write_subagent_settings`
/// 同先例）。
pub fn write_model_reasoning(
    path: &Path,
    provider_id: &str,
    model_id: &str,
    default_effort: Option<&str>,
    supported_efforts: Option<&[String]>,
) -> Result<(), ConfigError> {
    rmw_global_config(path, |table| {
        let mut reasoning = match table.remove("reasoning") {
            Some(toml::Value::Table(existing)) => existing,
            Some(_) | None => toml::Table::new(),
        };
        let models = match reasoning.remove("models") {
            Some(toml::Value::Array(existing)) => existing,
            Some(_) | None => Vec::new(),
        };
        let mut rewritten = Vec::with_capacity(models.len() + 1);
        let mut found = false;
        for item in models {
            let is_target = item.as_table().is_some_and(|entry| {
                entry.get("provider_id").and_then(toml::Value::as_str) == Some(provider_id)
                    && entry.get("model_id").and_then(toml::Value::as_str) == Some(model_id)
            });
            if !is_target {
                rewritten.push(item);
                continue;
            }
            found = true;
            if let Some(entry) =
                reasoning_entry(item.as_table().cloned().unwrap_or_default(), provider_id, model_id, default_effort, supported_efforts)
            {
                rewritten.push(toml::Value::Table(entry));
            }
        }
        if !found {
            if let Some(entry) = reasoning_entry(
                toml::Table::new(),
                provider_id,
                model_id,
                default_effort,
                supported_efforts,
            ) {
                rewritten.push(toml::Value::Table(entry));
            }
        }
        reasoning.insert("models".into(), toml::Value::Array(rewritten));
        table.insert("reasoning".into(), toml::Value::Table(reasoning));
        Ok((true, ()))
    })
}

/// 构造单条 reasoning 条目；两键皆 None 时返回 None（调用方移除该条目）。
fn reasoning_entry(
    mut entry: toml::Table,
    provider_id: &str,
    model_id: &str,
    default_effort: Option<&str>,
    supported_efforts: Option<&[String]>,
) -> Option<toml::Table> {
    if default_effort.is_none() && supported_efforts.is_none() {
        return None;
    }
    entry.insert(
        "provider_id".into(),
        toml::Value::String(provider_id.to_string()),
    );
    entry.insert(
        "model_id".into(),
        toml::Value::String(model_id.to_string()),
    );
    match default_effort {
        Some(effort) => {
            entry.insert(
                "default_effort".into(),
                toml::Value::String(effort.to_string()),
            );
        }
        None => {
            entry.remove("default_effort");
        }
    }
    match supported_efforts {
        Some(efforts) => {
            entry.insert(
                "supported_efforts".into(),
                toml::Value::Array(
                    efforts
                        .iter()
                        .map(|effort| toml::Value::String(effort.clone()))
                        .collect(),
                ),
            );
        }
        None => {
            entry.remove("supported_efforts");
        }
    }
    Some(entry)
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
    fn writes_naming_pair_roundtrip_and_keeps_default_pair() {
        let path = temp_path("naming-roundtrip");
        write_default_model_pair(&path, "glm-coding", "glm-5.2").expect("seed default pair");
        write_naming_model_pair(&path, "opencode-go", "glm-5.3-flash").expect("write naming");
        let content = std::fs::read_to_string(&path).expect("read back");
        let config: crate::config::schema::PaworkConfig =
            toml::from_str(&content).expect("schema parse");
        assert_eq!(
            config.naming_provider.as_deref(),
            Some("opencode-go"),
            "{content}"
        );
        assert_eq!(config.naming_model.as_deref(), Some("glm-5.3-flash"));
        assert_eq!(config.default_provider.as_deref(), Some("glm-coding"));
        assert_eq!(config.default_model.as_deref(), Some("glm-5.2"));
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

    #[test]
    fn write_model_pair_set_clear_and_schema_roundtrip() {
        let path = temp_path("model-pair");
        std::fs::write(
            &path,
            "trust_workspaces = true\n[extra_section]\nkey = \"v\"\n",
        )
        .expect("seed config");
        write_model_pair(
            &path,
            "vision_provider",
            "vision_model",
            Some(("glm-coding", "glm-4.6v")),
        )
        .expect("set vision pair");
        let content = std::fs::read_to_string(&path).expect("read back");
        let config: crate::config::schema::PaworkConfig =
            toml::from_str(&content).expect("schema roundtrip");
        assert_eq!(config.vision_provider.as_deref(), Some("glm-coding"));
        assert_eq!(config.vision_model.as_deref(), Some("glm-4.6v"));
        assert_eq!(config.trust_workspaces, Some(true));
        assert_eq!(
            config
                .extra
                .get("extra_section")
                .and_then(|v| v.get("key"))
                .and_then(|v| v.as_str()),
            Some("v")
        );

        write_model_pair(&path, "vision_provider", "vision_model", None).expect("clear pair");
        let content = std::fs::read_to_string(&path).expect("read back");
        assert!(!content.contains("vision_provider"));
        assert!(!content.contains("vision_model"));
        assert!(content.contains("trust_workspaces = true"));
        assert!(content.contains("[extra_section]"));

        // 两键都不存在时清除为 no-op：不写盘，内容不变。
        write_model_pair(&path, "vision_provider", "vision_model", None).expect("clear again");
        assert_eq!(std::fs::read_to_string(&path).expect("unchanged"), content);

        // 半配对清除：只移除存在的那一半。
        std::fs::write(&path, "search_provider = \"opencode-go\"\n").expect("seed half pair");
        write_model_pair(&path, "search_provider", "search_model", None).expect("clear half");
        let content = std::fs::read_to_string(&path).expect("read back");
        assert!(!content.contains("search_provider"));
        assert!(!content.contains("search_model"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_provider_disabled_models_set_clear_append_and_noop() {
        let path = temp_path("provider-disabled-models");
        std::fs::write(
            &path,
            "proxy_url = \"http://127.0.0.1:7890\"\n[[providers]]\nid = \"glm-coding\"\nbase_url = \"https://example.test/v1\"\n[[providers]]\nid = \"other\"\n",
        )
        .expect("seed config");
        // 命中既有 id：写数组，保留 base_url 与其它条目。
        write_provider_disabled_models(
            &path,
            "glm-coding",
            &["glm-5.2".to_string(), "glm-5.3-flash".to_string()],
        )
        .expect("set existing");
        let table: toml::Table =
            toml::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
        let providers = table
            .get("providers")
            .and_then(|v| v.as_array())
            .expect("providers");
        assert_eq!(providers.len(), 2);
        let first = providers[0].as_table().expect("entry");
        assert_eq!(
            first.get("base_url").and_then(|v| v.as_str()),
            Some("https://example.test/v1")
        );
        assert_eq!(
            first.get("disabled_models").and_then(|v| v.as_array()),
            Some(&vec![
                toml::Value::String("glm-5.2".into()),
                toml::Value::String("glm-5.3-flash".into())
            ])
        );
        assert!(providers[1]
            .as_table()
            .expect("entry")
            .get("disabled_models")
            .is_none());
        assert_eq!(
            table.get("proxy_url").and_then(|v| v.as_str()),
            Some("http://127.0.0.1:7890")
        );

        // 空切片：移除该键；再清一次为 no-op。
        write_provider_disabled_models(&path, "glm-coding", &[]).expect("clear existing");
        let table: toml::Table =
            toml::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
        let first = table
            .get("providers")
            .and_then(|v| v.as_array())
            .expect("providers")[0]
            .as_table()
            .expect("entry");
        assert!(first.get("disabled_models").is_none());
        assert_eq!(
            first.get("base_url").and_then(|v| v.as_str()),
            Some("https://example.test/v1")
        );
        let cleared = std::fs::read_to_string(&path).expect("read cleared");
        write_provider_disabled_models(&path, "glm-coding", &[]).expect("clear noop");
        assert_eq!(std::fs::read_to_string(&path).expect("unchanged"), cleared);

        // 无该条目且非空：追加 id + disabled_models 新条目。
        write_provider_disabled_models(&path, "new-provider", &["m1".to_string()])
            .expect("append missing");
        let table: toml::Table =
            toml::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
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
            created.get("disabled_models").and_then(|v| v.as_array()),
            Some(&vec![toml::Value::String("m1".into())])
        );

        // 无该条目且空：不写盘返回 Ok。
        let before = std::fs::read_to_string(&path).expect("read before noop");
        write_provider_disabled_models(&path, "ghost", &[]).expect("missing + empty is no-op");
        assert_eq!(std::fs::read_to_string(&path).expect("unchanged"), before);

        // 文件不存在 + 空切片：不创建文件。
        let missing = temp_path("provider-disabled-missing");
        let _ = std::fs::remove_file(&missing);
        write_provider_disabled_models(&missing, "ghost", &[]).expect("missing file is no-op");
        assert!(!missing.exists());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_model_reasoning_upserts_and_removes_entry() {
        let path = temp_path("model-reasoning");
        std::fs::write(&path, "trust_workspaces = true\n").expect("seed config");
        write_model_reasoning(
            &path,
            "glm-coding",
            "glm-5.3-flash",
            Some("high"),
            Some(&["low".to_string(), "medium".to_string(), "high".to_string()]),
        )
        .expect("write reasoning");
        let table: toml::Table =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let entry = &table["reasoning"]["models"][0];
        assert_eq!(entry["provider_id"].as_str(), Some("glm-coding"));
        assert_eq!(entry["default_effort"].as_str(), Some("high"));
        assert_eq!(
            entry["supported_efforts"]
                .as_array()
                .map(|efforts| efforts.len()),
            Some(3)
        );
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("trust_workspaces = true"));

        // 全态清除：两键皆 None 移除整条目，保留其他模型条目。
        write_model_reasoning(&path, "deepseek", "deepseek-chat", Some("low"), None)
            .expect("write second entry");
        write_model_reasoning(&path, "glm-coding", "glm-5.3-flash", None, None)
            .expect("clear first entry");
        let table: toml::Table =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let models = table["reasoning"]["models"].as_array().expect("models");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["provider_id"].as_str(), Some("deepseek"));
        assert!(models[0].get("supported_efforts").is_none());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_provider_disabled_models_fails_closed_on_non_array_providers() {
        let path = temp_path("provider-disabled-non-array");
        std::fs::write(&path, "providers = 1\n").expect("seed config");
        assert!(write_provider_disabled_models(&path, "glm-coding", &["m1".to_string()]).is_err());
        // 空切片同样 fail-closed：providers 形态非法时不静默放行。
        assert!(write_provider_disabled_models(&path, "glm-coding", &[]).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).expect("unchanged"),
            "providers = 1\n"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn model_preferences_clear_roles_atomically_and_preserve_on_error() {
        let path = temp_path("model-preferences");
        let original =
            "default_provider = \"glm-coding\"\ndefault_model = \"glm-5.2\"\ncustom = 7\n";
        std::fs::write(&path, original).expect("seed config");
        write_provider_model_preferences(
            &path,
            "glm-coding",
            &["glm-5.2".into()],
            &[("default_provider", "default_model")],
        )
        .expect("save preferences");
        let table: toml::Table = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(!table.contains_key("default_provider"));
        assert!(!table.contains_key("default_model"));
        assert_eq!(table["custom"].as_integer(), Some(7));
        assert_eq!(
            table["providers"][0]["disabled_models"][0].as_str(),
            Some("glm-5.2")
        );

        let invalid = format!("{original}providers = 1\n");
        std::fs::write(&path, &invalid).unwrap();
        assert!(write_provider_model_preferences(
            &path,
            "glm-coding",
            &["glm-5.2".into()],
            &[("default_provider", "default_model")],
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_subagent_settings_preserves_unknown_fields() {
        let path = temp_path("subagents");
        std::fs::write(
            &path,
            "trust_workspaces = true\n[extra_section]\nkey = \"v\"\n[subagents]\nenabled = false\nkeep = 1\n",
        )
        .expect("seed config");
        let settings = crate::config::SubagentConfig {
            enabled: true,
            max_concurrent: 2,
            models: vec![crate::config::SubagentModelConfig {
                provider_id: "glm-coding".into(),
                model_id: "glm-5.3-flash".into(),
                allow_spawn: true,
                allow_as_subagent: false,
                permissions: vec!["read".into()],
                default_effort: None,
                allowed_efforts: vec![],
            }],
        };
        write_subagent_settings(&path, &settings).expect("write");
        let content = std::fs::read_to_string(&path).expect("read back");
        assert!(content.contains("trust_workspaces = true"));
        assert!(content.contains("[extra_section]"));
        let table: toml::Table = toml::from_str(&content).expect("parse");
        let subagents = table
            .get("subagents")
            .and_then(|v| v.as_table())
            .expect("subagents");
        assert_eq!(
            subagents.get("enabled").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            subagents.get("max_concurrent").and_then(|v| v.as_integer()),
            Some(2)
        );
        assert_eq!(subagents.get("keep").and_then(|v| v.as_integer()), Some(1));
        std::fs::remove_file(&path).ok();
    }
}
