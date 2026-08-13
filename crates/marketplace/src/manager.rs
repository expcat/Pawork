//! Marketplace 编排（P17-3）：发现 → 解析（semver 范围 / pin）→ 拉取与完整性校验
//! （身份 / bundle hash / hash pin / 签名）→ trust 与 team policy 闸门 → 经宿主注入
//! 接口事务化安装 / 更新 / 卸载。
//!
//! 事务语义：
//! - 注册经 plugin_package::install_package（skills → agents → hooks → mcp → lsp →
//!   monitors），任一失败反向补偿、整体回滚；
//! - 更新 / 卸载先移除旧资源（monitor 先 stop 再 unregister），任一失败同样补偿恢复；
//! - 全部成功后才持久化状态；持久化失败也补偿回滚（fail-closed）；
//! - Marketplace 绝不执行子资源：只把声明提交给宿主。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use plugin_package::{
    install_package, read_archive, DispatchPlan, DispatchSummary, PackageArchive,
    PackageDependency, PackageId,
};
use semver::{Version, VersionReq};

use crate::error::MarketplaceError;
use crate::host::{
    agent_key, compensate, hook_key, lsp_key, mcp_key, monitor_key, remove_plan, skill_key,
    RegisteringSink, UndoStep,
};
use crate::pin::{resolve_candidate, Pin};
use crate::policy::{PolicyInput, TeamPolicy};
use crate::signature::{content_digest_hex, Keyring};
use crate::source::{discover, Candidate, Discovery, SourceIo, SourceSpec};
use crate::store::{InstalledPackage, MarketplaceState, StateStore};
use crate::trust::{TrustConfig, TrustLevel};

/// 安装结果。
#[derive(Clone, Debug)]
pub struct InstallOutcome {
    pub id: PackageId,
    pub version: Version,
    /// 实际安装来源 source 名。
    pub source: String,
    /// canonical payload 的 blake3 hex（内容寻址身份）。
    pub digest_hex: String,
    /// 各子资源计数。
    pub summary: DispatchSummary,
}

/// 更新结果。
#[derive(Clone, Debug)]
pub struct UpdateOutcome {
    pub id: PackageId,
    pub from: Version,
    pub to: Version,
    pub source: String,
    pub digest_hex: String,
    /// false 表示解析版本与已装版本相同（验证过的 no-op）。
    pub switched: bool,
    pub summary: DispatchSummary,
}

/// 卸载结果。
#[derive(Clone, Debug)]
pub struct UninstallOutcome {
    pub id: PackageId,
    pub version: Version,
    /// 移除的子资源数量。
    pub removed: usize,
}

/// 拉取并通过完整性校验的待装归档。
struct Prepared {
    archive: PackageArchive,
    digest_hex: String,
    staging: PathBuf,
}

/// Marketplace：多 source 发现、pin 解析、签名 / trust / policy 闸门、事务化
/// install / update / uninstall。
///
/// 全部 source I/O 经注入的 [SourceIo]，状态持久化经注入的 [StateStore]，
/// 子资源注册经宿主注入的 [crate::host::ResourceHost]；本结构不直接触达网络，
/// 仅管理 staging 临时目录（失败路径 best-effort 清理）。
pub struct Marketplace {
    sources: Vec<SourceSpec>,
    io: Box<dyn SourceIo>,
    keyring: Keyring,
    trust: TrustConfig,
    policy: TeamPolicy,
    store: Box<dyn StateStore>,
    staging_root: PathBuf,
    staging_counter: AtomicU64,
}

impl Marketplace {
    pub fn new(
        sources: Vec<SourceSpec>,
        io: Box<dyn SourceIo>,
        store: Box<dyn StateStore>,
        staging_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            sources,
            io,
            keyring: Keyring::default(),
            trust: TrustConfig::default(),
            policy: TeamPolicy::default(),
            store,
            staging_root: staging_root.into(),
            staging_counter: AtomicU64::new(0),
        }
    }

    pub fn with_keyring(mut self, keyring: Keyring) -> Self {
        self.keyring = keyring;
        self
    }

    pub fn with_trust_config(mut self, trust: TrustConfig) -> Self {
        self.trust = trust;
        self
    }

    pub fn with_policy(mut self, policy: TeamPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn sources(&self) -> &[SourceSpec] {
        &self.sources
    }

    /// 聚合全部 source 索引（任一失败整体失败，fail-closed）。
    pub fn discover(&self) -> Result<Discovery, MarketplaceError> {
        discover(&self.sources, &*self.io)
    }

    /// 当前安装集快照。
    pub fn installed(&self) -> Result<BTreeMap<String, InstalledPackage>, MarketplaceError> {
        Ok(self.store.load()?.installed)
    }

    /// 设置 pin（持久化，revision +1）。
    pub fn set_pin(&mut self, id: &PackageId, pin: Pin) -> Result<(), MarketplaceError> {
        let mut state = self.store.load()?;
        state.pins.insert(id.as_str().to_string(), pin);
        state.revision += 1;
        self.store.save(&state)
    }

    /// 清除 pin（存在时持久化，revision +1）。
    pub fn clear_pin(&mut self, id: &PackageId) -> Result<(), MarketplaceError> {
        let mut state = self.store.load()?;
        if state.pins.remove(id.as_str()).is_some() {
            state.revision += 1;
            self.store.save(&state)?;
        }
        Ok(())
    }

    /// 安装：解析候选 → 拉取校验 → 签名 / policy / trust 闸门 → 依赖与冲突检查 →
    /// 事务化注册 → 持久化。任一步失败不留下已注册资源。
    pub fn install(
        &mut self,
        host: &mut dyn crate::host::ResourceHost,
        id: &PackageId,
        requirement: Option<&VersionReq>,
        user_approved: bool,
    ) -> Result<InstallOutcome, MarketplaceError> {
        let mut state = self.store.load()?;
        if state.installed.contains_key(id.as_str()) {
            return Err(MarketplaceError::AlreadyInstalled(id.as_str().to_string()));
        }
        let pin = state.pin(id).cloned();
        let (source, candidate) = self.resolve(id, requirement, pin.as_ref())?;
        let prepared = self.prepare(id, &source, &candidate, pin.as_ref())?;
        if let Err(error) = self.gates(id, &source, &candidate, &prepared, user_approved) {
            self.cleanup(&prepared.staging);
            return Err(error);
        }
        if let Err(error) = check_dependencies(id, &prepared.archive, &state) {
            self.cleanup(&prepared.staging);
            return Err(error);
        }
        let plan = DispatchPlan::from_archive(&prepared.archive);
        if let Err(error) = check_conflicts(&plan, &state, None) {
            self.cleanup(&prepared.staging);
            return Err(error);
        }
        if let Err(error) = check_identity_conflicts(&prepared.archive, &state, None) {
            self.cleanup(&prepared.staging);
            return Err(error);
        }

        let mut sink = RegisteringSink::new(host, Vec::new());
        let dispatch_result = install_package(&prepared.archive, &mut sink);
        let (host, undo) = sink.into_parts();
        let summary = match dispatch_result {
            Ok(summary) => summary,
            Err(error) => {
                let failures = compensate(host, &undo);
                self.cleanup(&prepared.staging);
                return Err(finish_rollback(error.into(), failures));
            }
        };

        let record = InstalledPackage {
            id: id.clone(),
            version: candidate.entry.version.clone(),
            source: source.name.clone(),
            digest_hex: prepared.digest_hex.clone(),
            dependencies: prepared.archive.manifest.dependencies.clone(),
            plan,
            identity_keys: identity_keys(&prepared.archive)?,
        };
        state.installed.insert(id.as_str().to_string(), record);
        state.revision += 1;
        if let Err(error) = self.store.save(&state) {
            let failures = compensate(host, &undo);
            self.cleanup(&prepared.staging);
            return Err(finish_rollback(error, failures));
        }
        self.cleanup(&prepared.staging);
        Ok(InstallOutcome {
            id: id.clone(),
            version: candidate.entry.version.clone(),
            source: source.name.clone(),
            digest_hex: prepared.digest_hex,
            summary,
        })
    }

    /// 更新：解析目标版本；与已装版本相同则为验证过的 no-op（switched=false）。
    /// 切换时先移除旧资源（monitor 先 stop），再注册新资源；任一阶段失败补偿恢复。
    pub fn update(
        &mut self,
        host: &mut dyn crate::host::ResourceHost,
        id: &PackageId,
        requirement: Option<&VersionReq>,
        user_approved: bool,
    ) -> Result<UpdateOutcome, MarketplaceError> {
        let mut state = self.store.load()?;
        let current = state
            .installed
            .get(id.as_str())
            .cloned()
            .ok_or_else(|| MarketplaceError::NotInstalled(id.as_str().to_string()))?;
        let pin = state.pin(id).cloned();
        let (source, candidate) = self.resolve(id, requirement, pin.as_ref())?;
        let prepared = self.prepare(id, &source, &candidate, pin.as_ref())?;
        if let Err(error) = self.gates(id, &source, &candidate, &prepared, user_approved) {
            self.cleanup(&prepared.staging);
            return Err(error);
        }
        let from = current.version.clone();
        let to = candidate.entry.version.clone();
        if from == to {
            self.cleanup(&prepared.staging);
            return Ok(UpdateOutcome {
                id: id.clone(),
                from,
                to,
                source: source.name.clone(),
                digest_hex: prepared.digest_hex,
                switched: false,
                summary: DispatchSummary::default(),
            });
        }
        if let Err(error) = check_dependencies(id, &prepared.archive, &state) {
            self.cleanup(&prepared.staging);
            return Err(error);
        }
        if let Err(error) = check_dependents_after_change(id, Some(&to), &state) {
            self.cleanup(&prepared.staging);
            return Err(error);
        }
        let plan = DispatchPlan::from_archive(&prepared.archive);
        if let Err(error) = check_conflicts(&plan, &state, Some(id.as_str())) {
            self.cleanup(&prepared.staging);
            return Err(error);
        }
        if let Err(error) = check_identity_conflicts(&prepared.archive, &state, Some(id.as_str())) {
            self.cleanup(&prepared.staging);
            return Err(error);
        }

        let mut undo: Vec<UndoStep> = Vec::new();
        if let Err(error) = remove_plan(host, &current.plan, &mut undo) {
            let failures = compensate(host, &undo);
            self.cleanup(&prepared.staging);
            return Err(finish_rollback(error, failures));
        }
        let mut sink = RegisteringSink::new(host, undo);
        let dispatch_result = install_package(&prepared.archive, &mut sink);
        let (host, undo) = sink.into_parts();
        let summary = match dispatch_result {
            Ok(summary) => summary,
            Err(error) => {
                let failures = compensate(host, &undo);
                self.cleanup(&prepared.staging);
                return Err(finish_rollback(error.into(), failures));
            }
        };

        let record = InstalledPackage {
            id: id.clone(),
            version: to.clone(),
            source: source.name.clone(),
            digest_hex: prepared.digest_hex.clone(),
            dependencies: prepared.archive.manifest.dependencies.clone(),
            plan,
            identity_keys: identity_keys(&prepared.archive)?,
        };
        state.installed.insert(id.as_str().to_string(), record);
        state.revision += 1;
        if let Err(error) = self.store.save(&state) {
            let failures = compensate(host, &undo);
            self.cleanup(&prepared.staging);
            return Err(finish_rollback(error, failures));
        }
        self.cleanup(&prepared.staging);
        Ok(UpdateOutcome {
            id: id.clone(),
            from,
            to,
            source: source.name,
            digest_hex: prepared.digest_hex,
            switched: true,
            summary,
        })
    }

    /// 卸载：monitor 先 stop 再 unregister，其余资源按安装逆序注销；全部成功后
    /// 才从状态移除。失败补偿恢复已注销资源。
    pub fn uninstall(
        &mut self,
        host: &mut dyn crate::host::ResourceHost,
        id: &PackageId,
    ) -> Result<UninstallOutcome, MarketplaceError> {
        let mut state = self.store.load()?;
        let current = state
            .installed
            .get(id.as_str())
            .cloned()
            .ok_or_else(|| MarketplaceError::NotInstalled(id.as_str().to_string()))?;
        check_dependents_after_change(id, None, &state)?;
        let mut undo: Vec<UndoStep> = Vec::new();
        if let Err(error) = remove_plan(host, &current.plan, &mut undo) {
            let failures = compensate(host, &undo);
            return Err(finish_rollback(error, failures));
        }
        let removed = current.plan.total();
        state.installed.remove(id.as_str());
        state.revision += 1;
        if let Err(error) = self.store.save(&state) {
            let failures = compensate(host, &undo);
            return Err(finish_rollback(error, failures));
        }
        Ok(UninstallOutcome {
            id: id.clone(),
            version: current.version,
            removed,
        })
    }

    /// 多 source 发现并解析唯一候选（pin 与 semver 范围共同约束）。
    fn resolve(
        &self,
        id: &PackageId,
        requirement: Option<&VersionReq>,
        pin: Option<&Pin>,
    ) -> Result<(SourceSpec, Candidate), MarketplaceError> {
        let discovery = discover(&self.sources, &*self.io)?;
        let empty = Vec::new();
        let candidates = discovery.packages.get(id.as_str()).unwrap_or(&empty);
        let candidate = resolve_candidate(id, requirement, pin, candidates)?;
        let source = self.sources[candidate.source_index].clone();
        Ok((source, candidate.clone()))
    }

    /// 拉取归档到独立 staging 目录并做完整性校验：身份（id@version）→ 重算内容
    /// 摘要 → 索引 bundle hash 交叉校验 → hash pin（fail-closed）。
    fn prepare(
        &self,
        id: &PackageId,
        source: &SourceSpec,
        candidate: &Candidate,
        pin: Option<&Pin>,
    ) -> Result<Prepared, MarketplaceError> {
        let staging = self.staging_dir(id, &candidate.entry.version);
        if staging.exists() {
            std::fs::remove_dir_all(&staging).map_err(staging_error)?;
        }
        std::fs::create_dir_all(&staging).map_err(staging_error)?;
        let fail = |error: MarketplaceError| -> MarketplaceError {
            let _ = std::fs::remove_dir_all(&staging);
            error
        };

        if let Err(error) = self.io.fetch_archive(source, &candidate.entry, &staging) {
            return Err(fail(error));
        }
        let archive = match read_archive(&staging) {
            Ok(archive) => archive,
            Err(error) => return Err(fail(error.into())),
        };
        let expected = format!("{}@{}", id.as_str(), candidate.entry.version);
        let found = format!(
            "{}@{}",
            archive.manifest.id.as_str(),
            archive.manifest.version
        );
        if expected != found {
            return Err(fail(MarketplaceError::PackageIdentityMismatch {
                expected,
                found,
            }));
        }
        let digest_hex = content_digest_hex(&archive);
        if let Some(expected_digest) = &candidate.entry.digest_hex {
            if *expected_digest != digest_hex {
                return Err(fail(MarketplaceError::BundleHashMismatch {
                    id: id.as_str().to_string(),
                    version: candidate.entry.version.to_string(),
                    expected: expected_digest.clone(),
                    found: digest_hex,
                }));
            }
        }
        if let Some(Pin::Hash { blake3_hex }) = pin {
            if *blake3_hex != digest_hex {
                return Err(fail(MarketplaceError::HashPinMismatch {
                    id: id.as_str().to_string(),
                    pinned: blake3_hex.clone(),
                    found: digest_hex,
                }));
            }
        }
        Ok(Prepared {
            archive,
            digest_hex,
            staging,
        })
    }

    /// 安全闸门（顺序 fail-closed）：签名（存在即必须有效）→ 组织策略（优先于
    /// 用户批准）→ trust gate（untrusted 需显式批准；verified 必须有效签名）。
    fn gates(
        &self,
        id: &PackageId,
        source: &SourceSpec,
        candidate: &Candidate,
        prepared: &Prepared,
        user_approved: bool,
    ) -> Result<(), MarketplaceError> {
        let version = &candidate.entry.version;
        let mut signature_valid = false;
        if let Some(signature) = &candidate.entry.signature {
            self.keyring
                .verify_archive(id, version, &prepared.archive, signature)?;
            signature_valid = true;
        }
        let effective_trust = self.trust.effective(source, id);
        self.policy.evaluate(&PolicyInput {
            id,
            version,
            source,
            effective_trust,
            signature_valid,
        })?;
        match effective_trust {
            TrustLevel::Untrusted if !user_approved => Err(MarketplaceError::TrustDenied {
                id: id.as_str().to_string(),
                level: effective_trust.to_string(),
                message: "untrusted source requires explicit user approval".to_string(),
            }),
            TrustLevel::Verified if !signature_valid => Err(MarketplaceError::TrustDenied {
                id: id.as_str().to_string(),
                level: effective_trust.to_string(),
                message: "verified trust requires a valid Ed25519 signature".to_string(),
            }),
            _ => Ok(()),
        }
    }

    fn staging_dir(&self, id: &PackageId, version: &Version) -> PathBuf {
        let counter = self.staging_counter.fetch_add(1, Ordering::Relaxed);
        self.staging_root.join(format!(
            "{}-{}-{}-{counter}",
            id.as_str(),
            version,
            std::process::id()
        ))
    }

    fn cleanup(&self, dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// 依赖检查：Package 依赖必须已安装且版本满足；Provider / Runtime 依赖由宿主
/// 运行时层校验（marketplace 不引入 provider/runtime 注册表），此处跳过。
fn check_dependencies(
    id: &PackageId,
    archive: &PackageArchive,
    state: &MarketplaceState,
) -> Result<(), MarketplaceError> {
    for dependency in &archive.manifest.dependencies {
        let PackageDependency::Package {
            id: dependency_id,
            version,
        } = dependency
        else {
            continue;
        };
        if dependency_id.as_str() == id.as_str() {
            continue;
        }
        let requirement =
            VersionReq::parse(version).map_err(|error| MarketplaceError::Resolution {
                message: format!(
                    "dependency {} of {} has invalid version requirement {version}: {error}",
                    dependency_id.as_str(),
                    id.as_str()
                ),
            })?;
        match state.installed.get(dependency_id.as_str()) {
            Some(installed) if requirement.matches(&installed.version) => {}
            Some(installed) => {
                return Err(MarketplaceError::Resolution {
                    message: format!(
                        "dependency {} of {} requires {requirement}, but installed version is {}",
                        dependency_id.as_str(),
                        id.as_str(),
                        installed.version
                    ),
                });
            }
            None => {
                return Err(MarketplaceError::Resolution {
                    message: format!(
                        "dependency {} of {} is not installed",
                        dependency_id.as_str(),
                        id.as_str()
                    ),
                });
            }
        }
    }
    Ok(())
}

/// 验证更新/卸载后的反向依赖闭包。必须在任何宿主资源移除之前执行；失败时
/// 状态与已注册资源均保持不变。Provider / Runtime 依赖仍由宿主运行时负责。
fn check_dependents_after_change(
    changed_id: &PackageId,
    changed_version: Option<&Version>,
    state: &MarketplaceState,
) -> Result<(), MarketplaceError> {
    for dependent in state.installed.values() {
        if dependent.id == *changed_id {
            continue;
        }
        for dependency in &dependent.dependencies {
            let PackageDependency::Package { id, version } = dependency else {
                continue;
            };
            if id != changed_id {
                continue;
            }
            let requirement =
                VersionReq::parse(version).map_err(|error| MarketplaceError::Resolution {
                    message: format!(
                        "dependency {} of {} has invalid version requirement {version}: {error}",
                        changed_id.as_str(),
                        dependent.id.as_str()
                    ),
                })?;
            match changed_version {
                Some(candidate) if requirement.matches(candidate) => {}
                Some(candidate) => {
                    return Err(MarketplaceError::Resolution {
                        message: format!(
                            "cannot change {} to {candidate}: installed package {} requires {requirement}",
                            changed_id.as_str(),
                            dependent.id.as_str()
                        ),
                    });
                }
                None => {
                    return Err(MarketplaceError::Resolution {
                        message: format!(
                            "cannot uninstall {}: installed package {} requires {requirement}",
                            changed_id.as_str(),
                            dependent.id.as_str()
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

/// 计划资源的 (kind, key) 列表（冲突检测与宿主键约定一致）。
fn plan_keys(plan: &DispatchPlan) -> Vec<(&'static str, String)> {
    let mut keys = Vec::new();
    for dispatch in &plan.skills {
        keys.push(("skill", skill_key(dispatch)));
    }
    for dispatch in &plan.agents {
        keys.push(("agent", agent_key(dispatch)));
    }
    for dispatch in &plan.hooks {
        keys.push(("hook", hook_key(dispatch)));
    }
    for dispatch in &plan.mcp {
        keys.push(("mcp", mcp_key(dispatch)));
    }
    for dispatch in &plan.lsp {
        keys.push(("lsp", lsp_key(dispatch)));
    }
    for dispatch in &plan.monitors {
        keys.push(("monitor", monitor_key(dispatch)));
    }
    keys
}

/// 冲突检查：新计划的资源键不得与其他已安装包重叠（exclude 用于更新时排除自身）。
fn check_conflicts(
    plan: &DispatchPlan,
    state: &MarketplaceState,
    exclude: Option<&str>,
) -> Result<(), MarketplaceError> {
    let new_keys = plan_keys(plan);
    for (package_id, installed) in &state.installed {
        if exclude == Some(package_id.as_str()) {
            continue;
        }
        for (kind, key) in plan_keys(&installed.plan) {
            let collides = new_keys
                .iter()
                .any(|(new_kind, new_key)| *new_kind == kind && new_key == &key);
            if collides {
                return Err(MarketplaceError::ResourceConflict {
                    kind: kind.to_string(),
                    key,
                    package: package_id.clone(),
                });
            }
        }
    }
    Ok(())
}

fn staging_error(error: std::io::Error) -> MarketplaceError {
    MarketplaceError::Staging(error.to_string())
}

/// 从已验证归档抽出身份键。
///
/// 只持久化显式身份（skill `id`、agent `name`、hook `trigger`、mcp server 名、
/// lsp `id`、monitor id）。子 manifest 只有 `name`、没有 `id` 的旧 skill 夹具
/// 不产生 skill 身份键，仍由路径键 `check_conflicts` 兜底。
/// 子 manifest 无法读取或不是合法 TOML / JSON 时 fail-closed。
fn identity_keys(archive: &PackageArchive) -> Result<Vec<(String, String)>, MarketplaceError> {
    let mut keys = Vec::new();
    for skill in &archive.manifest.skills {
        if let Some(path) = skill.path() {
            if let Some(id) = optional_sub_identity(archive, path, Some("manifest.toml"), &["id"])? {
                keys.push(("skill".into(), id));
            }
        }
    }
    for agent in &archive.manifest.agents {
        if let Some(inline) = agent.inline() {
            if let Some(name) = inline.get("name").and_then(|value| value.as_str()) {
                keys.push(("agent".into(), name.to_string()));
            }
        } else if let Some(path) = agent.path() {
            if let Some(name) = optional_sub_identity(archive, path, None, &["name"])? {
                keys.push(("agent".into(), name));
            }
        }
    }
    for hook in &archive.manifest.hooks {
        if let Some(inline) = hook.inline() {
            if let Some(trigger) = inline.get("trigger").and_then(|value| value.as_str()) {
                keys.push(("hook".into(), trigger.to_string()));
            }
        } else if let Some(path) = hook.path() {
            if let Some(trigger) = optional_sub_identity(archive, path, None, &["trigger"])? {
                keys.push(("hook".into(), trigger));
            }
        }
    }
    for server in &archive.manifest.mcp {
        keys.push(("mcp".into(), server.name.clone()));
    }
    for lsp in &archive.manifest.lsp {
        if let Some(inline) = lsp.inline() {
            if let Some(id) = inline.get("id").and_then(|value| value.as_str()) {
                keys.push(("lsp".into(), id.to_string()));
            }
        } else if let Some(path) = lsp.path() {
            if let Some(id) = optional_sub_identity(archive, path, None, &["id"])? {
                keys.push(("lsp".into(), id));
            }
        }
    }
    for monitor in &archive.manifest.monitors {
        keys.push(("monitor".into(), monitor.monitor_id.as_str().to_string()));
    }
    Ok(keys)
}

fn optional_sub_identity(
    archive: &PackageArchive,
    path: &plugin_package::PackageRelativePath,
    sub_file: Option<&str>,
    fields: &[&str],
) -> Result<Option<String>, MarketplaceError> {
    let file_path = match sub_file {
        Some(name) => plugin_package::PackageRelativePath::new(path.as_path().join(name))?,
        None => path.clone(),
    };
    let bytes = archive.read_file(&file_path)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        MarketplaceError::Package(plugin_package::PackageError::ManifestField {
            field: format!("sub-manifest `{}`", file_path.to_posix_string()),
            message: "must be valid UTF-8".into(),
        })
    })?;
    if let Ok(table) = toml::from_str::<toml::Value>(text) {
        return Ok(first_toml_string(&table, fields));
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        return Ok(first_json_string(&value, fields));
    }
    Err(MarketplaceError::Package(
        plugin_package::PackageError::ManifestField {
            field: format!("sub-manifest `{}`", file_path.to_posix_string()),
            message: "must be valid TOML or JSON".into(),
        },
    ))
}

fn first_toml_string(table: &toml::Value, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        table
            .get(field)
            .and_then(toml::Value::as_str)
            .map(str::to_string)
    })
}

fn first_json_string(value: &serde_json::Value, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        value
            .get(field)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    })
}

/// 身份冲突：路径键之外再比对已持久化的 identity_keys。
/// 两个包即使路径不同，只要 skill/agent/hook/mcp/lsp/monitor 身份相同也 fail-closed。
/// `exclude` 用于更新时排除自身。旧快照没有身份键时只走路径键检查。
fn check_identity_conflicts(
    incoming: &PackageArchive,
    state: &MarketplaceState,
    exclude: Option<&str>,
) -> Result<(), MarketplaceError> {
    let incoming_keys = identity_keys(incoming)?;
    if incoming_keys.is_empty() {
        return Ok(());
    }
    for (package_id, installed) in &state.installed {
        if exclude == Some(package_id.as_str()) {
            continue;
        }
        for (kind, key) in &incoming_keys {
            if installed
                .identity_keys
                .iter()
                .any(|(installed_kind, installed_key)| {
                    installed_kind == kind && installed_key == key
                })
            {
                return Err(MarketplaceError::ResourceConflict {
                    kind: kind.clone(),
                    key: key.clone(),
                    package: package_id.clone(),
                });
            }
        }
    }
    Ok(())
}

/// 回滚收尾：补偿全部成功则上报原始错误；存在补偿失败升级为 RollbackFailed。
fn finish_rollback(original: MarketplaceError, failures: Vec<String>) -> MarketplaceError {
    if failures.is_empty() {
        original
    } else {
        MarketplaceError::RollbackFailed {
            original: original.to_string(),
            compensation_failures: failures,
        }
    }
}
