use std::path::PathBuf;

use serde::Serialize;

use crate::{
    cache::CachePaths,
    catalog::{Compatibility, CompatibilityStatus},
    display::terminal_safe,
    error::{GitxError, Result},
    git,
    manifest::{MANIFEST_FILE, Manifest, Permissions, resolve_entry},
    npm, report,
    source::{RefSpec, Source, SourceKind},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectReport {
    pub schema_version: u32,
    pub source: String,
    pub source_type: SourceKind,
    pub resolved_remote: Option<String>,
    pub requested_ref: Option<String>,
    pub resolved_commit: Option<String>,
    pub package: Option<String>,
    pub package_version: Option<String>,
    pub manifest_path: Option<String>,
    pub manifest_found: bool,
    pub tool_name: Option<String>,
    pub entrypoint: Option<String>,
    pub runtime: Option<String>,
    pub permissions: Permissions,
    pub permission_enforcement: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<Compatibility>,
    pub warnings: Vec<String>,
    pub risk_level: String,
    pub policy_decision: String,
    pub requires_confirmation: bool,
}

pub fn inspect_source(input: &str, json: bool) -> Result<String> {
    let report = build_report(input)?;

    if json {
        return Ok(
            serde_json::to_string_pretty(&report).expect("inspect report should always serialize")
        );
    }

    Ok(render_human(&report))
}

pub fn build_report(input: &str) -> Result<InspectReport> {
    let source = Source::parse(input)?;
    let mut warnings = source_warnings(&source);
    let mut resolved_commit = source.resolved_commit().map(str::to_string);
    let mut package = source.npm_package().map(str::to_string);
    let mut package_version = source.npm_version().map(str::to_string);
    let mut tool_name = None;
    let mut entrypoint = None;
    let mut runtime = None;
    let mut has_deny_findings = false;
    let mut compatibility = None;

    let (manifest, manifest_path, manifest_found) = match source.kind() {
        SourceKind::Local => {
            report::step("Reading the local GitX manifest");
            let path = source.local_path().expect("local source has a path");
            let manifest_path = path.join(MANIFEST_FILE);
            let manifest = Manifest::from_path(&manifest_path)?;
            resolve_entry(path, &manifest.entry)?;
            (Some(manifest), Some(display_path(manifest_path)), true)
        }
        SourceKind::Github | SourceKind::Git => {
            report::step("Resolving the Git source for inspection");
            let paths = CachePaths::discover();
            let checkout = paths
                .ensure_base_dirs()
                .and_then(|()| git::checkout_source(&paths, &source));

            match checkout {
                Ok(checkout) => {
                    resolved_commit = Some(checkout.commit.clone());
                    match Manifest::from_path(checkout.dir.join(MANIFEST_FILE)) {
                        Ok(manifest) => {
                            resolve_entry(&checkout.dir, &manifest.entry)?;
                            (Some(manifest), Some(MANIFEST_FILE.to_string()), true)
                        }
                        Err(GitxError::ManifestMissing { .. }) => {
                            warnings.push(
                                "No gitx.toml manifest found at the resolved commit.".to_string(),
                            );
                            (None, Some(MANIFEST_FILE.to_string()), false)
                        }
                        Err(error) => return Err(error),
                    }
                }
                Err(error) => {
                    warnings.push(format!("Could not resolve remote source: {error}"));
                    (None, Some(MANIFEST_FILE.to_string()), false)
                }
            }
        }
        SourceKind::Npm => {
            report::step("Resolving npm package metadata for inspection");
            match npm::inspect_package_spec(source.raw()) {
                Ok(inspection) => {
                    package = Some(inspection.package.clone());
                    package_version = Some(inspection.version.clone());
                    tool_name = Some(inspection.package.clone());
                    runtime = Some("node".to_string());
                    entrypoint = inspection.bin.clone();
                    has_deny_findings = inspection.has_deny_findings
                        || inspection.compatibility.status == CompatibilityStatus::Incompatible;
                    if inspection.compatibility.status == CompatibilityStatus::Incompatible {
                        warnings.push(format!(
                            "Package is incompatible with this host: {}",
                            inspection.compatibility.summary
                        ));
                    }
                    compatibility = Some(inspection.compatibility);
                    warnings.extend(inspection.warnings);
                }
                Err(error) => {
                    warnings.push(format!("Could not resolve npm package: {error}"));
                }
            }
            (None, None, false)
        }
    };

    if let Some(manifest) = &manifest {
        warnings.extend(manifest_warnings(manifest));
        tool_name = Some(manifest.name.clone());
        entrypoint = Some(manifest.entry.clone());
        runtime = Some(manifest.runtime.clone());
    }

    let permissions = manifest
        .as_ref()
        .map(|manifest| manifest.permissions.clone())
        .unwrap_or_default();
    let risk_level = if has_deny_findings {
        "high"
    } else {
        risk_level(&warnings)
    };
    let policy_decision = if has_deny_findings {
        "deny"
    } else if warnings.is_empty() {
        "allow"
    } else {
        "ask"
    };

    Ok(InspectReport {
        schema_version: 1,
        source: source.raw().to_string(),
        source_type: source.kind(),
        resolved_remote: source.resolved_remote().map(str::to_string),
        requested_ref: source.requested_ref().map(str::to_string),
        resolved_commit,
        package,
        package_version,
        manifest_path,
        manifest_found,
        tool_name,
        entrypoint,
        runtime,
        permissions,
        permission_enforcement: "none".to_string(),
        compatibility,
        warnings,
        risk_level: risk_level.to_string(),
        policy_decision: policy_decision.to_string(),
        requires_confirmation: policy_decision == "ask",
    })
}

fn source_warnings(source: &Source) -> Vec<String> {
    let mut warnings = Vec::new();

    match source.reference() {
        Some(RefSpec::Named(name)) => {
            warnings.push(format!(
                "Ref `{name}` is mutable; it may point at a different commit later."
            ));
        }
        Some(RefSpec::Commit(_)) => {}
        None if matches!(source.kind(), SourceKind::Github | SourceKind::Git) => {
            warnings.push("Source is not pinned to a ref or commit.".to_string());
        }
        None => {}
    }

    warnings.extend(transport_warnings(source));

    warnings
}

pub(crate) fn transport_warnings(source: &Source) -> Vec<String> {
    match source.resolved_remote() {
        Some(remote) if remote.starts_with("http://") || remote.starts_with("git://") => {
            vec!["Git source uses an unencrypted transport; prefer HTTPS or SSH.".to_string()]
        }
        _ => Vec::new(),
    }
}

pub(crate) fn manifest_warnings(manifest: &Manifest) -> Vec<String> {
    let mut warnings = Vec::new();

    if !matches!(
        manifest.runtime.as_str(),
        "node" | "bun" | "deno" | "python" | "shell"
    ) {
        warnings.push(format!("Runtime `{}` is not recognized.", manifest.runtime));
    }

    if manifest.runtime == "shell" {
        warnings.push("Shell entrypoints are high risk.".to_string());
    }

    if manifest.permissions.network {
        warnings.push("Manifest requests network access.".to_string());
    }

    if manifest.permissions.subprocess {
        warnings.push("Manifest may spawn subprocesses.".to_string());
    }

    for variable in &manifest.permissions.env {
        if is_sensitive_env(variable) {
            warnings.push(format!("Manifest requests sensitive env var `{variable}`."));
        }
    }

    for path in &manifest.permissions.write {
        if path.starts_with('/') || path.contains("..") {
            warnings.push(format!(
                "Manifest requests write access outside a simple workspace path: `{path}`."
            ));
        }
    }

    warnings
}

fn is_sensitive_env(variable: &str) -> bool {
    let upper = variable.to_ascii_uppercase();
    ["TOKEN", "SECRET", "PASSWORD", "PASS", "KEY", "CREDENTIAL"]
        .iter()
        .any(|needle| upper.contains(needle))
}

pub(crate) fn risk_level(warnings: &[String]) -> &'static str {
    if warnings.iter().any(|warning| {
        warning.contains("high risk")
            || warning.contains("sensitive env")
            || warning.contains("outside a simple workspace")
    }) {
        "medium"
    } else if warnings.is_empty() {
        "low"
    } else {
        "unknown"
    }
}

fn render_human(report: &InspectReport) -> String {
    let mut output = Vec::new();

    output.push(format!("Source: {}", terminal_safe(&report.source)));
    let source_type = format!("{:?}", report.source_type).to_lowercase();
    output.push(format!("Source type: {source_type}"));

    if let Some(remote) = &report.resolved_remote {
        output.push(format!("Remote: {}", terminal_safe(remote)));
    }

    if let Some(package) = &report.package {
        output.push(format!("Package: {}", terminal_safe(package)));
    }

    if let Some(version) = &report.package_version {
        output.push(format!("Package version: {}", terminal_safe(version)));
    }

    output.push(format!(
        "Ref: {}",
        terminal_safe(report.requested_ref.as_deref().unwrap_or("unresolved"))
    ));
    output.push(format!(
        "Commit: {}",
        terminal_safe(report.resolved_commit.as_deref().unwrap_or("unresolved"))
    ));

    if let Some(manifest_path) = &report.manifest_path {
        output.push(format!(
            "Manifest: {} ({})",
            terminal_safe(manifest_path),
            if report.manifest_found {
                "found"
            } else {
                "unresolved"
            }
        ));
    } else {
        output.push("Manifest: unresolved".to_string());
    }

    if let Some(tool_name) = &report.tool_name {
        output.push(format!("Tool: {}", terminal_safe(tool_name)));
    }

    output.push(format!(
        "Runtime: {}",
        terminal_safe(report.runtime.as_deref().unwrap_or("unresolved"))
    ));
    output.push(format!(
        "Entrypoint: {}",
        terminal_safe(report.entrypoint.as_deref().unwrap_or("unresolved"))
    ));

    if let Some(compatibility) = &report.compatibility {
        output.push(format!(
            "Compatibility: {} ({})",
            compatibility.status.label(),
            terminal_safe(&compatibility.summary)
        ));
        if !compatibility.platforms.is_empty() {
            output.push(format!(
                "Platform constraints: {}",
                compatibility
                    .platforms
                    .iter()
                    .map(|platform| terminal_safe(platform))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !compatibility.requirements.is_empty() {
            output.push(format!(
                "Runtime requirements: {}",
                compatibility
                    .requirements
                    .iter()
                    .map(|requirement| terminal_safe(requirement))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    output.push(format!(
        "Declared permissions (enforcement: {}):",
        report.permission_enforcement
    ));
    output.push(format!("  network: {}", report.permissions.network));
    output.push(format!(
        "  read: {}",
        display_list(&report.permissions.read)
    ));
    output.push(format!(
        "  write: {}",
        display_list(&report.permissions.write)
    ));
    output.push(format!("  env: {}", display_list(&report.permissions.env)));
    output.push(format!("  subprocess: {}", report.permissions.subprocess));
    output.push(format!("Risk: {}", report.risk_level));
    output.push(format!("Policy: {}", report.policy_decision));

    if report.warnings.is_empty() {
        output.push("Warnings: none".to_string());
    } else {
        output.push("Warnings:".to_string());
        output.extend(
            report
                .warnings
                .iter()
                .map(|warning| format!("  - {}", terminal_safe(warning))),
        );
    }

    output.join("\n")
}

fn display_list(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}

fn display_path(path: PathBuf) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{manifest_warnings, transport_warnings};
    use crate::manifest::{Manifest, Permissions};
    use crate::source::Source;

    #[test]
    fn deno_is_a_recognized_runtime() {
        let manifest = Manifest {
            schema: 1,
            name: "demo".to_string(),
            description: None,
            runtime: "deno".to_string(),
            entry: "main.ts".to_string(),
            permissions: Permissions::default(),
            metadata: BTreeMap::new(),
        };

        assert!(manifest_warnings(&manifest).is_empty());
    }

    #[test]
    fn warns_for_unencrypted_git_transports() {
        let source = Source::parse("http://example.test/tool.git").unwrap();

        assert_eq!(transport_warnings(&source).len(), 1);
    }
}
