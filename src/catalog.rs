use std::{collections::BTreeMap, env, fs};

use serde::Serialize;

use crate::display::{Style, colorize, terminal_safe};

/// The machine GitX is evaluating candidates for. Providers use this to label
/// results that cannot work on the current platform instead of hiding them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPlatform {
    pub os: String,
    pub arch: String,
    pub distribution: Option<String>,
}

impl HostPlatform {
    pub fn detect() -> Self {
        Self {
            os: env::consts::OS.to_string(),
            arch: env::consts::ARCH.to_string(),
            distribution: linux_distribution(),
        }
    }

    pub fn is_arch_linux(&self) -> bool {
        self.os == "linux"
            && self
                .distribution
                .as_deref()
                .is_some_and(is_arch_distribution)
    }

    pub fn is_macos(&self) -> bool {
        self.os == "macos"
    }

    pub fn is_windows(&self) -> bool {
        self.os == "windows"
    }

    pub fn label(&self) -> String {
        match &self.distribution {
            Some(distribution) => format!("{distribution} {} ({})", self.os, self.arch),
            None => format!("{} ({})", self.os, self.arch),
        }
    }
}

fn linux_distribution() -> Option<String> {
    if env::consts::OS != "linux" {
        return None;
    }

    let contents = fs::read_to_string("/etc/os-release").ok()?;
    linux_distribution_from_os_release(&contents)
}

fn linux_distribution_from_os_release(contents: &str) -> Option<String> {
    let id = os_release_value(contents, "ID")?;
    if is_arch_distribution(&id) {
        return Some(id);
    }

    if os_release_value(contents, "ID_LIKE")
        .is_some_and(|value| value.split_whitespace().any(is_arch_distribution))
    {
        return Some("arch".to_string());
    }

    Some(id)
}

fn os_release_value(contents: &str, key: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let value = line.strip_prefix(&format!("{key}="))?;
        let value = value.trim_matches('"').trim();
        (!value.is_empty()).then(|| value.to_ascii_lowercase())
    })
}

fn is_arch_distribution(distribution: &str) -> bool {
    matches!(
        distribution,
        "arch"
            | "archlinux"
            | "manjaro"
            | "endeavouros"
            | "garuda"
            | "cachyos"
            | "artix"
            | "arcolinux"
            | "parabola"
            | "rebornos"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CompatibilityStatus {
    Compatible,
    Incompatible,
    Unknown,
}

impl CompatibilityStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Compatible => "compatible",
            Self::Incompatible => "incompatible",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Compatibility {
    pub status: CompatibilityStatus,
    pub summary: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub platforms: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub requirements: Vec<String>,
}

impl Compatibility {
    pub fn compatible(summary: impl Into<String>, requirements: Vec<String>) -> Self {
        Self {
            status: CompatibilityStatus::Compatible,
            summary: summary.into(),
            platforms: Vec::new(),
            requirements,
        }
    }

    pub fn incompatible(
        summary: impl Into<String>,
        platforms: Vec<String>,
        requirements: Vec<String>,
    ) -> Self {
        Self {
            status: CompatibilityStatus::Incompatible,
            summary: summary.into(),
            platforms,
            requirements,
        }
    }

    pub fn unknown(summary: impl Into<String>) -> Self {
        Self {
            status: CompatibilityStatus::Unknown,
            summary: summary.into(),
            platforms: Vec::new(),
            requirements: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogAction {
    pub verb: String,
    pub command: String,
}

impl CatalogAction {
    pub fn new(verb: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            verb: verb.into(),
            command: command.into(),
        }
    }
}

/// A normalized result from a package registry, Git host, local project, or
/// operating-system catalog. The fields intentionally describe what GitX knows
/// now; unknown compatibility is shown as unknown rather than guessed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogCandidate {
    /// The adapter category that produced this record, such as `npm registry`
    /// or `Flatpak installed app`.
    pub source: String,
    /// The concrete registry, cache, local database, or configuration origin.
    /// This is required so a result never looks source-less in text or JSON.
    pub origin: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    pub compatibility: Compatibility,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub information: BTreeMap<String, String>,
    pub action: CatalogAction,
}

impl CatalogCandidate {
    pub fn label(&self) -> String {
        match &self.version {
            Some(version) => format!("{}@{}", self.name, version),
            None => self.name.clone(),
        }
    }

    pub fn render_detail(&self, number: usize, host: &HostPlatform) -> String {
        let mut output = vec![format!(
            "{} {}",
            colorize(Style::Accent, format!("{number}.")),
            colorize(Style::Package, terminal_safe(&self.label()))
        )];
        output.push(detail_row(Style::Source, "Source", &self.source));
        output.push(detail_row(Style::Muted, "Origin", &self.origin));
        if let Some(description) = &self.description {
            output.push(detail_row(Style::Label, "Description", description));
        }
        if let Some(homepage) = &self.homepage {
            output.push(detail_row(Style::Label, "Homepage", homepage));
        }
        if let Some(updated_at) = &self.updated_at {
            output.push(detail_row(Style::Label, "Updated", updated_at));
        }
        output.push(format!(
            "{} {}: {} ({})",
            colorize(Style::Label, "Compatibility on"),
            host.label(),
            self.compatibility.status.label(),
            terminal_safe(&self.compatibility.summary)
        ));
        if !self.compatibility.platforms.is_empty() {
            output.push(format!(
                "{} {}",
                colorize(Style::Label, "Platforms:"),
                self.compatibility
                    .platforms
                    .iter()
                    .map(|platform| terminal_safe(platform))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.compatibility.requirements.is_empty() {
            output.push(format!(
                "{} {}",
                colorize(Style::Label, "Requires:"),
                self.compatibility
                    .requirements
                    .iter()
                    .map(|requirement| terminal_safe(requirement))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for (key, value) in &self.information {
            output.push(format!(
                "{} {}",
                colorize(Style::Label, format!("{}:", terminal_safe(key))),
                terminal_safe(value)
            ));
        }
        output.push(format!(
            "{} {}",
            colorize(
                Style::Label,
                format!("{}:", terminal_safe(&self.action.verb))
            ),
            terminal_safe(&self.action.command)
        ));
        output.join("\n")
    }
}

fn detail_row(style: Style, label: &str, value: &str) -> String {
    format!(
        "{} {}",
        colorize(style, format!("{label}:")),
        terminal_safe(value)
    )
}

#[cfg(test)]
mod tests {
    use super::{
        Compatibility, CompatibilityStatus, HostPlatform, linux_distribution_from_os_release,
    };

    #[test]
    fn arch_compatibility_requires_linux_and_a_known_arch_distribution() {
        assert!(
            HostPlatform {
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                distribution: Some("arch".to_string()),
            }
            .is_arch_linux()
        );
        assert!(
            !HostPlatform {
                os: "macos".to_string(),
                arch: "aarch64".to_string(),
                distribution: None,
            }
            .is_arch_linux()
        );
    }

    #[test]
    fn compatibility_status_is_serializable_and_human_readable() {
        let compatibility = Compatibility::compatible("bunx is available", vec!["bun".into()]);
        assert_eq!(compatibility.status, CompatibilityStatus::Compatible);
        assert_eq!(compatibility.status.label(), "compatible");
    }

    #[test]
    fn recognizes_arch_derivatives_from_id_like() {
        assert_eq!(
            linux_distribution_from_os_release("ID=custom\nID_LIKE=arch linux\n"),
            Some("arch".to_string())
        );
        assert_eq!(
            linux_distribution_from_os_release("ID=ubuntu\nID_LIKE=debian\n"),
            Some("ubuntu".to_string())
        );
    }
}
