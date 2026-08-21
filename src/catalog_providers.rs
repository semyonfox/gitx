use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{
    catalog::{CatalogAction, CatalogCandidate, Compatibility, HostPlatform},
    display::{shell_quote, terminal_safe},
    error::{GitxError, Result},
    http, npm, toolchain,
};

const GITHUB_API: &str = "https://api.github.com";
const GITLAB_API: &str = "https://gitlab.com/api/v4";
const CODEBERG_API: &str = "https://codeberg.org/api/v1";
const AUR_RPC: &str = "https://aur.archlinux.org/rpc/v5";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryProfile {
    pub url: String,
    pub origin: String,
}

impl RegistryProfile {
    pub fn display_url(&self) -> String {
        redact_registry_url(&self.url)
    }
}

/// Read the registry that Bun would use for a query without loading code,
/// resolving credentials, or printing a token. This follows Bun's documented
/// source order: global/project `.npmrc`, global/project `bunfig.toml`, then
/// `BUN_CONFIG_REGISTRY` or `NPM_CONFIG_REGISTRY`. Scoped registries win for a
/// matching `@scope/package` query.
///
/// Search uses the selected endpoint only for discovery. GitX deliberately
/// leaves registry credentials to Bun and does not represent a private-registry
/// package as runnable by `gitx exec` until execution has a registry-scoped,
/// integrity-safe resolution path of its own.
pub fn bun_registry_for_query(project: &Path, query: &str) -> Result<RegistryProfile> {
    let mut registries = RegistrySelection::default();

    for path in npmrc_paths(project) {
        apply_registry_file(&mut registries, &path, RegistryFileKind::Npmrc)?;
    }
    for path in bunfig_paths(project) {
        apply_registry_file(&mut registries, &path, RegistryFileKind::Bunfig)?;
    }

    if let Some((variable, url)) = ["BUN_CONFIG_REGISTRY", "NPM_CONFIG_REGISTRY"]
        .into_iter()
        .find_map(|variable| env::var(variable).ok().map(|url| (variable, url)))
    {
        registries.default = RegistryProfile {
            url: validate_registry_url(&url)?,
            origin: variable.to_string(),
        };
    }

    let scope = package_scope(query);
    Ok(scope
        .as_deref()
        .and_then(|scope| registries.scopes.get(scope))
        .cloned()
        .unwrap_or(registries.default))
}

#[derive(Debug, Clone)]
struct RegistrySelection {
    default: RegistryProfile,
    scopes: BTreeMap<String, RegistryProfile>,
}

impl Default for RegistrySelection {
    fn default() -> Self {
        Self {
            default: RegistryProfile {
                url: npm::DEFAULT_REGISTRY.to_string(),
                origin: "Bun default".to_string(),
            },
            scopes: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum RegistryFileKind {
    Npmrc,
    Bunfig,
}

fn apply_registry_file(
    selection: &mut RegistrySelection,
    path: &Path,
    kind: RegistryFileKind,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let contents = fs::read_to_string(path).map_err(|source| GitxError::Io {
        context: format!("reading registry configuration {}", path.display()),
        source,
    })?;
    let values = match kind {
        RegistryFileKind::Npmrc => registry_values_from_npmrc(&contents),
        RegistryFileKind::Bunfig => registry_values_from_bunfig(&contents, path)?,
    };

    if let Some(url) = values.default {
        selection.default = RegistryProfile {
            url: validate_registry_url(&url)?,
            origin: path.display().to_string(),
        };
    }
    for (scope, url) in values.scopes {
        let scope = normalize_scope(&scope).ok_or_else(|| {
            GitxError::Message(format!(
                "invalid scoped registry name `{scope}` in {}",
                path.display()
            ))
        })?;
        selection.scopes.insert(
            scope.clone(),
            RegistryProfile {
                url: validate_registry_url(&url)?,
                origin: format!("{} (scope @{scope})", path.display()),
            },
        );
    }

    Ok(())
}

pub fn npm_candidates(query: &str, limit: usize) -> Result<Vec<CatalogCandidate>> {
    registry_candidates(npm::DEFAULT_REGISTRY, "npm registry", "npm", query, limit)
}

pub fn bun_candidates(
    profile: &RegistryProfile,
    query: &str,
    limit: usize,
) -> Result<Vec<CatalogCandidate>> {
    let source = format!("Bun registry ({})", profile.display_url());
    let mut candidates = registry_candidates(&profile.url, &source, "bunx", query, limit)?;
    for candidate in &mut candidates {
        candidate
            .information
            .insert("Registry configuration".to_string(), profile.origin.clone());
    }
    Ok(candidates)
}

fn registry_candidates(
    registry: &str,
    source: &str,
    preferred_runner: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<CatalogCandidate>> {
    let available_runners: Vec<_> = ["bunx", "npm", "pnpm"]
        .into_iter()
        .filter(|runner| toolchain::exists_on_path(runner))
        .map(str::to_string)
        .collect();
    let compatibility = if preferred_runner == "bunx"
        && !available_runners.iter().any(|runner| runner == "bunx")
    {
        Compatibility::incompatible(
            "Bun is not available on PATH, so this configured Bun registry cannot be used through bunx",
            Vec::new(),
            vec!["bunx".to_string()],
        )
    } else if available_runners.is_empty() {
        Compatibility::incompatible(
            "no supported package runner is available on PATH",
            Vec::new(),
            vec![preferred_runner.to_string()],
        )
    } else {
        Compatibility::unknown(format!(
            "package OS and CPU metadata needs an exact package inspection; available runners: {}",
            available_runners.join(", ")
        ))
    };

    npm::search_registry_at(registry, query, limit).map(|matches| {
        matches
            .into_iter()
            .map(|package| {
                let mut information = BTreeMap::new();
                if let Some(score) = package.registry_score {
                    information.insert("Registry score".to_string(), format!("{score:.3}"));
                }
                if let Some(license) = &package.license {
                    information.insert("License".to_string(), license.clone());
                }
                if !package.bins.is_empty() {
                    information.insert("Binaries".to_string(), package.bins.join(", "));
                }
                information.insert("Preferred runner".to_string(), preferred_runner.to_string());
                let action = if preferred_runner == "bunx" {
                    let package_spec = format!("{}@{}", package.name, package.version);
                    CatalogAction::new(
                        "Run",
                        format!("bunx {} -- <args>", shell_quote(&package_spec)),
                    )
                } else {
                    let package_spec = format!("npm:{}@{}", package.name, package.version);
                    CatalogAction::new(
                        "Inspect",
                        format!("gitx inspect {}", shell_quote(&package_spec)),
                    )
                };
                CatalogCandidate {
                    source: source.to_string(),
                    origin: format!("package registry: {}", redact_registry_url(registry)),
                    name: package.name.clone(),
                    version: Some(package.version.clone()),
                    description: package.description,
                    homepage: package.homepage,
                    updated_at: None,
                    compatibility: compatibility.clone(),
                    information,
                    action,
                }
            })
            .collect()
    })
}

pub fn github_candidates(query: &str, limit: usize) -> Result<Vec<CatalogCandidate>> {
    let client = http::search_client()?;
    let base = env::var("GITX_GITHUB_API").unwrap_or_else(|_| GITHUB_API.to_string());
    let origin = format!("GitHub API: {}", base.trim_end_matches('/'));
    let url = format!(
        "{}/search/repositories?q={}&per_page={limit}",
        base.trim_end_matches('/'),
        urlencoding::encode(query)
    );
    let mut request = client
        .get(url)
        .header("Accept", "application/vnd.github+json");
    if let Ok(token) = env::var("GITHUB_TOKEN") {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .map_err(|source| GitxError::Http {
            context: format!("searching GitHub repositories for `{query}`"),
            source,
        })?
        .error_for_status()
        .map_err(|source| GitxError::Http {
            context: format!("searching GitHub repositories for `{query}`"),
            source,
        })?
        .json::<GitHubSearchResponse>()
        .map_err(|source| GitxError::Http {
            context: format!("decoding GitHub repository search results for `{query}`"),
            source,
        })?;

    Ok(response
        .items
        .into_iter()
        .map(|repository| {
            let mut information = BTreeMap::new();
            information.insert("Stars".to_string(), repository.stargazers_count.to_string());
            information.insert("Forks".to_string(), repository.forks_count.to_string());
            if let Some(language) = repository.language {
                information.insert("Language".to_string(), language);
            }
            if repository.archived {
                information.insert("Archived".to_string(), "yes".to_string());
            }
            CatalogCandidate {
                source: "GitHub repository".to_string(),
                origin: origin.clone(),
                name: repository.full_name.clone(),
                version: repository.default_branch.clone(),
                description: repository.description,
                homepage: Some(repository.html_url),
                updated_at: repository.updated_at,
                compatibility: Compatibility::unknown(
                    "inspect gitx.toml before GitX can determine runtime and host compatibility",
                ),
                information,
                action: CatalogAction::new(
                    "Inspect",
                    format!(
                        "gitx inspect {}",
                        shell_quote(&format!(
                            "github:{}@{}",
                            repository.full_name,
                            repository.default_branch.as_deref().unwrap_or("HEAD")
                        ))
                    ),
                ),
            }
        })
        .collect())
}

pub fn gitlab_candidates(query: &str, limit: usize) -> Result<Vec<CatalogCandidate>> {
    let client = http::search_client()?;
    let base = env::var("GITX_GITLAB_API").unwrap_or_else(|_| GITLAB_API.to_string());
    let origin = format!("GitLab API: {}", base.trim_end_matches('/'));
    let url = format!(
        "{}/projects?search={}&simple=true&per_page={limit}&order_by=star_count&sort=desc",
        base.trim_end_matches('/'),
        urlencoding::encode(query)
    );
    let mut request = client.get(url);
    if let Ok(token) = env::var("GITLAB_TOKEN") {
        request = request.header("PRIVATE-TOKEN", token);
    }
    let response = request
        .send()
        .map_err(|source| GitxError::Http {
            context: format!("searching GitLab repositories for `{query}`"),
            source,
        })?
        .error_for_status()
        .map_err(|source| GitxError::Http {
            context: format!("searching GitLab repositories for `{query}`"),
            source,
        })?
        .json::<Vec<GitLabProject>>()
        .map_err(|source| GitxError::Http {
            context: format!("decoding GitLab repository search results for `{query}`"),
            source,
        })?;

    Ok(response
        .into_iter()
        .map(|project| {
            let mut information = BTreeMap::new();
            information.insert("Stars".to_string(), project.star_count.to_string());
            information.insert("Forks".to_string(), project.forks_count.to_string());
            if project.archived {
                information.insert("Archived".to_string(), "yes".to_string());
            }
            CatalogCandidate {
                source: "GitLab repository".to_string(),
                origin: origin.clone(),
                name: project.path_with_namespace.clone(),
                version: project.default_branch.clone(),
                description: project.description,
                homepage: Some(project.web_url),
                updated_at: project.last_activity_at,
                compatibility: Compatibility::unknown(
                    "inspect gitx.toml before GitX can determine runtime and host compatibility",
                ),
                information,
                action: CatalogAction::new(
                    "Inspect",
                    format!(
                        "gitx inspect {}",
                        shell_quote(&format!(
                            "gitlab:{}@{}",
                            project.path_with_namespace,
                            project.default_branch.as_deref().unwrap_or("HEAD")
                        ))
                    ),
                ),
            }
        })
        .collect())
}

pub fn codeberg_candidates(query: &str, limit: usize) -> Result<Vec<CatalogCandidate>> {
    let client = http::search_client()?;
    let base = env::var("GITX_CODEBERG_API").unwrap_or_else(|_| CODEBERG_API.to_string());
    let origin = format!("Codeberg API: {}", base.trim_end_matches('/'));
    let url = format!(
        "{}/repos/search?q={}&limit={limit}",
        base.trim_end_matches('/'),
        urlencoding::encode(query)
    );
    let mut request = client.get(url);
    if let Ok(token) = env::var("CODEBERG_TOKEN") {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .map_err(|source| GitxError::Http {
            context: format!("searching Codeberg repositories for `{query}`"),
            source,
        })?
        .error_for_status()
        .map_err(|source| GitxError::Http {
            context: format!("searching Codeberg repositories for `{query}`"),
            source,
        })?
        .json::<CodebergSearchResponse>()
        .map_err(|source| GitxError::Http {
            context: format!("decoding Codeberg repository search results for `{query}`"),
            source,
        })?;

    let mut repositories = response.data;
    if repositories.is_empty() {
        repositories = response.items;
    }
    Ok(repositories
        .into_iter()
        .map(|repository| {
            let mut information = BTreeMap::new();
            information.insert("Stars".to_string(), repository.stars_count.to_string());
            information.insert("Forks".to_string(), repository.forks_count.to_string());
            if let Some(language) = repository.language {
                information.insert("Language".to_string(), language);
            }
            if repository.archived {
                information.insert("Archived".to_string(), "yes".to_string());
            }
            CatalogCandidate {
                source: "Codeberg repository".to_string(),
                origin: origin.clone(),
                name: repository.full_name.clone(),
                version: repository.default_branch.clone(),
                description: repository.description,
                homepage: Some(repository.html_url),
                updated_at: repository.updated_at,
                compatibility: Compatibility::unknown(
                    "inspect gitx.toml before GitX can determine runtime and host compatibility",
                ),
                information,
                action: CatalogAction::new(
                    "Inspect",
                    format!(
                        "gitx inspect {}",
                        shell_quote(&format!(
                            "codeberg:{}@{}",
                            repository.full_name,
                            repository.default_branch.as_deref().unwrap_or("HEAD")
                        ))
                    ),
                ),
            }
        })
        .collect())
}

pub fn aur_candidates(
    host: &HostPlatform,
    query: &str,
    limit: usize,
) -> Result<Vec<CatalogCandidate>> {
    let client = http::search_client()?;
    let base = env::var("GITX_AUR_RPC").unwrap_or_else(|_| AUR_RPC.to_string());
    let origin = format!("AUR RPC: {}", base.trim_end_matches('/'));
    let url = format!(
        "{}/search/{}?by=name-desc",
        base.trim_end_matches('/'),
        urlencoding::encode(query)
    );
    let response = client
        .get(url)
        .send()
        .map_err(|source| GitxError::Http {
            context: format!("searching the AUR for `{query}`"),
            source,
        })?
        .error_for_status()
        .map_err(|source| GitxError::Http {
            context: format!("searching the AUR for `{query}`"),
            source,
        })?
        .json::<AurResponse>()
        .map_err(|source| GitxError::Http {
            context: format!("decoding AUR search results for `{query}`"),
            source,
        })?;
    if response.response_type == "error" {
        return Err(GitxError::Message(format!(
            "AUR search failed: {}",
            response
                .error
                .as_deref()
                .unwrap_or("the AUR returned an unspecified error")
        )));
    }

    let compatibility = if host.is_arch_linux() {
        Compatibility::compatible(
            "AUR package for the current Arch Linux host; review the PKGBUILD before building",
            vec!["pacman and an AUR helper, or a manual PKGBUILD build".to_string()],
        )
    } else {
        Compatibility::incompatible(
            "AUR packages target Arch Linux and its derivatives",
            vec!["Arch Linux".to_string()],
            vec!["pacman and an AUR helper, or a manual PKGBUILD build".to_string()],
        )
    };

    let mut results: Vec<_> = response
        .results
        .into_iter()
        .map(|package| {
            let mut information = BTreeMap::new();
            if let Some(votes) = package.num_votes {
                information.insert("Votes".to_string(), votes.to_string());
            }
            if let Some(popularity) = package.popularity {
                information.insert("Popularity".to_string(), format!("{popularity:.3}"));
            }
            if package.out_of_date.is_some() {
                information.insert("Out of date".to_string(), "yes".to_string());
            }
            if let Some(maintainer) = package.maintainer {
                information.insert("Maintainer".to_string(), maintainer);
            }
            CatalogCandidate {
                source: "Arch User Repository".to_string(),
                origin: origin.clone(),
                name: package.name.clone(),
                version: Some(package.version),
                description: package.description,
                homepage: package.url,
                updated_at: package
                    .last_modified
                    .map(|timestamp| format!("Unix {timestamp}")),
                compatibility: compatibility.clone(),
                information,
                action: CatalogAction::new(
                    "Open",
                    format!(
                        "https://aur.archlinux.org/packages/{}",
                        urlencoding::encode(&package.name)
                    ),
                ),
            }
        })
        .collect();
    results.truncate(limit);
    Ok(results)
}

fn bunfig_paths(project: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(config_home) = env::var("XDG_CONFIG_HOME") {
        paths.push(PathBuf::from(config_home).join(".bunfig.toml"));
    }
    if let Ok(home) = env::var("HOME") {
        paths.push(PathBuf::from(home).join(".bunfig.toml"));
    }
    paths.push(project.join("bunfig.toml"));
    paths
}

fn npmrc_paths(project: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(config_home) = env::var("XDG_CONFIG_HOME") {
        paths.push(PathBuf::from(config_home).join(".npmrc"));
    }
    if let Ok(home) = env::var("HOME") {
        paths.push(PathBuf::from(home).join(".npmrc"));
    }
    paths.push(project.join(".npmrc"));
    paths
}

#[derive(Debug, Default)]
struct RegistryValues {
    default: Option<String>,
    scopes: BTreeMap<String, String>,
}

#[cfg(test)]
fn registry_from_bunfig(contents: &str, path: &Path) -> Result<Option<String>> {
    registry_values_from_bunfig(contents, path)?
        .default
        .map(|url| validate_registry_url(&url))
        .transpose()
}

fn registry_values_from_bunfig(contents: &str, path: &Path) -> Result<RegistryValues> {
    let value =
        toml::from_str::<toml::Value>(contents).map_err(|source| GitxError::ParseManifest {
            path: path.to_path_buf(),
            source,
        })?;
    let Some(install) = value.get("install").and_then(toml::Value::as_table) else {
        return Ok(RegistryValues::default());
    };

    let default = install
        .get("registry")
        .map(|registry| registry_url_from_toml(registry, path))
        .transpose()?;
    let mut scopes = BTreeMap::new();
    if let Some(scope_values) = install.get("scopes").and_then(toml::Value::as_table) {
        for (scope, registry) in scope_values {
            scopes.insert(scope.to_string(), registry_url_from_toml(registry, path)?);
        }
    }

    Ok(RegistryValues { default, scopes })
}

fn registry_url_from_toml(registry: &toml::Value, path: &Path) -> Result<String> {
    registry
        .as_str()
        .map(str::to_string)
        .or_else(|| {
            registry
                .as_table()
                .and_then(|registry| registry.get("url"))
                .and_then(toml::Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| {
            GitxError::Message(format!(
                "Bun registry in {} must be a URL string or a table with `url`",
                path.display()
            ))
        })
}

#[cfg(test)]
fn registry_from_npmrc(contents: &str) -> Option<String> {
    registry_values_from_npmrc(contents).default
}

fn registry_values_from_npmrc(contents: &str) -> RegistryValues {
    let mut values = RegistryValues::default();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if key == "registry" {
            values.default = Some(value.to_string());
        } else if let Some(scope) = key.strip_suffix(":registry") {
            values.scopes.insert(scope.to_string(), value.to_string());
        }
    }
    values
}

fn package_scope(query: &str) -> Option<String> {
    let query = query.trim();
    let scope = query.strip_prefix('@')?.split('/').next()?;
    normalize_scope(scope)
}

fn normalize_scope(scope: &str) -> Option<String> {
    let scope = scope.trim().trim_start_matches('@');
    (!scope.is_empty() && !scope.contains('/') && !scope.chars().any(char::is_control))
        .then(|| scope.to_string())
}

fn validate_registry_url(input: &str) -> Result<String> {
    let parsed = reqwest::Url::parse(input.trim()).map_err(|_| {
        GitxError::Message(format!(
            "invalid package registry URL `{}`",
            terminal_safe(input)
        ))
    })?;
    if !matches!(parsed.scheme(), "https" | "http") {
        return Err(GitxError::Message(format!(
            "package registry URL must use HTTP or HTTPS: {}",
            terminal_safe(input)
        )));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(GitxError::Message(
            "registry URLs with embedded credentials are not supported; use the package manager's credential configuration".to_string(),
        ));
    }
    Ok(input.trim_end_matches('/').to_string())
}

fn redact_registry_url(input: &str) -> String {
    reqwest::Url::parse(input)
        .map(|url| {
            let host = url.host_str().unwrap_or("unknown");
            match url.port() {
                Some(port) => format!("{}://{host}:{port}", url.scheme()),
                None => format!("{}://{host}", url.scheme()),
            }
        })
        .unwrap_or_else(|_| "configured registry".to_string())
}

#[derive(Debug, Deserialize)]
struct GitHubSearchResponse {
    items: Vec<GitHubRepository>,
}

#[derive(Debug, Deserialize)]
struct GitHubRepository {
    full_name: String,
    description: Option<String>,
    html_url: String,
    default_branch: Option<String>,
    updated_at: Option<String>,
    stargazers_count: u64,
    forks_count: u64,
    language: Option<String>,
    archived: bool,
}

#[derive(Debug, Deserialize)]
struct GitLabProject {
    path_with_namespace: String,
    description: Option<String>,
    web_url: String,
    default_branch: Option<String>,
    last_activity_at: Option<String>,
    #[serde(default)]
    star_count: u64,
    #[serde(default)]
    forks_count: u64,
    #[serde(default)]
    archived: bool,
}

#[derive(Debug, Deserialize)]
struct CodebergSearchResponse {
    #[serde(default)]
    data: Vec<CodebergRepository>,
    #[serde(default)]
    items: Vec<CodebergRepository>,
}

#[derive(Debug, Deserialize)]
struct CodebergRepository {
    full_name: String,
    description: Option<String>,
    html_url: String,
    default_branch: Option<String>,
    updated_at: Option<String>,
    #[serde(default)]
    stars_count: u64,
    #[serde(default)]
    forks_count: u64,
    language: Option<String>,
    #[serde(default)]
    archived: bool,
}

#[derive(Debug, Deserialize)]
struct AurResponse {
    #[serde(rename = "type")]
    response_type: String,
    error: Option<String>,
    results: Vec<AurPackage>,
}

#[derive(Debug, Deserialize)]
struct AurPackage {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Version")]
    version: String,
    #[serde(rename = "Description")]
    description: Option<String>,
    #[serde(rename = "URL")]
    url: Option<String>,
    #[serde(rename = "NumVotes")]
    num_votes: Option<u64>,
    #[serde(rename = "Popularity")]
    popularity: Option<f64>,
    #[serde(rename = "OutOfDate")]
    out_of_date: Option<u64>,
    #[serde(rename = "Maintainer")]
    maintainer: Option<String>,
    #[serde(rename = "LastModified")]
    last_modified: Option<u64>,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        package_scope, registry_from_bunfig, registry_from_npmrc, registry_values_from_bunfig,
        registry_values_from_npmrc, validate_registry_url,
    };

    #[test]
    fn reads_a_bun_registry_url_from_a_string_or_table() {
        assert_eq!(
            registry_from_bunfig(
                "[install]\nregistry = \"https://packages.example.test/\"",
                Path::new("bunfig.toml")
            )
            .unwrap(),
            Some("https://packages.example.test".to_string())
        );
        assert_eq!(
            registry_from_bunfig(
                "[install]\nregistry = { url = \"https://packages.example.test\" }",
                Path::new("bunfig.toml")
            )
            .unwrap(),
            Some("https://packages.example.test".to_string())
        );
    }

    #[test]
    fn reads_project_npmrc_registry_and_refuses_credentials() {
        assert_eq!(
            registry_from_npmrc("registry=https://registry.example.test/\n"),
            Some("https://registry.example.test/".to_string())
        );
        assert!(validate_registry_url("https://user:secret@example.test").is_err());
    }

    #[test]
    fn retains_scoped_bun_and_npmrc_registries_without_reading_credentials() {
        let bunfig = registry_values_from_bunfig(
            "[install]\nregistry = \"https://registry.example.test\"\n[install.scopes]\nmyorg = { url = \"https://packages.example.test\", token = \"$TOKEN\" }\n",
            Path::new("bunfig.toml"),
        )
        .unwrap();
        assert_eq!(
            bunfig.scopes.get("myorg"),
            Some(&"https://packages.example.test".to_string())
        );

        let npmrc = registry_values_from_npmrc(
            "registry=https://registry.example.test\n@myorg:registry=https://packages.example.test\n//packages.example.test/:_authToken=${TOKEN}\n",
        );
        assert_eq!(
            npmrc.scopes.get("@myorg"),
            Some(&"https://packages.example.test".to_string())
        );
        assert_eq!(package_scope("@myorg/tool"), Some("myorg".to_string()));
    }
}
