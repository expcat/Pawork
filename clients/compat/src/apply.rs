//! 加载编排：scan（只读探测 + 解析 + 映射）→ dry-run 预览 → 显式幂等 export_plan。
//!
//! export_plan 只把 canonical 计划写入调用方指定的输出目录，绝不执行 hook / MCP /
//! script，绝不改写外部源文件。凭据只以 reference 形式出现在计划中。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use pawork_domain::WorkspaceId;

use crate::hook::HookScope;

use crate::detect::{detect_files, DetectedFile};
use crate::error::CompatError;
use crate::io::{atomic_write, fnv64, is_symlink, read_utf8_bounded};
use crate::limits::CompatLimits;
use crate::map::resolve_conflicts;
use crate::model::{CompatIssue, CompatPlan, CredentialReference, ImportStatus};
use crate::parse::{parse_content, ParseOutcome};
use crate::source::GlobalSource;

/// export_plan 写入的计划文件名。
pub const PLAN_FILE_NAME: &str = "compat-import.json";
/// export_plan 写入的幂等指纹文件名。
pub const FINGERPRINT_FILE_NAME: &str = ".compat-import-fingerprint";

/// 指纹格式版本：纳入指纹后，映射格式演进或序列化方式变化都能使旧指纹失效，
/// 避免误命中 noop。
const FINGERPRINT_FORMAT_VERSION: u64 = 1;

/// export_plan 结果：首次写入或指纹命中后的 noop。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportOutcome {
    Exported,
    Noop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportReport {
    pub outcome: ExportOutcome,
    pub items: usize,
    pub bytes_written: u64,
    pub plan_path: PathBuf,
}

/// P17-13 Compatibility Loader 入口。
#[derive(Clone, Debug, Default)]
pub struct CompatLoader {
    limits: CompatLimits,
}

impl CompatLoader {
    pub fn new(limits: CompatLimits) -> Self {
        Self { limits }
    }

    /// 只读扫描：探测 workspace（可选）与显式启用的全局来源根，解析并映射为
    /// canonical 计划。不做任何写入、不执行任何内容。
    ///
    /// workspace_id 用于给 workspace 来源的 hook 打 Workspace scope；
    /// 缺省时 fallback 为 Global（所有 hook 默认 disabled 且 requires_review）。
    pub fn scan(
        &self,
        workspace_root: Option<&Path>,
        globals: &[GlobalSource],
        workspace_id: Option<&WorkspaceId>,
    ) -> Result<CompatPlan, CompatError> {
        let mut issues = Vec::new();
        let mut summaries = Vec::new();
        let mut outcomes: Vec<ParseOutcome> = Vec::new();
        let mut chain: u64 = 0xcbf2_9ce4_8422_2325;

        let workspace_scope = match workspace_id {
            Some(id) => HookScope::Workspace {
                workspace_id: id.clone(),
            },
            None => HookScope::Global,
        };

        if let Some(root) = workspace_root {
            let files = detect_files(Some(root), &[], self.limits, &mut issues, &mut summaries);
            self.load_root(
                root,
                files,
                &workspace_scope,
                &mut issues,
                &mut outcomes,
                &mut chain,
            );
        }

        let mut ordered: Vec<&GlobalSource> = globals.iter().collect();
        ordered.sort_by(|left, right| {
            left.source
                .cmp(&right.source)
                .then_with(|| left.root.cmp(&right.root))
        });
        for global in ordered {
            let specs = [(global.source, global.root.clone())];
            let files = detect_files(None, &specs, self.limits, &mut issues, &mut summaries);
            self.load_root(
                &global.root,
                files,
                &HookScope::Global,
                &mut issues,
                &mut outcomes,
                &mut chain,
            );
        }

        let mut items = Vec::new();
        let mut credential_references: Vec<CredentialReference> = Vec::new();
        for outcome in outcomes {
            items.push(outcome.item);
            credential_references.extend(outcome.credentials);
        }
        credential_references.sort();
        credential_references.dedup();

        let items = resolve_conflicts(items, &mut issues);

        let mut plan = CompatPlan {
            manifest_version: 1,
            sources: summaries.iter().map(|summary| summary.external).collect(),
            items,
            issues,
            credential_references,
            fingerprint: String::new(),
        };
        plan.sort_deterministically();
        plan.fingerprint = fingerprint(chain, plan.manifest_version);
        Ok(plan)
    }

    /// dry-run：返回稳定文本预览，不写任何文件。
    pub fn dry_run(&self, plan: &CompatPlan) -> String {
        plan.preview()
    }

    /// 显式幂等 export_plan：把计划写入 output_dir；相同输入指纹重复调用直接 noop。
    /// 拒绝输出目录 / 目标文件为 symlink（防 symlink 逃逸），通过 tmp + rename
    /// 原子写入，noop 时同时校验计划文件的内容身份。不执行 hook / MCP / script，
    /// 不改写任何外部源文件。
    pub fn export_plan(
        &self,
        plan: &CompatPlan,
        output_dir: &Path,
    ) -> Result<ExportReport, CompatError> {
        std::fs::create_dir_all(output_dir).map_err(|error| CompatError::io(output_dir, error))?;
        if is_symlink(output_dir) {
            return Err(CompatError::UnsafeTarget(
                "output dir must not be a symlink".to_string(),
            ));
        }
        let plan_path = output_dir.join(PLAN_FILE_NAME);
        let fingerprint_path = output_dir.join(FINGERPRINT_FILE_NAME);
        if is_symlink(&plan_path) || is_symlink(&fingerprint_path) {
            return Err(CompatError::UnsafeTarget(
                "export target must not be a symlink".to_string(),
            ));
        }
        let payload = serde_json::to_vec_pretty(plan)
            .map_err(|error| CompatError::Invalid(format!("serialize plan: {error}")))?;
        // noop：指纹命中，且计划文件内容身份与当前序列化一致才跳过写入；
        // 指纹一致但内容被篡改 / 陈旧时仍重写（Exported）。
        let existing_fp = std::fs::read_to_string(&fingerprint_path).ok();
        if existing_fp.as_deref() == Some(plan.fingerprint.as_str()) && plan_path.is_file() {
            let on_disk = std::fs::read(&plan_path).unwrap_or_default();
            if on_disk.as_slice() == payload.as_slice() {
                return Ok(ExportReport {
                    outcome: ExportOutcome::Noop,
                    items: plan.items.len(),
                    bytes_written: 0,
                    plan_path,
                });
            }
        }
        atomic_write(&plan_path, &payload)?;
        atomic_write(&fingerprint_path, plan.fingerprint.as_bytes())?;
        Ok(ExportReport {
            outcome: ExportOutcome::Exported,
            items: plan.items.len(),
            bytes_written: payload.len() as u64,
            plan_path,
        })
    }

    fn load_root(
        &self,
        root: &Path,
        files: Vec<DetectedFile>,
        hook_scope: &HookScope,
        issues: &mut Vec<CompatIssue>,
        outcomes: &mut Vec<ParseOutcome>,
        chain: &mut u64,
    ) {
        for file in files {
            let rel = Path::new(&file.relative_path);
            let content = match read_utf8_bounded(root, rel, self.limits.max_file_bytes) {
                Ok(content) => content,
                Err(_) => {
                    issues.push(
                        CompatIssue::warning(
                            "read_failed",
                            format!("candidate unreadable during scan: {}", file.relative_path),
                        )
                        .with_source(file.relative_path.clone()),
                    );
                    continue;
                }
            };
            let header = format!("{}:{}", file.tier.priority(), file.relative_path);
            let mut digest = fnv64(header.as_bytes());
            digest ^= fnv64(content.as_bytes());
            digest = digest.wrapping_mul(0x100_0000_01b3);
            *chain ^= digest;
            *chain = chain.wrapping_mul(0x100_0000_01b3);
            parse_content(&file, &content, hook_scope.clone(), issues, outcomes);
        }
    }
}

impl CompatPlan {
    /// dry-run 预览：稳定文本，不含文件正文 / 命令参数 / Secret 值。
    pub fn preview(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "P17-13 dry-run preview: {} item(s), {} issue(s), {} credential reference(s), fingerprint {}",
            self.items.len(),
            self.issues.len(),
            self.credential_references.len(),
            self.fingerprint,
        ));
        for item in &self.items {
            lines.push(format!(
                "[{}] {} {} <- source={} tier={} path={} review={}",
                status_str(item.status),
                item.category.as_str(),
                item.id,
                item.source.external.as_str(),
                item.source.tier.priority(),
                item.source.relative_path,
                item.requires_review,
            ));
        }
        for reference in &self.credential_references {
            lines.push(format!(
                "credential service={} account={} location={} <- source={}",
                reference.service,
                reference.account,
                reference.location,
                reference.source.external.as_str(),
            ));
        }
        lines.join("\n")
    }

    /// 按项选择：只保留指定 id 的条目（显式应用前的人工筛选）。
    pub fn select(&self, item_ids: &BTreeSet<String>) -> CompatPlan {
        let mut plan = self.clone();
        plan.items.retain(|item| item_ids.contains(&item.id));
        plan.credential_references.retain(|reference| {
            plan.items.iter().any(|item| {
                item.category == crate::model::ImportCategory::McpServer
                    && item.source == reference.source
                    && item.id.strip_prefix("mcp:").is_some_and(|server| {
                        reference.service == "mcp"
                            && reference
                                .account
                                .strip_prefix(server)
                                .is_some_and(|suffix| suffix.starts_with(':'))
                    })
            })
        });

        // 选择集属于 export_plan 身份的一部分；不同选择不能共享同一个幂等指纹。
        // 使用长度前缀避免 `["ab", "c"]` 与 `["a", "bc"]` 之类的拼接歧义。
        let mut selected = self.fingerprint.as_bytes().to_vec();
        for item_id in item_ids {
            selected.extend_from_slice(&(item_id.len() as u64).to_le_bytes());
            selected.extend_from_slice(item_id.as_bytes());
        }
        plan.fingerprint = format!("{:016x}", fnv64(&selected));
        plan
    }

    /// 各状态条目数（供预览与报告）。
    pub fn counts_by_status(&self) -> BTreeMap<ImportStatus, usize> {
        let mut counts = BTreeMap::new();
        for item in &self.items {
            *counts.entry(item.status).or_insert(0) += 1;
        }
        counts
    }
}

fn status_str(status: ImportStatus) -> &'static str {
    match status {
        ImportStatus::Imported => "imported",
        ImportStatus::Disabled => "disabled",
        ImportStatus::Unsupported => "unsupported",
        ImportStatus::Conflict => "conflict",
    }
}

/// 计划指纹：纳入映射格式版本（FINGERPRINT_FORMAT_VERSION）、manifest_version
/// 与输入内容链。选择 / 序列化内容由 `CompatPlan::select` 在此基础上二次混入，
/// 确保不同选择互不共享幂等身份。
fn fingerprint(content_chain: u64, manifest_version: u32) -> String {
    let mut hash = fnv64(&FINGERPRINT_FORMAT_VERSION.to_le_bytes());
    hash ^= fnv64(&manifest_version.to_le_bytes());
    hash = hash.wrapping_mul(0x100000001b3);
    hash ^= content_chain;
    hash = hash.wrapping_mul(0x100000001b3);
    format!("{hash:016x}")
}
