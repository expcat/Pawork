//! Markdown Prompt Template 加载与渲染。

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

use config_service::ConfigTier;
use serde::{Deserialize, Serialize};

use crate::{
    io::{
        path_key, read_utf8_bounded, read_utf8_bounded_within, sorted_children_within,
        workspace_relative_key,
    },
    ResourceDiagnosticEntry, ResourceDiagnosticStatus, ResourceDiagnostics, ResourceIssue,
    ResourceKind, ResourceLimits, ResourceOrigin, ResourceProvenance, ResourceSelection,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptParameter {
    pub description: Option<String>,
    pub required: bool,
    pub default: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptDefaults {
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub tools: Vec<String>,
    pub budget: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptTemplate {
    pub id: String,
    pub description: Option<String>,
    pub parameters: BTreeMap<String, PromptParameter>,
    pub file_refs: Vec<String>,
    pub defaults: PromptDefaults,
    pub body: String,
    pub provenance: ResourceProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderedPrompt {
    pub template_id: String,
    pub content: String,
    pub defaults: PromptDefaults,
    pub provenance: ResourceProvenance,
    pub included_files: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateResolution {
    pub templates: Vec<PromptTemplate>,
    pub selected: Option<RenderedPrompt>,
    pub diagnostics: ResourceDiagnostics,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct PromptHeader {
    id: Option<String>,
    description: Option<String>,
    parameters: BTreeMap<String, PromptParameter>,
    files: Vec<String>,
    defaults: PromptDefaults,
}

#[derive(Clone)]
struct Candidate {
    template: PromptTemplate,
    workspace_root: Option<PathBuf>,
}

impl PromptTemplate {
    /// 渲染纯参数模板。包含 `{{file:...}}` 时应通过 [`ResourceLoader`](crate::ResourceLoader)
    /// 渲染，以便执行工作区边界和大小限制。
    pub fn render(
        &self,
        arguments: &BTreeMap<String, String>,
    ) -> Result<RenderedPrompt, ResourceIssue> {
        render_candidate(
            &Candidate {
                template: self.clone(),
                workspace_root: None,
            },
            arguments,
            ResourceLimits::default(),
        )
    }
}

pub(crate) fn load_templates(
    global_resource_dir: Option<&Path>,
    workspace_roots: &[PathBuf],
    workspace_resource_dir: &str,
    selected_root_index: usize,
    selection: &ResourceSelection,
    limits: ResourceLimits,
) -> TemplateResolution {
    let mut candidates = Vec::new();
    let mut diagnostics = ResourceDiagnostics::default();

    if let Some(global_root) = global_resource_dir {
        load_directory(
            &global_root.join("prompts"),
            global_root,
            ConfigTier::Global,
            None,
            limits,
            &mut candidates,
            &mut diagnostics,
        );
    }
    for (root_index, root) in workspace_roots.iter().enumerate() {
        let directory = root.join(workspace_resource_dir).join("prompts");
        load_directory(
            &directory,
            root,
            ConfigTier::Workspace,
            Some(root_index),
            limits,
            &mut candidates,
            &mut diagnostics,
        );
    }

    candidates.sort_by(|left, right| {
        left.template
            .provenance
            .tier
            .cmp(&right.template.provenance.tier)
            .then_with(|| {
                left.template
                    .provenance
                    .source_key
                    .cmp(&right.template.provenance.source_key)
            })
    });

    let mut effective: BTreeMap<String, Candidate> = BTreeMap::new();
    for candidate in candidates {
        if let Some(overridden) = effective.insert(candidate.template.id.clone(), candidate.clone())
        {
            diagnostics.entries.push(ResourceDiagnosticEntry {
                kind: ResourceKind::PromptTemplate,
                resource_id: overridden.template.id.clone(),
                status: ResourceDiagnosticStatus::Overridden,
                provenance: overridden.template.provenance,
            });
        }
    }

    let selected_candidate = selection
        .prompt_template
        .as_ref()
        .and_then(|id| effective.get(id));
    let selected = match (&selection.prompt_template, selected_candidate) {
        (Some(id), None) => {
            diagnostics.issues.push(
                ResourceIssue::error(
                    "prompt_template_not_found",
                    format!("selected prompt template '{id}' was not found"),
                )
                .for_resource(ResourceKind::PromptTemplate, id, "selection:prompt"),
            );
            None
        }
        (Some(_), Some(candidate)) => {
            let mut candidate_for_render = candidate.clone();
            // 文件引用始终以当前请求所属 workspace root 为边界，而不是模板来源目录。
            candidate_for_render.workspace_root = workspace_roots.get(selected_root_index).cloned();
            match render_candidate(&candidate_for_render, &selection.prompt_arguments, limits) {
                Ok(rendered) => Some(rendered),
                Err(issue) => {
                    diagnostics.issues.push(issue);
                    None
                }
            }
        }
        (None, _) => None,
    };

    let selected_id = selected
        .as_ref()
        .map(|rendered| rendered.template_id.as_str());
    let requested_id = selection.prompt_template.as_deref();
    let mut templates = Vec::with_capacity(effective.len());
    for (_, candidate) in effective {
        diagnostics.entries.push(ResourceDiagnosticEntry {
            kind: ResourceKind::PromptTemplate,
            resource_id: candidate.template.id.clone(),
            status: if selected_id == Some(candidate.template.id.as_str()) {
                ResourceDiagnosticStatus::Active
            } else if requested_id == Some(candidate.template.id.as_str()) {
                ResourceDiagnosticStatus::Rejected
            } else {
                ResourceDiagnosticStatus::Loaded
            },
            provenance: candidate.template.provenance.clone(),
        });
        templates.push(candidate.template);
    }
    diagnostics.sort_deterministically();
    TemplateResolution {
        templates,
        selected,
        diagnostics,
    }
}

fn load_directory(
    directory: &Path,
    source_root: &Path,
    tier: ConfigTier,
    root_index: Option<usize>,
    limits: ResourceLimits,
    candidates: &mut Vec<Candidate>,
    diagnostics: &mut ResourceDiagnostics,
) {
    let paths = match sorted_children_within(directory, source_root, limits.max_resources_per_kind)
    {
        Ok(paths) => paths,
        Err(error) => {
            diagnostics.issues.push(ResourceIssue::error(
                error.code(),
                format!("prompt directory could not be read: {error}"),
            ));
            return;
        }
    };
    for path in paths {
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let relative = workspace_relative_key(&path, source_root);
        let source_key = match root_index {
            Some(index) => format!("workspace:{index:08}:prompt:{relative}"),
            None => format!("global:prompt:{relative}"),
        };
        let provenance = ResourceProvenance::new(
            tier,
            source_key.clone(),
            match root_index {
                Some(index) => ResourceOrigin::Workspace {
                    root_index: index,
                    relative_path: relative,
                },
                None => ResourceOrigin::Global {
                    relative_path: relative,
                },
            },
        );
        match parse_template(&path, source_root, provenance, limits) {
            Ok(template) => candidates.push(Candidate {
                template,
                workspace_root: root_index.map(|_| source_root.to_path_buf()),
            }),
            Err(issue) => diagnostics.issues.push(issue),
        }
    }
}

fn parse_template(
    path: &Path,
    source_root: &Path,
    provenance: ResourceProvenance,
    limits: ResourceLimits,
) -> Result<PromptTemplate, ResourceIssue> {
    let source_key = provenance.source_key.clone();
    let content =
        read_utf8_bounded_within(path, source_root, limits.max_file_bytes).map_err(|error| {
            ResourceIssue::error(
                error.code(),
                format!("prompt template could not be loaded: {error}"),
            )
            .for_resource(
                ResourceKind::PromptTemplate,
                file_stem(path),
                source_key.clone(),
            )
        })?;
    let (header, body) = split_frontmatter(&content).map_err(|message| {
        ResourceIssue::error("prompt_frontmatter_invalid", message).for_resource(
            ResourceKind::PromptTemplate,
            file_stem(path),
            source_key.clone(),
        )
    })?;
    let mut header: PromptHeader = toml::from_str(header).map_err(|_| {
        ResourceIssue::error(
            "prompt_frontmatter_invalid",
            "prompt frontmatter has invalid TOML syntax",
        )
        .for_resource(
            ResourceKind::PromptTemplate,
            file_stem(path),
            source_key.clone(),
        )
    })?;
    let id = header.id.unwrap_or_else(|| file_stem(path));
    validate_id(&id).map_err(|message| {
        ResourceIssue::error("prompt_id_invalid", message).for_resource(
            ResourceKind::PromptTemplate,
            &id,
            source_key.clone(),
        )
    })?;
    for name in header.parameters.keys() {
        validate_parameter_name(name).map_err(|message| {
            ResourceIssue::error("prompt_parameter_invalid", message).for_resource(
                ResourceKind::PromptTemplate,
                &id,
                source_key.clone(),
            )
        })?;
    }
    header.files.sort();
    header.files.dedup();
    if header.files.len() > limits.max_template_file_refs {
        return Err(ResourceIssue::error(
            "prompt_file_reference_limit",
            format!(
                "prompt declares more than {} file references",
                limits.max_template_file_refs
            ),
        )
        .for_resource(ResourceKind::PromptTemplate, &id, source_key.clone()));
    }
    for file_ref in &header.files {
        validate_relative_reference(file_ref).map_err(|message| {
            ResourceIssue::error("prompt_file_reference_invalid", message).for_resource(
                ResourceKind::PromptTemplate,
                &id,
                source_key.clone(),
            )
        })?;
    }
    Ok(PromptTemplate {
        id,
        description: header.description,
        parameters: header.parameters,
        file_refs: header.files,
        defaults: header.defaults,
        body: body.to_owned(),
        provenance,
    })
}

fn split_frontmatter(content: &str) -> Result<(&str, &str), String> {
    let mut lines = content.split_inclusive('\n');
    let first = lines.next().unwrap_or_default();
    if first.trim_end_matches(['\r', '\n']) != "+++" {
        return Err("prompt template must start with a +++ TOML frontmatter block".into());
    }
    let header_start = first.len();
    let mut cursor = header_start;
    for line in lines {
        let line_end = cursor + line.len();
        if line.trim_end_matches(['\r', '\n']) == "+++" {
            return Ok((&content[header_start..cursor], &content[line_end..]));
        }
        cursor = line_end;
    }
    Err("prompt template frontmatter is missing the closing +++ delimiter".into())
}

fn render_candidate(
    candidate: &Candidate,
    arguments: &BTreeMap<String, String>,
    limits: ResourceLimits,
) -> Result<RenderedPrompt, ResourceIssue> {
    let template = &candidate.template;
    let source_key = template.provenance.source_key.clone();
    let mut values: BTreeMap<&str, &str> = BTreeMap::new();
    for (name, parameter) in &template.parameters {
        if let Some(value) = arguments.get(name).or(parameter.default.as_ref()) {
            values.insert(name.as_str(), value.as_str());
        } else if parameter.required {
            return Err(ResourceIssue::error(
                "prompt_parameter_missing",
                format!("required prompt parameter '{name}' is missing"),
            )
            .for_resource(ResourceKind::PromptTemplate, &template.id, source_key));
        }
    }

    let allowed_files = template.file_refs.iter().cloned().collect::<BTreeSet<_>>();
    let mut included_files = BTreeSet::new();
    let mut file_cache: BTreeMap<String, String> = BTreeMap::new();
    let mut file_reference_count = 0_usize;
    let content = replace_placeholders(
        &template.body,
        limits.max_rendered_prompt_bytes,
        |placeholder| {
            if let Some(reference) = placeholder.strip_prefix("file:") {
                validate_relative_reference(reference)?;
                if !allowed_files.contains(reference) {
                    return Err(format!(
                        "file reference '{reference}' is not declared in frontmatter files"
                    ));
                }
                file_reference_count = file_reference_count.saturating_add(1);
                if file_reference_count > limits.max_template_file_refs {
                    return Err(format!(
                        "prompt uses more than {} file references",
                        limits.max_template_file_refs
                    ));
                }
                if let Some(content) = file_cache.get(reference) {
                    included_files.insert(reference.to_owned());
                    return Ok(content.clone());
                }
                let root = candidate.workspace_root.as_deref().ok_or_else(|| {
                    format!("file reference '{reference}' requires a workspace context")
                })?;
                let path = checked_workspace_file(root, reference)?;
                let content = read_utf8_bounded(&path, limits.max_file_bytes).map_err(|error| {
                    format!("file reference '{reference}' could not be loaded: {error}")
                })?;
                included_files.insert(reference.to_owned());
                file_cache.insert(reference.to_owned(), content.clone());
                Ok(content)
            } else {
                let value = values.get(placeholder).copied().ok_or_else(|| {
                    "prompt contains an undeclared parameter placeholder".to_string()
                })?;
                if u64::try_from(value.len()).unwrap_or(u64::MAX) > limits.max_rendered_prompt_bytes
                {
                    return Err("prompt parameter exceeds the rendered prompt byte limit".into());
                }
                Ok(value.to_owned())
            }
        },
    )
    .map_err(|message| {
        ResourceIssue::error("prompt_render_failed", message).for_resource(
            ResourceKind::PromptTemplate,
            &template.id,
            template.provenance.source_key.clone(),
        )
    })?;

    Ok(RenderedPrompt {
        template_id: template.id.clone(),
        content,
        defaults: template.defaults.clone(),
        provenance: template.provenance.clone(),
        included_files: included_files.into_iter().collect(),
    })
}

fn replace_placeholders(
    input: &str,
    max_output_bytes: u64,
    mut value_for: impl FnMut(&str) -> Result<String, String>,
) -> Result<String, String> {
    let maximum = usize::try_from(max_output_bytes).unwrap_or(usize::MAX);
    let mut output = String::with_capacity(input.len().min(maximum));
    let mut rest = input;
    while let Some(start) = rest.find("{{") {
        push_bounded(&mut output, &rest[..start], maximum)?;
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("}}") else {
            return Err("prompt contains an unclosed '{{' placeholder".into());
        };
        let placeholder = after_start[..end].trim();
        if placeholder.is_empty() {
            return Err("prompt contains an empty placeholder".into());
        }
        let replacement = value_for(placeholder)?;
        push_bounded(&mut output, &replacement, maximum)?;
        rest = &after_start[end + 2..];
    }
    push_bounded(&mut output, rest, maximum)?;
    Ok(output)
}

fn push_bounded(output: &mut String, value: &str, maximum: usize) -> Result<(), String> {
    if output.len().saturating_add(value.len()) > maximum {
        return Err(format!(
            "rendered prompt exceeds the configured {maximum}-byte limit"
        ));
    }
    output.push_str(value);
    Ok(())
}

fn checked_workspace_file(root: &Path, reference: &str) -> Result<PathBuf, String> {
    validate_relative_reference(reference)?;
    let root = dunce::canonicalize(root)
        .map_err(|error| format!("workspace root could not be resolved: {error}"))?;
    let candidate = root.join(reference);
    let canonical = dunce::canonicalize(&candidate)
        .map_err(|error| format!("file reference '{reference}' could not be resolved: {error}"))?;
    if !canonical.starts_with(&root) {
        return Err(format!(
            "file reference '{reference}' leaves the workspace root"
        ));
    }
    if !canonical.is_file() {
        return Err(format!(
            "file reference '{reference}' is not a regular file"
        ));
    }
    Ok(canonical)
}

fn validate_relative_reference(reference: &str) -> Result<(), String> {
    let path = Path::new(reference);
    if reference.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "file reference '{reference}' must be a non-empty workspace-relative path without '..'"
        ));
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!(
            "prompt id '{id}' must contain only ASCII letters, digits, '.', '-' or '_'"
        ));
    }
    Ok(())
}

fn validate_parameter_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(format!("prompt parameter name '{name}' is invalid"));
    }
    Ok(())
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| path_key(path))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn write_template(root: &Path, relative: &str, body: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, body).expect("write template");
    }

    #[test]
    fn workspace_template_overrides_global_and_renders_defaults() {
        let global = tempfile::tempdir().expect("global");
        let workspace = tempfile::tempdir().expect("workspace");
        write_template(
            global.path(),
            "prompts/review.md",
            "+++\nid='review'\n[parameters.target]\nrequired=true\n+++\nglobal {{target}}",
        );
        write_template(
            workspace.path(),
            ".pawork/prompts/review.md",
            "+++\nid='review'\n[parameters.target]\ndefault='workspace'\n[defaults]\nmodel='m'\nbudget=42\n+++\nreview {{target}}",
        );
        let selection = ResourceSelection {
            prompt_template: Some("review".into()),
            ..ResourceSelection::default()
        };
        let resolution = load_templates(
            Some(global.path()),
            &[workspace.path().to_path_buf()],
            ".pawork",
            0,
            &selection,
            ResourceLimits::default(),
        );
        let rendered = resolution.selected.expect("selected");
        assert_eq!(rendered.content, "review workspace");
        assert_eq!(rendered.defaults.model.as_deref(), Some("m"));
        assert_eq!(rendered.defaults.budget, Some(42));
        assert!(resolution
            .diagnostics
            .entries
            .iter()
            .any(|entry| entry.status == ResourceDiagnosticStatus::Overridden));
    }

    #[test]
    fn file_reference_is_bounded_to_workspace() {
        let workspace = tempfile::tempdir().expect("workspace");
        fs::write(workspace.path().join("input.txt"), "inside").expect("input");
        write_template(
            workspace.path(),
            ".pawork/prompts/include.md",
            "+++\nid='include'\nfiles=['input.txt']\n+++\n{{file:input.txt}}",
        );
        let selection = ResourceSelection {
            prompt_template: Some("include".into()),
            ..ResourceSelection::default()
        };
        let resolution = load_templates(
            None,
            &[workspace.path().to_path_buf()],
            ".pawork",
            0,
            &selection,
            ResourceLimits::default(),
        );
        assert_eq!(resolution.selected.expect("rendered").content, "inside");
        assert!(validate_relative_reference("../secret").is_err());
        assert!(validate_relative_reference("/secret").is_err());
    }

    #[test]
    fn missing_parameter_is_isolated() {
        let workspace = tempfile::tempdir().expect("workspace");
        write_template(
            workspace.path(),
            ".pawork/prompts/review.md",
            "+++\nid='review'\n[parameters.target]\nrequired=true\n+++\n{{target}}",
        );
        let selection = ResourceSelection {
            prompt_template: Some("review".into()),
            ..ResourceSelection::default()
        };
        let resolution = load_templates(
            None,
            &[workspace.path().to_path_buf()],
            ".pawork",
            0,
            &selection,
            ResourceLimits::default(),
        );
        assert!(resolution.selected.is_none());
        assert!(resolution
            .diagnostics
            .issues
            .iter()
            .any(|issue| issue.code == "prompt_parameter_missing"));
        assert!(resolution.diagnostics.entries.iter().any(|entry| {
            entry.resource_id == "review" && entry.status == ResourceDiagnosticStatus::Rejected
        }));
    }

    #[test]
    fn file_reference_must_be_declared_and_usage_is_bounded() {
        let workspace = tempfile::tempdir().expect("workspace");
        fs::write(workspace.path().join("input.txt"), "inside").expect("input");
        write_template(
            workspace.path(),
            ".pawork/prompts/include.md",
            "+++\nid='include'\n+++\n{{file:input.txt}}",
        );
        let selection = ResourceSelection {
            prompt_template: Some("include".into()),
            ..ResourceSelection::default()
        };
        let undeclared = load_templates(
            None,
            &[workspace.path().to_path_buf()],
            ".pawork",
            0,
            &selection,
            ResourceLimits::default(),
        );
        assert!(undeclared.selected.is_none());
        assert!(undeclared
            .diagnostics
            .issues
            .iter()
            .any(|issue| issue.code == "prompt_render_failed"));

        write_template(
            workspace.path(),
            ".pawork/prompts/include.md",
            "+++\nid='include'\nfiles=['input.txt']\n+++\n{{file:input.txt}}{{file:input.txt}}",
        );
        let bounded = load_templates(
            None,
            &[workspace.path().to_path_buf()],
            ".pawork",
            0,
            &selection,
            ResourceLimits {
                max_template_file_refs: 1,
                ..ResourceLimits::default()
            },
        );
        assert!(bounded.selected.is_none());
    }

    #[test]
    fn rendered_prompt_has_a_total_byte_limit() {
        let workspace = tempfile::tempdir().expect("workspace");
        write_template(
            workspace.path(),
            ".pawork/prompts/limited.md",
            "+++\nid='limited'\n[parameters.value]\nrequired=true\n+++\n{{value}}",
        );
        let selection = ResourceSelection {
            prompt_template: Some("limited".into()),
            prompt_arguments: BTreeMap::from([("value".into(), "0123456789".into())]),
            ..ResourceSelection::default()
        };
        let resolution = load_templates(
            None,
            &[workspace.path().to_path_buf()],
            ".pawork",
            0,
            &selection,
            ResourceLimits {
                max_rendered_prompt_bytes: 5,
                ..ResourceLimits::default()
            },
        );
        assert!(resolution.selected.is_none());
        assert!(resolution
            .diagnostics
            .issues
            .iter()
            .any(|issue| issue.code == "prompt_render_failed"));
    }
}
