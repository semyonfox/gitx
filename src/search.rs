use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    thread,
};

use serde::Serialize;

use crate::{
    catalog::{CatalogAction, CatalogCandidate, Compatibility, HostPlatform},
    catalog_config,
    catalog_local::DeviceProvider,
    catalog_providers::{self, RegistryProfile},
    display::{Style, colorize, shell_quote, terminal_safe},
    error::{GitxError, Result},
    install, npm, report,
    selector::{self, SelectionCandidate, SelectionRenderOptions},
};

/// A discovery source. `All` is only a command-line convenience: it expands to
/// the sources that make sense for the current host before any lookup starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchSource {
    All,
    Device,
    Npm,
    Bun,
    Github,
    Gitlab,
    Codeberg,
    Aur,
    Installed,
    Cache,
    Project,
    System,
    Apt,
    Pacman,
    Flatpak,
    Snap,
    Cargo,
    Python,
    NpmGlobal,
}

impl SearchSource {
    fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Device => "device",
            Self::Npm => "npm",
            Self::Bun => "bun",
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Codeberg => "codeberg",
            Self::Aur => "aur",
            Self::Installed => "installed",
            Self::Cache => "cache",
            Self::Project => "project",
            Self::System => "system",
            Self::Apt => "apt",
            Self::Pacman => "pacman",
            Self::Flatpak => "flatpak",
            Self::Snap => "snap",
            Self::Cargo => "cargo",
            Self::Python => "python",
            Self::NpmGlobal => "npm-global",
        }
    }

    fn is_remote(self) -> bool {
        matches!(
            self,
            Self::Npm | Self::Bun | Self::Github | Self::Gitlab | Self::Codeberg | Self::Aur
        )
    }

    fn device_provider(self) -> Option<DeviceProvider> {
        match self {
            Self::System => Some(DeviceProvider::System),
            Self::Apt => Some(DeviceProvider::AptCache),
            Self::Pacman => Some(DeviceProvider::PacmanCache),
            Self::Flatpak => Some(DeviceProvider::Flatpak),
            Self::Snap => Some(DeviceProvider::Snap),
            Self::Cargo => Some(DeviceProvider::Cargo),
            Self::Python => Some(DeviceProvider::Python),
            Self::NpmGlobal => Some(DeviceProvider::NpmGlobal),
            Self::All
            | Self::Device
            | Self::Npm
            | Self::Bun
            | Self::Github
            | Self::Gitlab
            | Self::Codeberg
            | Self::Aur
            | Self::Installed
            | Self::Cache
            | Self::Project => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFormat {
    Text,
    Json,
}

#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub query: String,
    pub sources: Vec<SearchSource>,
    pub project: PathBuf,
    pub limit: usize,
    pub recommend: bool,
    pub select: Option<String>,
    pub interactive: bool,
    pub details: Option<String>,
    pub offline: bool,
    pub device_only: bool,
    pub catalogs: Vec<String>,
    pub format: SearchFormat,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub schema_version: u32,
    pub query: String,
    pub host: HostPlatform,
    pub sources: Vec<SearchSource>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub catalogs: Vec<String>,
    pub results: Vec<CatalogCandidate>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// The search backend uses blocking HTTP, so each remote provider gets a
/// short-lived worker thread. Keep the bound small: a normal all-source search
/// never starts an unbounded number of network requests.
const MAX_PARALLEL_REMOTE_PROVIDERS: usize = 4;
const MAX_PARALLEL_DEVICE_PROVIDERS: usize = 4;
const SEARCH_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone)]
enum RemoteProvider {
    Npm,
    Bun(RegistryProfile),
    Github,
    Gitlab,
    Codeberg,
    Aur(HostPlatform),
}

impl RemoteProvider {
    fn source(&self) -> SearchSource {
        match self {
            Self::Npm => SearchSource::Npm,
            Self::Bun(_) => SearchSource::Bun,
            Self::Github => SearchSource::Github,
            Self::Gitlab => SearchSource::Gitlab,
            Self::Codeberg => SearchSource::Codeberg,
            Self::Aur(_) => SearchSource::Aur,
        }
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<CatalogCandidate>> {
        match self {
            Self::Npm => catalog_providers::npm_candidates(query, limit),
            Self::Bun(profile) => catalog_providers::bun_candidates(profile, query, limit),
            Self::Github => catalog_providers::github_candidates(query, limit),
            Self::Gitlab => catalog_providers::gitlab_candidates(query, limit),
            Self::Codeberg => catalog_providers::codeberg_candidates(query, limit),
            Self::Aur(host) => catalog_providers::aur_candidates(host, query, limit),
        }
    }
}

pub fn search(mut request: SearchRequest) -> Result<String> {
    let query = request.query.trim().to_string();
    if query.is_empty() || query.chars().any(char::is_control) {
        return Err(GitxError::Message(
            "search query cannot be empty or contain control characters".to_string(),
        ));
    }
    if !(1..=20).contains(&request.limit) {
        return Err(GitxError::Message(
            "search limit must be a whole number from 1 to 20".to_string(),
        ));
    }
    if request.format == SearchFormat::Json
        && (request.select.is_some() || request.interactive || request.details.is_some())
    {
        return Err(GitxError::Message(
            "--format json cannot be combined with --select, --interactive, or --details; JSON already includes complete result data"
                .to_string(),
        ));
    }
    if request.details.is_some() && (request.select.is_some() || request.interactive) {
        return Err(GitxError::Message(
            "--details cannot be combined with --select or --interactive".to_string(),
        ));
    }
    request.query = query.clone();

    let host = HostPlatform::detect();
    let (sources, default_sources, device_source_selected) =
        expand_sources(&request.sources, &host, request.device_only)?;
    report::step(format!(
        "Using {} search source(s) on {}",
        sources.len(),
        host.label()
    ));
    let mut notes = Vec::new();
    let mut candidates = collect_candidates(
        &request,
        &sources,
        &host,
        default_sources,
        device_source_selected,
        &mut notes,
    )?;
    candidates = deduplicate_candidates(candidates);
    candidates.truncate(request.limit);

    let response = SearchResponse {
        schema_version: SEARCH_SCHEMA_VERSION,
        query,
        host,
        sources,
        catalogs: request.catalogs.clone(),
        results: candidates,
        notes,
    };

    match request.format {
        SearchFormat::Text => render_text_response(&request, &response),
        SearchFormat::Json => Ok(serde_json::to_string_pretty(&response)
            .expect("search response should always serialize")),
    }
}

fn expand_sources(
    requested: &[SearchSource],
    host: &HostPlatform,
    device_only: bool,
) -> Result<(Vec<SearchSource>, bool, bool)> {
    if device_only && !requested.is_empty() {
        return Err(GitxError::Message(
            "--device cannot be combined with --source; use --source device for a local source alongside another source"
                .to_string(),
        ));
    }
    let uses_default = requested.is_empty() || requested.contains(&SearchSource::All);
    if uses_default && requested.len() > 1 {
        return Err(GitxError::Message(
            "--source all cannot be combined with another source".to_string(),
        ));
    }

    let sources = if device_only {
        device_sources(host)
    } else if uses_default {
        let mut sources = device_sources(host);
        sources.extend([
            SearchSource::Npm,
            SearchSource::Bun,
            SearchSource::Github,
            SearchSource::Gitlab,
            SearchSource::Codeberg,
        ]);
        if host.is_arch_linux() {
            sources.push(SearchSource::Aur);
        }
        sources
    } else {
        let mut sources = Vec::new();
        for source in requested {
            if *source == SearchSource::Device {
                sources.extend(device_sources(host));
            } else {
                sources.push(*source);
            }
        }
        deduplicate_sources(&sources)
    };

    Ok((
        sources,
        uses_default,
        device_only || requested.contains(&SearchSource::Device),
    ))
}

fn device_sources(host: &HostPlatform) -> Vec<SearchSource> {
    let mut sources = vec![
        SearchSource::Installed,
        SearchSource::Project,
        SearchSource::Cache,
    ];
    for (source, provider) in [
        (SearchSource::System, DeviceProvider::System),
        (SearchSource::Apt, DeviceProvider::AptCache),
        (SearchSource::Pacman, DeviceProvider::PacmanCache),
        (SearchSource::Flatpak, DeviceProvider::Flatpak),
        (SearchSource::Snap, DeviceProvider::Snap),
        (SearchSource::Cargo, DeviceProvider::Cargo),
        (SearchSource::Python, DeviceProvider::Python),
        (SearchSource::NpmGlobal, DeviceProvider::NpmGlobal),
    ] {
        if provider.available(host) {
            sources.push(source);
        }
    }
    sources
}

fn deduplicate_sources(sources: &[SearchSource]) -> Vec<SearchSource> {
    let mut seen = HashSet::new();
    sources
        .iter()
        .copied()
        .filter(|source| seen.insert(*source))
        .collect()
}

fn collect_candidates(
    request: &SearchRequest,
    sources: &[SearchSource],
    host: &HostPlatform,
    default_sources: bool,
    device_source_selected: bool,
    notes: &mut Vec<String>,
) -> Result<Vec<CatalogCandidate>> {
    let mut candidates = Vec::new();
    let explicit = !default_sources;

    if sources.contains(&SearchSource::Installed) {
        report::detail("Searching GitX managed installs");
        candidates.extend(installed_candidates(&request.query, request.limit)?);
    }
    if sources.contains(&SearchSource::Project) {
        report::detail("Searching the project command directory");
        candidates.extend(project_candidates(
            &request.project,
            &request.query,
            request.limit,
            notes,
            explicit,
        )?);
    }
    if sources.contains(&SearchSource::Cache) {
        report::detail("Searching the GitX exact-version cache");
        candidates.extend(cache_candidates(&request.query, request.limit)?);
    }
    let device_providers: Vec<_> = sources
        .iter()
        .copied()
        .filter_map(|source| source.device_provider().map(|provider| (source, provider)))
        .collect();
    if !device_providers.is_empty() {
        report::step(format!(
            "Searching {} on-device catalog(s), up to {} at a time",
            device_providers.len(),
            MAX_PARALLEL_DEVICE_PROVIDERS
        ));
    }
    candidates.extend(query_device_providers(
        device_providers,
        host,
        &request.query,
        request.limit,
        request.offline || request.device_only || device_source_selected,
        explicit,
        notes,
    )?);
    if !request.catalogs.is_empty() {
        report::step(format!(
            "Searching {} configured catalog(s)",
            request.catalogs.len()
        ));
        candidates.extend(catalog_config::search(
            &request.catalogs,
            &request.query,
            request.limit,
            request.offline || request.device_only || device_source_selected,
        )?);
    }

    let requested_remote: Vec<_> = sources
        .iter()
        .copied()
        .filter(|source| source.is_remote())
        .collect();
    if requested_remote.is_empty() {
        return Ok(candidates);
    }

    if request.offline {
        if explicit {
            let names = requested_remote
                .iter()
                .map(|source| source.label())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(GitxError::Message(format!(
                "--offline cannot search remote source(s): {names}; choose device, installed, project, cache, or another local source"
            )));
        }
        notes.push(
            "offline mode skipped npm, Bun, GitHub, GitLab, Codeberg, and AUR lookups".to_string(),
        );
        return Ok(candidates);
    }

    let providers = remote_providers(
        &requested_remote,
        &request.project,
        &request.query,
        host,
        explicit,
        notes,
    )?;
    if !providers.is_empty() {
        report::step(format!(
            "Querying {} remote source(s), up to {} at a time",
            providers.len(),
            MAX_PARALLEL_REMOTE_PROVIDERS
        ));
    }
    candidates.extend(query_remote_providers(
        providers,
        &request.query,
        request.limit,
        explicit,
        notes,
    )?);

    Ok(candidates)
}

fn query_device_providers(
    providers: Vec<(SearchSource, DeviceProvider)>,
    host: &HostPlatform,
    query: &str,
    limit: usize,
    local_only: bool,
    explicit: bool,
    notes: &mut Vec<String>,
) -> Result<Vec<CatalogCandidate>> {
    let mut candidates = Vec::new();

    for batch in providers.chunks(MAX_PARALLEL_DEVICE_PROVIDERS) {
        let handles: Vec<_> = batch
            .iter()
            .copied()
            .map(|(source, provider)| {
                let host = host.clone();
                let query = query.to_string();
                report::detail(format!("Starting {} local catalog", source.label()));
                (
                    source,
                    thread::spawn(move || provider.search(&host, &query, limit, local_only)),
                )
            })
            .collect();

        for (source, handle) in handles {
            match handle.join() {
                Ok(Ok(matches)) => {
                    report::detail(format!(
                        "{} local catalog returned {} match(es)",
                        source.label(),
                        matches.len()
                    ));
                    candidates.extend(matches);
                }
                Ok(Err(error)) if explicit => return Err(error),
                Ok(Err(error)) => notes.push(format!(
                    "{} local catalog was unavailable: {}",
                    source.label(),
                    terminal_safe(&error.to_string())
                )),
                Err(_) if explicit => {
                    return Err(GitxError::Message(format!(
                        "{} local catalog worker stopped unexpectedly",
                        source.label()
                    )));
                }
                Err(_) => notes.push(format!(
                    "{} local catalog worker stopped unexpectedly",
                    source.label()
                )),
            }
        }
    }

    Ok(candidates)
}

fn remote_providers(
    requested: &[SearchSource],
    project: &Path,
    query: &str,
    host: &HostPlatform,
    explicit: bool,
    notes: &mut Vec<String>,
) -> Result<Vec<RemoteProvider>> {
    let mut providers = Vec::new();
    let wants_npm = requested.contains(&SearchSource::Npm);

    for source in requested {
        match source {
            SearchSource::Npm => providers.push(RemoteProvider::Npm),
            SearchSource::Bun => match catalog_providers::bun_registry_for_query(project, query) {
                Ok(profile) if wants_npm && profile.url == npm::DEFAULT_REGISTRY && !explicit => {
                    notes.push(
                        "Bun uses the npm registry here; npm results are also available through bunx when Bun is installed"
                            .to_string(),
                    );
                }
                Ok(profile) => providers.push(RemoteProvider::Bun(profile)),
                Err(error) if explicit => return Err(error),
                Err(error) => notes.push(format!(
                    "skipped the configured Bun registry: {}",
                    terminal_safe(&error.to_string())
                )),
            },
            SearchSource::Github => providers.push(RemoteProvider::Github),
            SearchSource::Gitlab => providers.push(RemoteProvider::Gitlab),
            SearchSource::Codeberg => providers.push(RemoteProvider::Codeberg),
            SearchSource::Aur => providers.push(RemoteProvider::Aur(host.clone())),
            SearchSource::All
            | SearchSource::Device
            | SearchSource::Installed
            | SearchSource::Cache
            | SearchSource::Project
            | SearchSource::System
            | SearchSource::Apt
            | SearchSource::Pacman
            | SearchSource::Flatpak
            | SearchSource::Snap
            | SearchSource::Cargo
            | SearchSource::Python
            | SearchSource::NpmGlobal => {}
        }
    }

    Ok(providers)
}

fn query_remote_providers(
    providers: Vec<RemoteProvider>,
    query: &str,
    limit: usize,
    explicit: bool,
    notes: &mut Vec<String>,
) -> Result<Vec<CatalogCandidate>> {
    let mut candidates = Vec::new();

    for batch in providers.chunks(MAX_PARALLEL_REMOTE_PROVIDERS) {
        let handles: Vec<_> = batch
            .iter()
            .cloned()
            .map(|provider| {
                let query = query.to_string();
                let source = provider.source();
                report::step(format!("Querying {}", source.label()));
                (
                    source,
                    thread::spawn(move || provider.search(&query, limit)),
                )
            })
            .collect();

        for (source, handle) in handles {
            match handle.join() {
                Ok(Ok(matches)) => {
                    report::detail(format!(
                        "{} returned {} match(es)",
                        source.label(),
                        matches.len()
                    ));
                    candidates.extend(matches);
                }
                Ok(Err(error)) if explicit => return Err(error),
                Ok(Err(error)) => notes.push(format!(
                    "{} search was unavailable: {}",
                    source.label(),
                    terminal_safe(&error.to_string())
                )),
                Err(_) if explicit => {
                    return Err(GitxError::Message(format!(
                        "{} search worker stopped unexpectedly",
                        source.label()
                    )));
                }
                Err(_) => notes.push(format!(
                    "{} search worker stopped unexpectedly",
                    source.label()
                )),
            }
        }
    }

    Ok(candidates)
}

fn installed_candidates(query: &str, limit: usize) -> Result<Vec<CatalogCandidate>> {
    let runtime_compatibility = if cfg!(unix) {
        Compatibility::compatible("managed GitX shims are supported on this host", Vec::new())
    } else {
        Compatibility::incompatible(
            "managed GitX shims are currently supported only on Unix-like hosts",
            vec!["Unix-like host".to_string()],
            Vec::new(),
        )
    };

    install::search(query, limit).map(|matches| {
        matches
            .into_iter()
            .map(|tool| {
                let action = if let Some(package) = tool.pinned.strip_prefix("npm:") {
                    CatalogAction::new(
                        "Run",
                        format!("gitx exec {} -- <args>", shell_quote(package)),
                    )
                } else {
                    CatalogAction::new(
                        "Run",
                        format!("gitx run {} -- <args>", shell_quote(&tool.pinned)),
                    )
                };
                let mut information = BTreeMap::new();
                information.insert("Pinned".to_string(), tool.pinned.clone());
                information.insert("Kind".to_string(), tool.kind.clone());
                if let Some(runtime) = &tool.runtime {
                    information.insert("Runtime".to_string(), runtime.clone());
                }
                if let Some(entrypoint) = &tool.entrypoint {
                    information.insert("Entrypoint".to_string(), entrypoint.clone());
                }
                if let Some(license) = &tool.license {
                    information.insert("License".to_string(), license.clone());
                }
                if !tool.binaries.is_empty() {
                    information.insert("Binaries".to_string(), tool.binaries.join(", "));
                }
                for (key, value) in &tool.metadata {
                    information.insert(format!("Metadata.{key}"), value.clone());
                }
                CatalogCandidate {
                    source: "GitX install".to_string(),
                    origin: "GitX managed install record on this device".to_string(),
                    name: tool.name,
                    version: None,
                    description: tool
                        .description
                        .or_else(|| Some(format!("{} tool pinned by GitX", tool.kind))),
                    homepage: tool.homepage,
                    updated_at: None,
                    compatibility: Compatibility {
                        status: runtime_compatibility.status,
                        summary: runtime_compatibility.summary.clone(),
                        platforms: tool.platforms,
                        requirements: tool.requirements,
                    },
                    information,
                    action,
                }
            })
            .collect()
    })
}

fn cache_candidates(query: &str, limit: usize) -> Result<Vec<CatalogCandidate>> {
    npm::search_cached(query, limit).map(|matches| {
        matches
            .into_iter()
            .map(|package| {
                let mut information = BTreeMap::new();
                information.insert(
                    "Cache".to_string(),
                    "GitX exact-version metadata".to_string(),
                );
                if let Some(license) = &package.license {
                    information.insert("License".to_string(), license.clone());
                }
                if !package.bins.is_empty() {
                    information.insert("Binaries".to_string(), package.bins.join(", "));
                }
                CatalogCandidate {
                    source: "GitX package cache".to_string(),
                    origin: "GitX exact-version package metadata cache on this device".to_string(),
                    name: package.name.clone(),
                    version: Some(package.version.clone()),
                    description: package.description,
                    homepage: package.homepage,
                    updated_at: None,
                    compatibility: package.compatibility,
                    information,
                    action: CatalogAction::new(
                        "Inspect",
                        format!(
                            "gitx inspect {}",
                            shell_quote(&format!("npm:{}@{}", package.name, package.version))
                        ),
                    ),
                }
            })
            .collect()
    })
}

fn project_candidates(
    project: &Path,
    query: &str,
    limit: usize,
    notes: &mut Vec<String>,
    required: bool,
) -> Result<Vec<CatalogCandidate>> {
    if !project.is_dir() {
        if required {
            return Err(GitxError::Message(format!(
                "project directory {} does not exist or is not a directory",
                project.display()
            )));
        }
        return Ok(Vec::new());
    }

    let bin_dir = project.join("node_modules").join(".bin");
    if !bin_dir.exists() {
        return Ok(Vec::new());
    }
    let metadata = fs::symlink_metadata(&bin_dir).map_err(|source| GitxError::Io {
        context: format!("inspecting project bin directory {}", bin_dir.display()),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        if required {
            return Err(GitxError::Message(format!(
                "refusing unsafe project bin directory {}",
                bin_dir.display()
            )));
        }
        notes.push(format!(
            "skipped unsafe project bin directory {}",
            terminal_safe(&bin_dir.display().to_string())
        ));
        return Ok(Vec::new());
    }

    let normalized = query.trim().to_ascii_lowercase();
    let mut candidates = Vec::new();
    for entry in fs::read_dir(&bin_dir).map_err(|source| GitxError::Io {
        context: format!("reading project bin directory {}", bin_dir.display()),
        source,
    })? {
        let entry = entry.map_err(|source| GitxError::Io {
            context: format!("reading project bin entry in {}", bin_dir.display()),
            source,
        })?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.is_empty()
            || name.chars().any(char::is_control)
            || !name.to_ascii_lowercase().contains(&normalized)
        {
            continue;
        }
        let path = entry.path();
        let mut information = BTreeMap::new();
        information.insert("Path".to_string(), path.display().to_string());
        candidates.push(CatalogCandidate {
            source: "project bin".to_string(),
            origin: format!("project directory {}", project.display()),
            name,
            version: None,
            description: Some(path.display().to_string()),
            homepage: None,
            updated_at: None,
            compatibility: Compatibility::compatible(
                "available from this project's node_modules/.bin",
                Vec::new(),
            ),
            information,
            action: CatalogAction::new(
                "Run",
                format!("{} -- <args>", shell_quote(&path.display().to_string())),
            ),
        });
    }
    candidates.sort_by(|left, right| left.name.cmp(&right.name));
    candidates.truncate(limit);
    Ok(candidates)
}

/// Keep the first source's result when it would produce the same next command.
/// Local results intentionally precede remote catalog results.
fn deduplicate_candidates(candidates: Vec<CatalogCandidate>) -> Vec<CatalogCandidate> {
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| {
            seen.insert((
                candidate.action.verb.clone(),
                candidate.action.command.clone(),
            ))
        })
        .collect()
}

fn recommended_index(candidates: &[CatalogCandidate], query: &str) -> usize {
    candidates
        .iter()
        .position(|candidate| candidate.name.eq_ignore_ascii_case(query.trim()))
        .unwrap_or(0)
}

fn render_text_response(request: &SearchRequest, response: &SearchResponse) -> Result<String> {
    if response.results.is_empty() {
        let mut output = vec![format!(
            "{} No matches for {}.",
            colorize(Style::Accent, "::"),
            colorize(
                Style::Package,
                format!("`{}`", terminal_safe(&response.query))
            )
        )];
        output.extend(response.notes.iter().map(|note| {
            format!(
                "{} {}",
                colorize(Style::Unknown, "note"),
                terminal_safe(note)
            )
        }));
        return Ok(output.join("\n"));
    }

    let recommendation = request
        .recommend
        .then(|| recommended_index(&response.results, &response.query));
    let display_candidates: Vec<_> = response.results.iter().map(selection_candidate).collect();
    let rendered = selector::render_candidates(
        &display_candidates,
        SelectionRenderOptions {
            recommended_index: recommendation,
        },
    )
    .expect("a non-empty search candidate list is always renderable");

    let result_word = if response.results.len() == 1 {
        "result"
    } else {
        "results"
    };
    let mut output = vec![
        format!(
            "{} {} {} for {} {}",
            colorize(Style::Accent, "::"),
            colorize(Style::Heading, response.results.len().to_string()),
            result_word,
            colorize(
                Style::Package,
                format!("`{}`", terminal_safe(&response.query))
            ),
            colorize(
                Style::Muted,
                format!("on {}", terminal_safe(&response.host.label()))
            )
        ),
        colorize(Style::Muted, "   package  version  [source]  compatibility"),
        String::new(),
        rendered,
    ];
    output.extend(response.notes.iter().map(|note| {
        format!(
            "{} {}",
            colorize(Style::Unknown, "note"),
            terminal_safe(note)
        )
    }));

    if let Some(details) = &request.details {
        let index = one_detail_index(details, response.results.len())?;
        output.push(String::new());
        output.push(response.results[index].render_detail(index + 1, &response.host));
        return Ok(output.join("\n"));
    }

    let selection = selection_for_request(request, response.results.len())?;
    if selection.is_empty() {
        output.push(String::new());
        output.push(format!(
            "{} `--details 1` shows the full card. `--select 1` prints a command. `--interactive` opens the picker.",
            colorize(Style::Hint, "hint")
        ));
    } else {
        output.push(String::new());
        output.push(format!(
            "{} selected actions",
            colorize(Style::Accent, "::")
        ));
        for index in selection {
            let action = &response.results[index].action;
            output.push(format!(
                "  {}  {}",
                colorize(Style::Label, terminal_safe(&action.verb)),
                terminal_safe(&action.command)
            ));
        }
    }

    Ok(output.join("\n"))
}

fn selection_candidate(candidate: &CatalogCandidate) -> SelectionCandidate {
    let mut display = SelectionCandidate::new(&candidate.name)
        .with_source(&candidate.source)
        .with_origin(&candidate.origin)
        .with_compatibility(candidate.compatibility.status);
    if let Some(version) = &candidate.version {
        display = display.with_version(version);
    }
    if let Some(description) = &candidate.description {
        display = display.with_description(description);
    }
    display
}

fn one_detail_index(selection: &str, candidate_count: usize) -> Result<usize> {
    let indices = selector::parse_selection(selection, candidate_count)
        .map_err(|error| GitxError::Message(error.to_string()))?;
    if indices.len() != 1 {
        return Err(GitxError::Message(
            "--details accepts exactly one result number".to_string(),
        ));
    }
    Ok(indices[0])
}

fn selection_for_request(request: &SearchRequest, candidate_count: usize) -> Result<Vec<usize>> {
    if let Some(selection) = &request.select {
        return selector::parse_selection(selection, candidate_count)
            .map_err(|error| GitxError::Message(error.to_string()));
    }
    if !request.interactive {
        return Ok(Vec::new());
    }
    if !io::stdin().is_terminal() {
        return Err(GitxError::Message(
            "--interactive requires a terminal; use --select <selection> in scripts".to_string(),
        ));
    }

    eprint!("Select result(s), or press Enter to cancel: ");
    io::stderr().flush().map_err(|source| GitxError::Io {
        context: "flushing search selection prompt".to_string(),
        source,
    })?;
    let mut selection = String::new();
    io::stdin()
        .read_line(&mut selection)
        .map_err(|source| GitxError::Io {
            context: "reading search selection prompt".to_string(),
            source,
        })?;
    if selection.trim().is_empty() {
        return Ok(Vec::new());
    }

    selector::parse_selection(&selection, candidate_count)
        .map_err(|error| GitxError::Message(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use super::{
        CatalogAction, CatalogCandidate, Compatibility, SearchFormat, SearchRequest, SearchSource,
        deduplicate_candidates, one_detail_index, recommended_index, search,
    };

    fn candidate(name: &str, action: &str) -> CatalogCandidate {
        CatalogCandidate {
            source: "test".to_string(),
            origin: "test fixture".to_string(),
            name: name.to_string(),
            version: None,
            description: None,
            homepage: None,
            updated_at: None,
            compatibility: Compatibility::unknown("test candidate"),
            information: BTreeMap::new(),
            action: CatalogAction::new("Inspect", action),
        }
    }

    #[test]
    fn exact_name_wins_the_recommendation() {
        let candidates = vec![
            candidate("pretty-quick", "one"),
            candidate("prettier", "two"),
        ];

        assert_eq!(recommended_index(&candidates, "prettier"), 1);
    }

    #[test]
    fn duplicate_next_actions_keep_the_highest_priority_result() {
        let candidates = vec![
            candidate("cached-demo", "gitx inspect npm:demo@1.0.0"),
            candidate("registry-demo", "gitx inspect npm:demo@1.0.0"),
        ];

        let candidates = deduplicate_candidates(candidates);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name, "cached-demo");
    }

    #[test]
    fn details_accepts_one_result_only() {
        assert_eq!(one_detail_index("2", 3).unwrap(), 1);
        assert!(one_detail_index("1-2", 3).is_err());
    }

    #[test]
    fn library_search_rejects_an_out_of_range_limit_before_lookup() {
        let error = search(SearchRequest {
            query: "demo".to_string(),
            sources: vec![SearchSource::Npm],
            project: PathBuf::from("."),
            limit: 0,
            recommend: false,
            select: None,
            interactive: false,
            details: None,
            offline: false,
            device_only: false,
            catalogs: Vec::new(),
            format: SearchFormat::Text,
        })
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("search limit must be a whole number from 1 to 20")
        );
    }
}
