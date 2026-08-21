use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    cache::CachePaths,
    catalog::{Compatibility, CompatibilityStatus, HostPlatform},
    display::terminal_safe,
    error::{GitxError, Result},
    http, prompt, report,
    scanner::{self, NpmPackageScanInput},
    toolchain,
};

pub const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageBackend {
    Auto,
    Bun,
    Npm,
    Pnpm,
}

#[derive(Debug, Clone)]
pub struct PackageRunRequest {
    pub target: String,
    pub packages: Vec<String>,
    pub args: Vec<String>,
    pub backend: PackageBackend,
    pub yes: bool,
    pub allow_scripts: bool,
    pub allow_dangerous_scripts: bool,
    pub preview: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSpec {
    pub name: String,
    pub requested: Option<String>,
}

/// A package candidate available from the public registry or GitX's local
/// exact-version cache. Search never materializes or executes this package.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageSearchMatch {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub bins: Vec<String>,
    pub platforms: Vec<String>,
    pub requirements: Vec<String>,
    pub compatibility: Compatibility,
    pub registry_score: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    objects: Vec<SearchObject>,
}

#[derive(Debug, Deserialize)]
struct SearchObject {
    package: SearchPackage,
    score: SearchScore,
}

#[derive(Debug, Deserialize)]
struct SearchPackage {
    name: String,
    version: String,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchScore {
    #[serde(rename = "final")]
    final_score: f64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PackageMetadata {
    #[serde(rename = "dist-tags")]
    dist_tags: HashMap<String, String>,
    versions: BTreeMap<String, PackageVersion>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PackageVersion {
    name: String,
    version: String,
    description: Option<String>,
    homepage: Option<serde_json::Value>,
    license: Option<serde_json::Value>,
    bin: Option<BinField>,
    os: Option<Vec<String>>,
    cpu: Option<Vec<String>>,
    engines: Option<BTreeMap<String, String>>,
    keywords: Option<Vec<String>>,
    scripts: Option<HashMap<String, String>>,
    dependencies: Option<HashMap<String, String>>,
    #[serde(rename = "optionalDependencies")]
    optional_dependencies: Option<HashMap<String, String>>,
    #[serde(rename = "peerDependencies")]
    peer_dependencies: Option<HashMap<String, String>>,
    dist: PackageDist,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PackageDist {
    tarball: Option<String>,
    integrity: Option<String>,
    shasum: Option<String>,
    #[serde(rename = "fileCount")]
    file_count: Option<u64>,
    #[serde(rename = "unpackedSize")]
    unpacked_size: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
enum BinField {
    Path(String),
    Map(BTreeMap<String, String>),
}

#[derive(Debug, Clone)]
struct ResolvedPackage {
    spec: PackageSpec,
    version: PackageVersion,
    requested_was_mutable: bool,
    cache_status: ResolutionCacheStatus,
}

#[derive(Debug, Clone)]
enum ResolutionCacheStatus {
    ExactHit,
    ExactFetched,
    TagResolved {
        tag: String,
        version: String,
    },
    TagRefreshed {
        tag: String,
        version: String,
    },
    TagMoved {
        tag: String,
        previous: String,
        current: String,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PackageVersionCache {
    package: String,
    version: String,
    cached_at: u64,
    metadata: PackageVersion,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PackageTagCache {
    package: String,
    cached_at: u64,
    tags: HashMap<String, String>,
}

pub fn search_registry(query: &str, limit: usize) -> Result<Vec<PackageSearchMatch>> {
    search_registry_at(DEFAULT_REGISTRY, query, limit)
}

/// Search any npm-compatible registry that implements the standard v1 search
/// endpoint. Callers must keep cache and execution policy separate when they
/// use a non-default registry.
pub fn search_registry_at(
    registry: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<PackageSearchMatch>> {
    let client = http::search_client()?;
    let registry = registry.trim_end_matches('/');
    let url = format!(
        "{registry}/-/v1/search?text={}&size={limit}&quality=0.25&popularity=0.55&maintenance=0.20",
        urlencoding::encode(query),
    );

    let response = client
        .get(url)
        .send()
        .map_err(|source| GitxError::Http {
            context: format!("searching package registry {registry} for `{query}`"),
            source,
        })?
        .error_for_status()
        .map_err(|source| GitxError::Http {
            context: format!("searching package registry {registry} for `{query}`"),
            source,
        })?
        .json::<SearchResponse>()
        .map_err(|source| GitxError::Http {
            context: format!(
                "decoding package-registry search results from {registry} for `{query}`"
            ),
            source,
        })?;

    Ok(response
        .objects
        .into_iter()
        .map(|object| PackageSearchMatch {
            name: object.package.name,
            version: object.package.version,
            description: object.package.description,
            homepage: None,
            license: None,
            bins: Vec::new(),
            platforms: Vec::new(),
            requirements: Vec::new(),
            compatibility: Compatibility::unknown(
                "inspect the exact package to determine its OS, CPU, and runtime requirements",
            ),
            registry_score: Some(object.score.final_score),
        })
        .collect())
}

/// Return exact package versions already known to GitX without contacting the
/// registry. Corrupt or unsafe cache entries are ignored rather than trusted.
pub fn search_cached(query: &str, limit: usize) -> Result<Vec<PackageSearchMatch>> {
    let paths = CachePaths::discover();
    if !paths.has_safe_npm_version_cache()? {
        return Ok(Vec::new());
    }

    let normalized = query.trim().to_ascii_lowercase();
    let host = HostPlatform::detect();
    let mut matches = Vec::new();
    let versions_dir = paths.npm_versions_dir();
    for package_entry in fs::read_dir(&versions_dir).map_err(|source| GitxError::Io {
        context: format!("reading npm version cache {}", versions_dir.display()),
        source,
    })? {
        let package_entry = package_entry.map_err(|source| GitxError::Io {
            context: format!(
                "reading npm version cache entry in {}",
                versions_dir.display()
            ),
            source,
        })?;
        let package_type = package_entry.file_type().map_err(|source| GitxError::Io {
            context: format!(
                "inspecting npm version cache entry {}",
                package_entry.path().display()
            ),
            source,
        })?;
        if !package_type.is_dir() || package_type.is_symlink() {
            continue;
        }

        for version_entry in fs::read_dir(package_entry.path()).map_err(|source| GitxError::Io {
            context: format!(
                "reading npm package cache {}",
                package_entry.path().display()
            ),
            source,
        })? {
            let version_entry = version_entry.map_err(|source| GitxError::Io {
                context: format!(
                    "reading npm package cache entry in {}",
                    package_entry.path().display()
                ),
                source,
            })?;
            let version_type = version_entry.file_type().map_err(|source| GitxError::Io {
                context: format!(
                    "inspecting npm package cache entry {}",
                    version_entry.path().display()
                ),
                source,
            })?;
            if !version_type.is_file() || version_type.is_symlink() {
                continue;
            }

            let Ok(body) = fs::read_to_string(version_entry.path()) else {
                continue;
            };
            let Ok(cache) = serde_json::from_str::<PackageVersionCache>(&body) else {
                continue;
            };
            if cache.package != cache.metadata.name
                || cache.version != cache.metadata.version
                || !package_matches(&cache.metadata, &normalized)
            {
                continue;
            }

            let metadata = cache.metadata;
            matches.push(PackageSearchMatch {
                name: metadata.name.clone(),
                version: metadata.version.clone(),
                description: metadata.description.clone(),
                homepage: metadata_text(metadata.homepage.as_ref()),
                license: metadata_text(metadata.license.as_ref()),
                bins: bin_names(&metadata),
                platforms: platform_constraints(&metadata),
                requirements: engine_requirements(&metadata),
                compatibility: package_compatibility(&metadata, &host),
                registry_score: None,
            });
        }
    }

    matches.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.version.cmp(&right.version))
    });
    matches.truncate(limit);
    Ok(matches)
}

pub fn run_package(request: PackageRunRequest) -> Result<()> {
    let paths = CachePaths::discover();
    paths.ensure_base_dirs()?;

    let package_specs = package_specs_for_request(&request)?;
    report::step(format!(
        "Resolving {} package specification(s)",
        package_specs.len()
    ));
    let mut resolved = Vec::new();
    for spec in package_specs {
        resolved.push(resolve_package(&paths, &spec)?);
    }

    let backend = choose_backend(&request)?;
    report::step(format!("Using {} for package execution", backend.label()));
    let command_name = command_name_for_request(&request, &resolved)?;
    validate_command_name(&command_name)?;
    let has_deny_findings = has_deny_scan_findings(&resolved);
    let incompatible_packages = incompatible_package_names(&resolved);
    let warnings = package_warnings(
        &resolved,
        backend,
        request.allow_scripts,
        request.allow_dangerous_scripts,
    );

    report::step("Checking package metadata and lifecycle policy");
    print_preflight(
        &resolved,
        PackagePreflight {
            backend,
            paths: &paths,
            command_name: &command_name,
            warnings: &warnings,
            has_deny_findings,
            incompatible_packages: &incompatible_packages,
            allow_dangerous_scripts: request.allow_dangerous_scripts,
        },
    );
    if request.preview {
        return Ok(());
    }

    if !incompatible_packages.is_empty() {
        return Err(GitxError::Message(format!(
            "refusing to run package(s) that exclude this host: {}",
            incompatible_packages.join(", ")
        )));
    }
    if has_deny_findings && !request.allow_dangerous_scripts {
        return Err(GitxError::Message(
            "scanner produced deny-level findings; review the preflight and rerun with --allow-scripts --allow-dangerous-scripts to override"
                .to_string(),
        ));
    }
    prompt::confirm_if_needed(&warnings, request.yes)?;
    report::step(format!(
        "Handing off download and execution to {}",
        backend.label()
    ));

    match backend {
        PackageBackend::Npm => run_with_npm(&paths, &resolved, &command_name, &request),
        PackageBackend::Bun => run_with_bun(&paths, &resolved, &command_name, &request),
        PackageBackend::Pnpm => run_with_pnpm(&paths, &resolved, &command_name, &request),
        PackageBackend::Auto => unreachable!("choose_backend returns a concrete backend"),
    }
}

impl PackageBackend {
    fn label(self) -> &'static str {
        match self {
            Self::Auto => "the automatic backend selector",
            Self::Bun => "bunx",
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
        }
    }
}

/// Resolved package facts for inspect and install flows that need
/// metadata without executing anything.
#[derive(Debug, Clone)]
pub struct PackageInspection {
    pub package: String,
    pub version: String,
    pub bin: Option<String>,
    pub bins: Vec<String>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub platforms: Vec<String>,
    pub requirements: Vec<String>,
    pub compatibility: Compatibility,
    pub integrity: Option<String>,
    pub was_mutable: bool,
    pub has_deny_findings: bool,
    pub warnings: Vec<String>,
}

pub fn inspect_package_spec(input: &str) -> Result<PackageInspection> {
    let paths = CachePaths::discover();
    paths.ensure_base_dirs()?;

    let spec = parse_package_spec(input)?;
    let resolved = resolve_package(&paths, &spec)?;
    let bin = infer_bin_name(&resolved.version, None).ok();
    let bins = bin_names(&resolved.version);
    let resolved_packages = std::slice::from_ref(&resolved);
    let warnings = metadata_warnings(resolved_packages);
    let has_deny_findings = has_deny_scan_findings(resolved_packages);

    Ok(PackageInspection {
        package: resolved.version.name.clone(),
        version: resolved.version.version.clone(),
        bin,
        bins,
        description: resolved.version.description.clone(),
        homepage: metadata_text(resolved.version.homepage.as_ref()),
        license: metadata_text(resolved.version.license.as_ref()),
        platforms: platform_constraints(&resolved.version),
        requirements: engine_requirements(&resolved.version),
        compatibility: package_compatibility(&resolved.version, &HostPlatform::detect()),
        integrity: resolved.version.dist.integrity.clone(),
        was_mutable: resolved.requested_was_mutable,
        has_deny_findings,
        warnings,
    })
}

pub fn parse_package_spec(input: &str) -> Result<PackageSpec> {
    let input = input.strip_prefix("npm:").unwrap_or(input);

    if input.is_empty() || input.chars().any(char::is_control) {
        return Err(GitxError::NpmResolution(
            "package spec cannot be empty or contain control characters".to_string(),
        ));
    }

    let version_separator = input.rfind('@').filter(|index| *index > 0);
    let (name, requested) = match version_separator {
        Some(index) => {
            let (name, requested_with_at) = input.split_at(index);
            let requested = requested_with_at.trim_start_matches('@');
            if requested.is_empty() {
                return Err(GitxError::NpmResolution(format!(
                    "package spec `{input}` has an empty version"
                )));
            }
            (name, Some(requested.to_string()))
        }
        None => (input, None),
    };

    if name.is_empty() {
        return Err(GitxError::NpmResolution(format!(
            "package spec `{input}` has an empty name"
        )));
    }

    if name.starts_with('-') {
        return Err(GitxError::NpmResolution(
            "package names cannot start with `-`".to_string(),
        ));
    }

    if name.starts_with('@') {
        let Some((scope, package)) = name.split_once('/') else {
            return Err(GitxError::NpmResolution(format!(
                "scoped package spec `{input}` must look like @scope/name"
            )));
        };
        if scope.len() <= 1 || package.is_empty() {
            return Err(GitxError::NpmResolution(format!(
                "scoped package spec `{input}` must look like @scope/name"
            )));
        }
    }

    Ok(PackageSpec {
        name: name.to_string(),
        requested,
    })
}

fn resolve_package(paths: &CachePaths, spec: &PackageSpec) -> Result<ResolvedPackage> {
    let requested = spec.requested.as_deref().unwrap_or("latest");
    report::step(format!("Resolving npm package {}@{requested}", spec.name));

    if spec.requested.is_some()
        && let Some(version) = read_version_cache(paths, &spec.name, requested)?
    {
        report::detail(format!(
            "Using cached exact metadata for {}@{requested}",
            spec.name
        ));
        validate_package_version(&version, &spec.name, requested)?;
        return Ok(ResolvedPackage {
            spec: spec.clone(),
            version,
            requested_was_mutable: false,
            cache_status: ResolutionCacheStatus::ExactHit,
        });
    }

    if spec.requested.is_some()
        && looks_like_exact_version(requested)
        && let Some(version) = fetch_exact_version(&spec.name, requested)?
    {
        report::detail(format!(
            "Fetched exact metadata for {}@{requested}",
            spec.name
        ));
        validate_package_version(&version, &spec.name, requested)?;
        write_version_cache(paths, &version)?;
        return Ok(ResolvedPackage {
            spec: spec.clone(),
            version,
            requested_was_mutable: false,
            cache_status: ResolutionCacheStatus::ExactFetched,
        });
    }

    let previous_tags = read_tag_cache(paths, &spec.name)?;
    let previous_tag_target = previous_tags
        .as_ref()
        .and_then(|cache| cache.tags.get(requested))
        .cloned();
    let metadata = fetch_metadata(paths, &spec.name)?;

    let (resolved_version, requested_was_mutable) = if let Some(version) =
        metadata.dist_tags.get(requested)
    {
        (version.clone(), true)
    } else if metadata.versions.contains_key(requested) {
        (requested.to_string(), false)
    } else {
        return Err(GitxError::NpmResolution(format!(
            "`{}` does not have version or dist-tag `{requested}`; npm semver ranges are not implemented yet",
            spec.name
        )));
    };

    let version = metadata
        .versions
        .get(&resolved_version)
        .cloned()
        .ok_or_else(|| {
            GitxError::NpmResolution(format!(
                "metadata for `{}` points to missing version `{resolved_version}`",
                spec.name
            ))
        })?;
    validate_package_version(&version, &spec.name, &resolved_version)?;

    let cache_status = if requested_was_mutable {
        write_tag_cache(paths, &spec.name, &metadata.dist_tags)?;

        match previous_tag_target {
            Some(previous) if previous == version.version => ResolutionCacheStatus::TagRefreshed {
                tag: requested.to_string(),
                version: version.version.clone(),
            },
            Some(previous) => ResolutionCacheStatus::TagMoved {
                tag: requested.to_string(),
                previous,
                current: version.version.clone(),
            },
            None => ResolutionCacheStatus::TagResolved {
                tag: requested.to_string(),
                version: version.version.clone(),
            },
        }
    } else {
        ResolutionCacheStatus::ExactFetched
    };

    write_version_cache(paths, &version)?;

    Ok(ResolvedPackage {
        spec: spec.clone(),
        version,
        requested_was_mutable,
        cache_status,
    })
}

fn fetch_metadata(paths: &CachePaths, package: &str) -> Result<PackageMetadata> {
    report::detail(format!("Fetching npm metadata for {package}"));
    let client = http::client()?;
    let encoded = encode_package_name(package);
    let url = format!("{DEFAULT_REGISTRY}/{encoded}");
    let body = client
        .get(url)
        .header("Accept", "application/vnd.npm.install-v1+json")
        .send()
        .map_err(|source| GitxError::Http {
            context: format!("fetching npm metadata for `{package}`"),
            source,
        })?
        .error_for_status()
        .map_err(|source| GitxError::Http {
            context: format!("fetching npm metadata for `{package}`"),
            source,
        })?
        .text()
        .map_err(|source| GitxError::Http {
            context: format!("reading npm metadata for `{package}`"),
            source,
        })?;

    write_metadata_cache(paths, package, &body)?;

    serde_json::from_str(&body).map_err(|source| GitxError::Json {
        context: format!("decoding npm metadata for `{package}`"),
        source,
    })
}

fn fetch_exact_version(package: &str, version: &str) -> Result<Option<PackageVersion>> {
    report::detail(format!("Fetching npm metadata for {package}@{version}"));
    let client = http::client()?;
    let encoded_package = encode_package_name(package);
    let encoded_version = urlencoding::encode(version);
    let url = format!("{DEFAULT_REGISTRY}/{encoded_package}/{encoded_version}");
    let response = client.get(url).send().map_err(|source| GitxError::Http {
        context: format!("fetching npm metadata for `{package}@{version}`"),
        source,
    })?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }

    let body = response
        .error_for_status()
        .map_err(|source| GitxError::Http {
            context: format!("fetching npm metadata for `{package}@{version}`"),
            source,
        })?
        .text()
        .map_err(|source| GitxError::Http {
            context: format!("reading npm metadata for `{package}@{version}`"),
            source,
        })?;

    serde_json::from_str(&body)
        .map(Some)
        .map_err(|source| GitxError::Json {
            context: format!("decoding npm metadata for `{package}@{version}`"),
            source,
        })
}

fn write_metadata_cache(paths: &CachePaths, package: &str, body: &str) -> Result<()> {
    paths.ensure_base_dirs()?;
    let path = paths.package_metadata_path(package);
    write_cache_file(&path, body.as_bytes(), "metadata")
}

fn read_version_cache(
    paths: &CachePaths,
    package: &str,
    version: &str,
) -> Result<Option<PackageVersion>> {
    let path = paths.package_version_path(package, version);
    if !path.exists() {
        return Ok(None);
    }

    let body = fs::read_to_string(&path).map_err(|source| GitxError::Io {
        context: format!("reading version cache {}", path.display()),
        source,
    })?;
    let cache = match serde_json::from_str::<PackageVersionCache>(&body) {
        Ok(cache) => cache,
        Err(_) => {
            discard_cache_file(&path, "corrupt version")?;
            return Ok(None);
        }
    };

    if cache.package != package
        || cache.version != version
        || cache.metadata.name != package
        || cache.metadata.version != version
    {
        discard_cache_file(&path, "mismatched version")?;
        return Ok(None);
    }

    Ok(Some(cache.metadata))
}

fn write_version_cache(paths: &CachePaths, version: &PackageVersion) -> Result<()> {
    let package_dir = paths.package_versions_dir(&version.name);
    paths.ensure_managed_dir(&package_dir)?;

    let path = paths.package_version_path(&version.name, &version.version);
    let cache = PackageVersionCache {
        package: version.name.clone(),
        version: version.version.clone(),
        cached_at: unix_now(),
        metadata: version.clone(),
    };
    let body = serde_json::to_string_pretty(&cache).map_err(|source| GitxError::Json {
        context: format!("encoding version cache {}", path.display()),
        source,
    })?;

    write_cache_file(&path, body.as_bytes(), "version")
}

fn read_tag_cache(paths: &CachePaths, package: &str) -> Result<Option<PackageTagCache>> {
    let path = paths.package_tag_path(package);
    if !path.exists() {
        return Ok(None);
    }

    let body = fs::read_to_string(&path).map_err(|source| GitxError::Io {
        context: format!("reading tag cache {}", path.display()),
        source,
    })?;
    let cache = match serde_json::from_str::<PackageTagCache>(&body) {
        Ok(cache) => cache,
        Err(_) => {
            discard_cache_file(&path, "corrupt tag")?;
            return Ok(None);
        }
    };

    if cache.package != package {
        discard_cache_file(&path, "mismatched tag")?;
        return Ok(None);
    }

    Ok(Some(cache))
}

fn validate_package_version(
    version: &PackageVersion,
    expected_name: &str,
    expected_version: &str,
) -> Result<()> {
    if version.name != expected_name || version.version != expected_version {
        return Err(GitxError::NpmResolution(format!(
            "registry metadata identity mismatch: requested `{expected_name}@{expected_version}`, received `{}@{}`",
            version.name, version.version
        )));
    }
    Ok(())
}

fn write_tag_cache(
    paths: &CachePaths,
    package: &str,
    tags: &HashMap<String, String>,
) -> Result<()> {
    paths.ensure_managed_dir(&paths.npm_tags_dir())?;

    let path = paths.package_tag_path(package);
    let cache = PackageTagCache {
        package: package.to_string(),
        cached_at: unix_now(),
        tags: tags.clone(),
    };
    let body = serde_json::to_string_pretty(&cache).map_err(|source| GitxError::Json {
        context: format!("encoding tag cache {}", path.display()),
        source,
    })?;

    write_cache_file(&path, body.as_bytes(), "tag")
}

fn write_cache_file(path: &Path, body: &[u8], kind: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| GitxError::Message(format!("invalid cache path {}", path.display())))?;
    fs::create_dir_all(parent).map_err(|source| GitxError::Io {
        context: format!("creating {kind} cache directory {}", parent.display()),
        source,
    })?;

    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| GitxError::Io {
            context: format!(
                "creating temporary {kind} cache file in {}",
                parent.display()
            ),
            source,
        })?;
    temporary
        .write_all(body)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| GitxError::Io {
            context: format!("writing temporary {kind} cache file for {}", path.display()),
            source,
        })?;

    temporary
        .persist(path)
        .map(|_| ())
        .map_err(|error| GitxError::Io {
            context: format!("persisting {kind} cache file {}", path.display()),
            source: error.error,
        })
}

fn discard_cache_file(path: &Path, kind: &str) -> Result<()> {
    fs::remove_file(path).map_err(|source| GitxError::Io {
        context: format!("removing {kind} cache {}", path.display()),
        source,
    })
}

fn package_specs_for_request(request: &PackageRunRequest) -> Result<Vec<PackageSpec>> {
    if request.target.trim().is_empty() {
        return Err(GitxError::NpmResolution(
            "missing package or command target".to_string(),
        ));
    }

    if request.packages.is_empty() {
        return Ok(vec![parse_package_spec(&request.target)?]);
    }

    request
        .packages
        .iter()
        .map(|package| parse_package_spec(package))
        .collect()
}

fn command_name_for_request(
    request: &PackageRunRequest,
    resolved: &[ResolvedPackage],
) -> Result<String> {
    if !request.packages.is_empty() {
        return Ok(request.target.clone());
    }

    infer_bin_name(&resolved[0].version, None)
}

fn incompatible_package_names(resolved: &[ResolvedPackage]) -> Vec<String> {
    let host = HostPlatform::detect();
    resolved
        .iter()
        .filter(|package| {
            package_compatibility(&package.version, &host).status
                == CompatibilityStatus::Incompatible
        })
        .map(|package| format!("{}@{}", package.version.name, package.version.version))
        .collect()
}

fn validate_command_name(name: &str) -> Result<()> {
    if name.is_empty() || name.starts_with('-') || name.chars().any(char::is_control) {
        return Err(GitxError::NpmResolution(
            "package binary names cannot be empty, start with `-`, or contain control characters"
                .to_string(),
        ));
    }

    Ok(())
}

fn infer_bin_name(version: &PackageVersion, requested: Option<&str>) -> Result<String> {
    match version.bin.as_ref() {
        Some(BinField::Path(_)) => Ok(unscoped_name(&version.name).to_string()),
        Some(BinField::Map(entries)) => {
            if let Some(requested) = requested
                && entries.contains_key(requested)
            {
                return Ok(requested.to_string());
            }

            if entries.len() == 1 {
                return entries.keys().next().cloned().ok_or_else(|| {
                    GitxError::NpmResolution(format!(
                        "package `{}` has no bin entries",
                        version.name
                    ))
                });
            }

            let unscoped = unscoped_name(&version.name);
            if entries.contains_key(unscoped) {
                return Ok(unscoped.to_string());
            }

            Err(GitxError::NpmResolution(format!(
                "package `{}` has multiple bins; rerun with `gitx exec --package {}@{} --bin <bin>`",
                version.name, version.name, version.version
            )))
        }
        None => Err(GitxError::NpmResolution(format!(
            "package `{}` does not declare a bin",
            version.name
        ))),
    }
}

fn bin_names(version: &PackageVersion) -> Vec<String> {
    let mut bins = match &version.bin {
        Some(BinField::Path(_)) => vec![unscoped_name(&version.name).to_string()],
        Some(BinField::Map(entries)) => entries.keys().cloned().collect(),
        None => Vec::new(),
    };
    bins.sort();
    bins
}

fn platform_constraints(version: &PackageVersion) -> Vec<String> {
    let mut constraints = Vec::new();
    if let Some(os) = &version.os
        && !os.is_empty()
    {
        constraints.push(format!("OS: {}", os.join(", ")));
    }
    if let Some(cpu) = &version.cpu
        && !cpu.is_empty()
    {
        constraints.push(format!("CPU: {}", cpu.join(", ")));
    }
    constraints
}

fn package_compatibility(version: &PackageVersion, host: &HostPlatform) -> Compatibility {
    let os = npm_os(&host.os);
    let arch = npm_cpu(&host.arch);
    let os_matches = matches_npm_constraint(version.os.as_deref(), os);
    let cpu_matches = matches_npm_constraint(version.cpu.as_deref(), arch);
    let constraints = platform_constraints(version);

    if !os_matches || !cpu_matches {
        let mut blocked_by = Vec::new();
        if !os_matches {
            blocked_by.push(format!("OS `{os}`"));
        }
        if !cpu_matches {
            blocked_by.push(format!("CPU `{arch}`"));
        }
        return Compatibility::incompatible(
            format!(
                "package metadata excludes this host ({})",
                blocked_by.join(" and ")
            ),
            constraints,
            engine_requirements(version),
        );
    }

    if constraints.is_empty() {
        Compatibility {
            status: CompatibilityStatus::Unknown,
            summary:
                "the package does not publish OS or CPU restrictions; runtime requirements still need checking"
                    .to_string(),
            platforms: Vec::new(),
            requirements: engine_requirements(version),
        }
    } else {
        Compatibility {
            status: CompatibilityStatus::Compatible,
            summary: "published OS and CPU restrictions allow this host".to_string(),
            platforms: constraints,
            requirements: engine_requirements(version),
        }
    }
}

fn matches_npm_constraint(values: Option<&[String]>, host_value: &str) -> bool {
    let Some(values) = values else {
        return true;
    };
    let positives: Vec<_> = values
        .iter()
        .filter_map(|value| (!value.starts_with('!')).then_some(value.as_str()))
        .collect();
    let excluded = values
        .iter()
        .filter_map(|value| value.strip_prefix('!'))
        .any(|value| value.eq_ignore_ascii_case(host_value));

    !excluded
        && (positives.is_empty()
            || positives
                .iter()
                .any(|value| value.eq_ignore_ascii_case(host_value)))
}

fn npm_os(os: &str) -> &str {
    match os {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

fn npm_cpu(arch: &str) -> &str {
    match arch {
        "x86_64" => "x64",
        "x86" | "i686" | "i586" => "ia32",
        "aarch64" => "arm64",
        other => other,
    }
}

fn engine_requirements(version: &PackageVersion) -> Vec<String> {
    version
        .engines
        .as_ref()
        .map(|engines| {
            engines
                .iter()
                .map(|(runtime, requirement)| format!("{runtime} {requirement}"))
                .collect()
        })
        .unwrap_or_default()
}

fn metadata_text(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(text) if !text.trim().is_empty() => Some(text.to_string()),
        serde_json::Value::Object(values) => values
            .get("url")
            .or_else(|| values.get("type"))
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_string),
        _ => None,
    }
}

fn package_warnings(
    resolved: &[ResolvedPackage],
    backend: PackageBackend,
    allow_scripts: bool,
    allow_dangerous_scripts: bool,
) -> Vec<String> {
    let mut warnings = metadata_warnings(resolved);
    let lifecycle_packages = lifecycle_script_packages(resolved);

    if allow_scripts {
        warnings.push("Install lifecycle scripts are explicitly allowed for this run.".to_string());
    } else if !lifecycle_packages.is_empty() && backend == PackageBackend::Npm {
        warnings.push(
            "npm strict script policy will refuse unapproved install lifecycle scripts."
                .to_string(),
        );
    } else if !lifecycle_packages.is_empty() && backend == PackageBackend::Pnpm {
        warnings.push("pnpm install lifecycle scripts will be disabled for this run.".to_string());
    }

    if !allow_scripts && backend == PackageBackend::Bun {
        warnings.push(
            "bunx does not expose a reliable per-run script-blocking flag; execution requires --allow-scripts."
                .to_string(),
        );
    }

    if allow_dangerous_scripts {
        warnings
            .push("Deny-level lifecycle-script findings are explicitly overridden.".to_string());
    }

    warnings.sort();
    warnings.dedup();
    warnings
}

fn metadata_warnings(resolved: &[ResolvedPackage]) -> Vec<String> {
    let mut warnings = Vec::new();

    for package in resolved {
        if package.requested_was_mutable {
            warnings.push(format!(
                "`{}@{}` resolved through a mutable dist-tag; exact version is {}",
                package.spec.name,
                package.spec.requested.as_deref().unwrap_or("latest"),
                package.version.version
            ));
        }

        warnings.extend(scan_package_metadata(&package.version).warning_lines());
    }

    warnings.sort();
    warnings.dedup();
    warnings
}

fn has_deny_scan_findings(resolved: &[ResolvedPackage]) -> bool {
    resolved
        .iter()
        .any(|package| scan_package_metadata(&package.version).has_deny_findings())
}

fn scan_package_metadata(version: &PackageVersion) -> scanner::ScanReport {
    let mut input = NpmPackageScanInput::new(&version.name, &version.version);
    input.integrity = version.dist.integrity.clone();
    input.scripts = version
        .scripts
        .clone()
        .unwrap_or_default()
        .into_iter()
        .collect();
    input.dependency_count = version
        .dependencies
        .as_ref()
        .map(HashMap::len)
        .unwrap_or_default();
    input.optional_dependency_count = version
        .optional_dependencies
        .as_ref()
        .map(HashMap::len)
        .unwrap_or_default();
    input.peer_dependency_count = version
        .peer_dependencies
        .as_ref()
        .map(HashMap::len)
        .unwrap_or_default();

    scanner::scan_npm_package(&input)
}

fn lifecycle_script_packages(resolved: &[ResolvedPackage]) -> Vec<&str> {
    resolved
        .iter()
        .filter(|package| lifecycle_scripts(&package.version).is_some())
        .map(|package| package.version.name.as_str())
        .collect()
}

fn lifecycle_scripts(version: &PackageVersion) -> Option<Vec<&'static str>> {
    let scripts = version.scripts.as_ref()?;
    let lifecycle_scripts: Vec<_> = ["preinstall", "install", "postinstall"]
        .into_iter()
        .filter(|script| scripts.contains_key(*script))
        .collect();

    if lifecycle_scripts.is_empty() {
        None
    } else {
        Some(lifecycle_scripts)
    }
}

fn choose_backend(request: &PackageRunRequest) -> Result<PackageBackend> {
    let requested = request.backend;
    let candidates: &[(&str, PackageBackend)] = match requested {
        PackageBackend::Auto if request.packages.len() <= 1 && request.allow_scripts => &[
            ("npm", PackageBackend::Npm),
            ("bunx", PackageBackend::Bun),
            ("pnpm", PackageBackend::Pnpm),
        ],
        PackageBackend::Auto => &[("npm", PackageBackend::Npm), ("pnpm", PackageBackend::Pnpm)],
        PackageBackend::Npm => &[("npm", PackageBackend::Npm)],
        PackageBackend::Bun => &[("bunx", PackageBackend::Bun)],
        PackageBackend::Pnpm => &[("pnpm", PackageBackend::Pnpm)],
    };

    for (tool, backend) in candidates {
        if toolchain::exists_on_path(tool) {
            return Ok(*backend);
        }
    }

    let names = candidates
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ");
    Err(GitxError::MissingToolchain(names))
}

fn run_with_npm(
    paths: &CachePaths,
    resolved: &[ResolvedPackage],
    command_name: &str,
    request: &PackageRunRequest,
) -> Result<()> {
    let mut command = Command::new("npm");
    command.arg("exec").arg("--yes");
    if !request.allow_scripts {
        command.arg("--strict-allow-scripts");
    } else {
        command.arg("--dangerously-allow-all-scripts");
    }
    for package in resolved {
        command.arg(format!(
            "--package={}@{}",
            package.version.name, package.version.version
        ));
    }
    command.arg("--").arg(command_name).args(&request.args);
    command.env("npm_config_cache", paths.npm_backend_cache_dir());
    run_command(command, "npm exec")
}

fn run_with_bun(
    paths: &CachePaths,
    resolved: &[ResolvedPackage],
    command_name: &str,
    request: &PackageRunRequest,
) -> Result<()> {
    if !request.allow_scripts {
        return Err(GitxError::NpmResolution(
            "bunx cannot guarantee lifecycle-script blocking for a one-shot install; use --backend npm or explicitly pass --allow-scripts"
                .to_string(),
        ));
    }

    if request.packages.len() > 1 {
        return Err(GitxError::NpmResolution(
            "bun backend currently supports one package at a time".to_string(),
        ));
    }

    let mut command = Command::new("bunx");
    command.env("BUN_INSTALL_CACHE_DIR", paths.npm_backend_cache_dir());

    if request.packages.is_empty() {
        let package = &resolved[0].version;
        command.arg(format!("{}@{}", package.name, package.version));
    } else {
        command
            .arg("--package")
            .arg(format!(
                "{}@{}",
                resolved[0].version.name, resolved[0].version.version
            ))
            .arg(command_name);
    }

    command.args(&request.args);
    run_command(command, "bunx")
}

fn run_with_pnpm(
    paths: &CachePaths,
    resolved: &[ResolvedPackage],
    command_name: &str,
    request: &PackageRunRequest,
) -> Result<()> {
    let mut command = Command::new("pnpm");
    if !request.allow_scripts {
        command.arg("--config.ignore-scripts=true");
    }
    command.arg("dlx");
    let backend_cache = paths.npm_backend_cache_dir();
    command.env("pnpm_config_cache_dir", &backend_cache);
    command.env("pnpm_config_store_dir", backend_cache.join("pnpm-store"));
    if request.allow_scripts {
        for package in resolved {
            command.arg("--allow-build").arg(&package.version.name);
        }
    }

    if request.packages.is_empty() {
        let package = &resolved[0].version;
        command.arg(format!("{}@{}", package.name, package.version));
    } else {
        for package in resolved {
            command.arg("--package").arg(format!(
                "{}@{}",
                package.version.name, package.version.version
            ));
        }
        command.arg(command_name);
    }

    command.args(&request.args);
    run_command(command, "pnpm dlx")
}

fn run_command(mut command: Command, display_name: &str) -> Result<()> {
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|source| GitxError::Io {
            context: format!("running {display_name}"),
            source,
        })?;

    if status.success() {
        Ok(())
    } else {
        Err(GitxError::ToolExit {
            command: display_name.to_string(),
            code: status.code().unwrap_or(1),
        })
    }
}

struct PackagePreflight<'a> {
    backend: PackageBackend,
    paths: &'a CachePaths,
    command_name: &'a str,
    warnings: &'a [String],
    has_deny_findings: bool,
    incompatible_packages: &'a [String],
    allow_dangerous_scripts: bool,
}

fn print_preflight(resolved: &[ResolvedPackage], preflight: PackagePreflight<'_>) {
    eprintln!("GitX package preflight");
    eprintln!("Backend: {}", backend_name(preflight.backend));
    eprintln!(
        "Cache: {}",
        terminal_safe(&preflight.paths.root().display().to_string())
    );
    eprintln!("Command: {}", terminal_safe(preflight.command_name));
    let host = HostPlatform::detect();
    for package in resolved {
        eprintln!(
            "Package: {}@{}",
            terminal_safe(&package.version.name),
            terminal_safe(&package.version.version)
        );
        eprintln!("Resolution cache: {}", package.cache_status.summary());
        if let Some(description) = &package.version.description {
            eprintln!("Description: {}", terminal_safe(description));
        }
        let compatibility = package_compatibility(&package.version, &host);
        eprintln!(
            "Compatibility: {} ({})",
            compatibility.status.label(),
            terminal_safe(&compatibility.summary)
        );
        if let Some(homepage) = metadata_text(package.version.homepage.as_ref()) {
            eprintln!("Homepage: {}", terminal_safe(&homepage));
        }
        if let Some(license) = metadata_text(package.version.license.as_ref()) {
            eprintln!("License: {}", terminal_safe(&license));
        }
        let platforms = platform_constraints(&package.version);
        if !platforms.is_empty() {
            eprintln!(
                "Platform constraints: {}",
                terminal_safe(&platforms.join("; "))
            );
        }
        let requirements = engine_requirements(&package.version);
        if !requirements.is_empty() {
            eprintln!(
                "Runtime requirements: {}",
                terminal_safe(&requirements.join(", "))
            );
        }
        if let Some(integrity) = &package.version.dist.integrity {
            eprintln!("Integrity: {}", terminal_safe(integrity));
        }
        if let Some(shasum) = &package.version.dist.shasum {
            eprintln!("SHA1: {}", terminal_safe(shasum));
        }
        if let Some(tarball) = &package.version.dist.tarball {
            eprintln!("Tarball: {}", terminal_safe(tarball));
        }
        if let Some(file_count) = package.version.dist.file_count {
            eprintln!("Files: {file_count}");
        }
        if let Some(unpacked_size) = package.version.dist.unpacked_size {
            eprintln!("Unpacked: {} KB", unpacked_size / 1024);
        }
        if let Some(dependencies) = &package.version.dependencies {
            eprintln!("Dependencies: {}", dependencies.len());
        }
        if let Some(BinField::Path(path)) = &package.version.bin {
            eprintln!(
                "Bin: {} -> {}",
                terminal_safe(unscoped_name(&package.version.name)),
                terminal_safe(path)
            );
        }
        let bins = bin_names(&package.version);
        if bins.len() > 1 {
            eprintln!("Bins: {}", terminal_safe(&bins.join(", ")));
        }
    }

    let (risk, policy) = package_risk_and_policy(
        preflight.warnings,
        preflight.has_deny_findings,
        !preflight.incompatible_packages.is_empty(),
        preflight.allow_dangerous_scripts,
    );
    eprintln!("Risk: {risk}");
    eprintln!("Policy: {policy}");
    if !preflight.warnings.is_empty() {
        eprintln!("Warnings:");
        for warning in preflight.warnings {
            eprintln!("  - {}", terminal_safe(warning));
        }
    }
}

fn package_risk_and_policy(
    warnings: &[String],
    has_deny_findings: bool,
    has_platform_incompatibility: bool,
    allow_dangerous_scripts: bool,
) -> (&'static str, &'static str) {
    let risk = if has_deny_findings || has_platform_incompatibility {
        "high"
    } else if warnings.is_empty() {
        "low"
    } else {
        "unknown"
    };
    let policy = if has_platform_incompatibility || (has_deny_findings && !allow_dangerous_scripts)
    {
        "deny"
    } else if warnings.is_empty() {
        "allow"
    } else {
        "ask"
    };

    (risk, policy)
}

fn package_matches(version: &PackageVersion, normalized_query: &str) -> bool {
    let description = version.description.as_deref().unwrap_or_default();
    let keywords = version
        .keywords
        .as_deref()
        .map(|keywords| keywords.join(" "))
        .unwrap_or_default();
    format!("{} {description} {keywords}", version.name)
        .to_ascii_lowercase()
        .contains(normalized_query)
}

fn encode_package_name(package: &str) -> String {
    if package.starts_with('@') {
        package.replace('/', "%2f")
    } else {
        urlencoding::encode(package).to_string()
    }
}

fn looks_like_exact_version(version: &str) -> bool {
    let core = version.split(['-', '+']).next().unwrap_or(version);
    let mut parts = core.split('.');

    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(major), Some(minor), Some(patch), None)
            if [major, minor, patch]
                .into_iter()
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    )
}

fn unscoped_name(package: &str) -> &str {
    package
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(package)
}

fn backend_name(backend: PackageBackend) -> &'static str {
    match backend {
        PackageBackend::Auto => "auto",
        PackageBackend::Bun => "bun",
        PackageBackend::Npm => "npm",
        PackageBackend::Pnpm => "pnpm",
    }
}

impl ResolutionCacheStatus {
    fn summary(&self) -> String {
        match self {
            ResolutionCacheStatus::ExactHit => "exact version cache hit".to_string(),
            ResolutionCacheStatus::ExactFetched => "exact version cached".to_string(),
            ResolutionCacheStatus::TagResolved { tag, version } => {
                format!("dist-tag `{tag}` cached as {version}")
            }
            ResolutionCacheStatus::TagRefreshed { tag, version } => {
                format!("dist-tag `{tag}` refreshed at {version}")
            }
            ResolutionCacheStatus::TagMoved {
                tag,
                previous,
                current,
            } => {
                format!("dist-tag `{tag}` moved {previous} -> {current}")
            }
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, fs};

    use super::{
        BinField, PackageDist, PackageSpec, PackageVersion, PackageVersionCache, lifecycle_scripts,
        looks_like_exact_version, package_compatibility, package_risk_and_policy,
        parse_package_spec, read_version_cache, validate_command_name, write_version_cache,
    };
    use crate::{
        cache::CachePaths,
        catalog::{CompatibilityStatus, HostPlatform},
    };

    #[test]
    fn parses_unscoped_package_spec() {
        assert_eq!(
            parse_package_spec("prettier@3.6.2").unwrap(),
            PackageSpec {
                name: "prettier".to_string(),
                requested: Some("3.6.2".to_string())
            }
        );
    }

    #[test]
    fn parses_scoped_package_spec() {
        assert_eq!(
            parse_package_spec("@eslint/js@9.30.0").unwrap(),
            PackageSpec {
                name: "@eslint/js".to_string(),
                requested: Some("9.30.0".to_string())
            }
        );
    }

    #[test]
    fn parses_scoped_package_without_version() {
        assert_eq!(
            parse_package_spec("@eslint/js").unwrap(),
            PackageSpec {
                name: "@eslint/js".to_string(),
                requested: None
            }
        );
    }

    #[test]
    fn recognizes_semver_exact_versions() {
        assert!(looks_like_exact_version("1.2.3"));
        assert!(looks_like_exact_version("1.2.3-alpha.1"));
        assert!(looks_like_exact_version("1.2.3+build.4"));
        assert!(!looks_like_exact_version("latest"));
        assert!(!looks_like_exact_version("1"));
        assert!(!looks_like_exact_version("^1.2.3"));
    }

    #[test]
    fn rejects_option_like_package_and_binary_names() {
        assert!(parse_package_spec("--help@1.0.0").is_err());
        assert!(validate_command_name("--help").is_err());
        assert!(validate_command_name("demo").is_ok());
    }

    #[test]
    fn reports_deny_findings_as_high_risk_until_explicitly_overridden() {
        let warnings = vec!["deny: suspicious install script".to_string()];

        assert_eq!(
            package_risk_and_policy(&warnings, true, false, false),
            ("high", "deny")
        );
        assert_eq!(
            package_risk_and_policy(&warnings, true, false, true),
            ("high", "ask")
        );
        assert_eq!(
            package_risk_and_policy(&[], false, false, false),
            ("low", "allow")
        );
        assert_eq!(
            package_risk_and_policy(&[], false, true, true),
            ("high", "deny")
        );
    }

    #[test]
    fn respects_positive_and_negative_npm_platform_constraints() {
        let host = HostPlatform {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            distribution: Some("arch".to_string()),
        };
        let mut version = package_version_for_test();
        version.os = Some(vec!["linux".to_string()]);
        version.cpu = Some(vec!["x64".to_string()]);
        assert_eq!(
            package_compatibility(&version, &host).status,
            CompatibilityStatus::Compatible
        );

        version.os = Some(vec!["!linux".to_string()]);
        assert_eq!(
            package_compatibility(&version, &host).status,
            CompatibilityStatus::Incompatible
        );
    }

    #[test]
    fn prepare_script_is_not_registry_install_lifecycle() {
        let mut version = package_version_for_test();
        version.scripts = Some(HashMap::from([(
            "prepare".to_string(),
            "rollup -c".to_string(),
        )]));

        assert!(lifecycle_scripts(&version).is_none());

        version
            .scripts
            .as_mut()
            .unwrap()
            .insert("postinstall".to_string(), "node install.js".to_string());

        assert_eq!(lifecycle_scripts(&version), Some(vec!["postinstall"]));
    }

    #[test]
    fn reads_cached_exact_version() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = CachePaths::from_root_for_tests(tempdir.path());
        let version = package_version_for_test();

        write_version_cache(&paths, &version).unwrap();

        let cached = read_version_cache(&paths, "demo", "1.2.3")
            .unwrap()
            .unwrap();

        assert_eq!(cached.name, "demo");
        assert_eq!(cached.version, "1.2.3");
    }

    #[test]
    fn discards_corrupt_or_mismatched_exact_version_cache_entries() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = CachePaths::from_root_for_tests(tempdir.path());
        let path = paths.package_version_path("demo", "1.2.3");
        fs::create_dir_all(path.parent().unwrap()).unwrap();

        fs::write(&path, "not json").unwrap();
        assert!(
            read_version_cache(&paths, "demo", "1.2.3")
                .unwrap()
                .is_none()
        );
        assert!(!path.exists());

        let mut metadata = package_version_for_test();
        metadata.name = "other".to_string();
        let mismatched = PackageVersionCache {
            package: "demo".to_string(),
            version: "1.2.3".to_string(),
            cached_at: 0,
            metadata,
        };
        fs::write(&path, serde_json::to_vec(&mismatched).unwrap()).unwrap();

        assert!(
            read_version_cache(&paths, "demo", "1.2.3")
                .unwrap()
                .is_none()
        );
        assert!(!path.exists());
    }

    fn package_version_for_test() -> PackageVersion {
        PackageVersion {
            name: "demo".to_string(),
            version: "1.2.3".to_string(),
            description: None,
            homepage: None,
            license: None,
            bin: Some(BinField::Path("cli.js".to_string())),
            os: None,
            cpu: None,
            engines: None,
            keywords: None,
            scripts: None,
            dependencies: None,
            optional_dependencies: None,
            peer_dependencies: None,
            dist: PackageDist {
                tarball: None,
                integrity: Some("sha512-demo".to_string()),
                shasum: None,
                file_count: None,
                unpacked_size: None,
            },
        }
    }
}
