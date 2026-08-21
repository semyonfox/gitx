use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;

use crate::{
    catalog::{CatalogAction, CatalogCandidate, Compatibility, HostPlatform},
    display::shell_quote,
    error::{GitxError, Result},
    toolchain,
};

/// A local catalog command should be quick. It reads an already-present package
/// database or cache, never asks a package manager to refresh it, and GitX
/// stops it after this bound.
pub const LOCAL_CATALOG_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const LOCAL_CATALOG_OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceProvider {
    System,
    AptCache,
    PacmanCache,
    Flatpak,
    Snap,
    Cargo,
    Python,
    NpmGlobal,
}

impl DeviceProvider {
    pub fn available(self, host: &HostPlatform) -> bool {
        match self {
            Self::System => system_catalog_kind(host).is_some(),
            Self::AptCache => host.os == "linux" && toolchain::exists_on_path("apt-cache"),
            Self::PacmanCache => host.os == "linux" && toolchain::exists_on_path("pacman"),
            Self::Flatpak => toolchain::exists_on_path("flatpak"),
            Self::Snap => host.os == "linux" && toolchain::exists_on_path("snap"),
            Self::Cargo => toolchain::exists_on_path("cargo"),
            Self::Python => python_program().is_some(),
            Self::NpmGlobal => toolchain::exists_on_path("npm"),
        }
    }

    pub fn search(
        self,
        host: &HostPlatform,
        query: &str,
        limit: usize,
        local_only: bool,
    ) -> Result<Vec<CatalogCandidate>> {
        match self {
            Self::System => system_candidates(host, query, limit),
            Self::AptCache => apt_cache_candidates(host, query, limit),
            Self::PacmanCache => pacman_cache_candidates(host, query, limit),
            Self::Flatpak => flatpak_candidates(host, query, limit, local_only),
            Self::Snap => snap_candidates(query, limit),
            Self::Cargo => cargo_candidates(query, limit),
            Self::Python => python_candidates(query, limit),
            Self::NpmGlobal => npm_global_candidates(query, limit),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum SystemCatalogKind {
    Dpkg,
    Pacman,
    Rpm,
    Brew,
    Chocolatey,
}

fn system_catalog_kind(host: &HostPlatform) -> Option<SystemCatalogKind> {
    if host.os == "linux" && toolchain::exists_on_path("dpkg-query") {
        return Some(SystemCatalogKind::Dpkg);
    }
    if host.os == "linux" && toolchain::exists_on_path("pacman") {
        return Some(SystemCatalogKind::Pacman);
    }
    if host.os == "linux" && toolchain::exists_on_path("rpm") {
        return Some(SystemCatalogKind::Rpm);
    }
    if (host.is_macos() || host.os == "linux") && toolchain::exists_on_path("brew") {
        return Some(SystemCatalogKind::Brew);
    }
    if host.is_windows() && toolchain::exists_on_path("choco") {
        return Some(SystemCatalogKind::Chocolatey);
    }
    None
}

fn system_candidates(
    host: &HostPlatform,
    query: &str,
    limit: usize,
) -> Result<Vec<CatalogCandidate>> {
    let kind = system_catalog_kind(host).ok_or_else(|| {
        GitxError::Message(
            "no supported local system package database was found for this host".to_string(),
        )
    })?;

    match kind {
        SystemCatalogKind::Dpkg => {
            let output = run_command(
                "dpkg-query",
                [
                    "-W",
                    "-f=${binary:Package}\\t${Version}\\t${binary:Summary}\\n",
                ],
                "reading the local dpkg status database",
            )?;
            Ok(parse_dpkg_packages(&output, query, limit))
        }
        SystemCatalogKind::Pacman => {
            let output = run_command("pacman", ["-Q"], "reading the local pacman database")?;
            Ok(parse_pacman_packages(&output, query, limit))
        }
        SystemCatalogKind::Rpm => {
            let output = run_command(
                "rpm",
                [
                    "-qa",
                    "--qf",
                    "%{NAME}\\t%{VERSION}-%{RELEASE}\\t%{SUMMARY}\\n",
                ],
                "reading the local RPM database",
            )?;
            Ok(parse_rpm_packages(&output, query, limit))
        }
        SystemCatalogKind::Brew => {
            let output = run_command("brew", ["list", "--versions"], "reading Homebrew installs")?;
            Ok(parse_brew_packages(&output, query, limit))
        }
        SystemCatalogKind::Chocolatey => {
            let output = run_command(
                "choco",
                ["list", "--local-only", "--limit-output", "--no-color"],
                "reading Chocolatey installs",
            )?;
            Ok(parse_chocolatey_packages(&output, query, limit))
        }
    }
}

fn apt_cache_candidates(
    host: &HostPlatform,
    query: &str,
    limit: usize,
) -> Result<Vec<CatalogCandidate>> {
    if host.os != "linux" {
        return Err(GitxError::Message(
            "the apt source is available only on Linux hosts".to_string(),
        ));
    }
    if !toolchain::exists_on_path("apt-cache") {
        return Err(GitxError::MissingToolchain("apt-cache".to_string()));
    }

    let pattern = literal_regex_pattern(query);
    let search = run_command(
        "apt-cache",
        ["--no-generate", "search", "--names-only", pattern.as_str()],
        "searching local APT package lists",
    )?;
    let found = parse_apt_search(&search, limit);
    if found.is_empty() {
        return Ok(Vec::new());
    }

    let names: Vec<_> = found.iter().map(|entry| entry.name.as_str()).collect();
    let mut args = vec!["--no-generate", "show", "--no-all-versions"];
    args.extend(names);
    let details = run_command("apt-cache", args, "reading local APT package details")?;
    let details = parse_apt_details(&details);

    Ok(found
        .into_iter()
        .map(|entry| {
            let detail = details
                .get(&entry.name)
                .or_else(|| details.get(entry.name.split(':').next().unwrap_or(&entry.name)));
            let version = detail.and_then(|detail| detail.version.clone());
            let description = detail
                .and_then(|detail| detail.description.clone())
                .or(entry.description);
            let homepage = detail.and_then(|detail| detail.homepage.clone());
            let mut information = BTreeMap::new();
            information.insert("Manager".to_string(), "APT".to_string());
            information.insert(
                "State".to_string(),
                "available in local package lists".to_string(),
            );
            CatalogCandidate {
                source: "APT package cache".to_string(),
                origin: "APT package lists already stored on this device".to_string(),
                name: entry.name.clone(),
                version,
                description,
                homepage,
                updated_at: None,
                compatibility: Compatibility::compatible(
                    "available in this host's local APT package lists",
                    vec!["APT and administrator privileges to install".to_string()],
                ),
                information,
                action: CatalogAction::new(
                    "Install",
                    format!("sudo apt install {}", shell_quote(&entry.name)),
                ),
            }
        })
        .collect())
}

fn pacman_cache_candidates(
    host: &HostPlatform,
    query: &str,
    limit: usize,
) -> Result<Vec<CatalogCandidate>> {
    if host.os != "linux" {
        return Err(GitxError::Message(
            "the pacman source is available only on Linux hosts".to_string(),
        ));
    }
    if !toolchain::exists_on_path("pacman") {
        return Err(GitxError::MissingToolchain("pacman".to_string()));
    }
    let pattern = literal_regex_pattern(query);
    let output = run_command(
        "pacman",
        ["-Ss", pattern.as_str()],
        "searching local pacman sync databases",
    )?;
    Ok(parse_pacman_sync_packages(&output, limit))
}

fn snap_candidates(query: &str, limit: usize) -> Result<Vec<CatalogCandidate>> {
    if !toolchain::exists_on_path("snap") {
        return Err(GitxError::MissingToolchain("snap".to_string()));
    }
    let output = run_command("snap", ["list"], "reading installed Snap packages")?;
    Ok(parse_snap_packages(&output, query, limit))
}

fn flatpak_candidates(
    host: &HostPlatform,
    query: &str,
    limit: usize,
    local_only: bool,
) -> Result<Vec<CatalogCandidate>> {
    if !toolchain::exists_on_path("flatpak") {
        return Err(GitxError::MissingToolchain("flatpak".to_string()));
    }

    let installed = run_command(
        "flatpak",
        [
            "list",
            "--app",
            "--columns=application,version,description,origin",
        ],
        "reading installed Flatpak applications",
    );
    let cached = (!local_only).then(|| {
        run_command(
            "flatpak",
            [
                "remote-ls",
                "--cached",
                "--app",
                "--columns=application,version,description,origin",
            ],
            "reading cached Flatpak remote summaries",
        )
    });

    let mut candidates = Vec::new();
    let mut read_a_view = false;
    let mut first_error = None;
    match installed {
        Ok(installed) => {
            read_a_view = true;
            candidates.extend(
                parse_flatpak_rows(&installed, query, limit)
                    .into_iter()
                    .map(|app| flatpak_candidate(host, app, true)),
            );
        }
        Err(error) => first_error = Some(error),
    }

    // Flatpak documents --cached as a preference, not an absolute no-network
    // guarantee. Strict offline/device searches therefore use only `list`.
    // Outside those modes, an old release that lacks --cached still leaves
    // installed apps useful.
    match cached {
        Some(Ok(cached)) => {
            read_a_view = true;
            let remaining = limit.saturating_sub(candidates.len());
            if remaining > 0 {
                candidates.extend(
                    parse_flatpak_rows(&cached, query, remaining)
                        .into_iter()
                        .map(|app| flatpak_candidate(host, app, false)),
                );
            }
        }
        Some(Err(error)) if !read_a_view => return Err(first_error.unwrap_or(error)),
        Some(Err(_)) | None => {}
    }
    if read_a_view {
        Ok(candidates)
    } else {
        Err(first_error.expect("a Flatpak view error exists when none could be read"))
    }
}

fn cargo_candidates(query: &str, limit: usize) -> Result<Vec<CatalogCandidate>> {
    if !toolchain::exists_on_path("cargo") {
        return Err(GitxError::MissingToolchain("cargo".to_string()));
    }
    let output = run_command("cargo", ["install", "--list"], "reading Cargo installs")?;
    Ok(parse_cargo_installs(&output, query, limit))
}

fn python_candidates(query: &str, limit: usize) -> Result<Vec<CatalogCandidate>> {
    let python = python_program()
        .ok_or_else(|| GitxError::MissingToolchain("python3 or python".to_string()))?;
    let output = run_command(
        python,
        [
            "-m",
            "pip",
            "--disable-pip-version-check",
            "list",
            "--format=json",
        ],
        "reading installed Python packages",
    )?;
    parse_python_packages(&output, python, query, limit)
}

fn npm_global_candidates(query: &str, limit: usize) -> Result<Vec<CatalogCandidate>> {
    if !toolchain::exists_on_path("npm") {
        return Err(GitxError::MissingToolchain("npm".to_string()));
    }
    let output = run_command(
        "npm",
        ["ls", "--global", "--depth=0", "--json", "--silent"],
        "reading globally installed npm packages",
    )?;
    parse_npm_global_packages(&output, query, limit)
}

fn python_program() -> Option<&'static str> {
    ["python3", "python"]
        .into_iter()
        .find(|program| toolchain::exists_on_path(program))
}

pub fn run_command<I, S>(program: &str, args: I, context: &str) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_string())
        .collect();
    // A package database can be larger than a pipe buffer. Writing it to an
    // unlinked temporary file avoids deadlocking the child while retaining a
    // time and size bound for the catalog path.
    let mut stdout = tempfile::tempfile().map_err(|source| GitxError::Io {
        context: format!("creating temporary output for {context}"),
        source,
    })?;
    let child_stdout = stdout.try_clone().map_err(|source| GitxError::Io {
        context: format!("preparing temporary output for {context}"),
        source,
    })?;
    let mut child = Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(child_stdout))
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| GitxError::Io {
            context: format!("starting {context}"),
            source,
        })?;
    let deadline = Instant::now() + LOCAL_CATALOG_COMMAND_TIMEOUT;

    loop {
        if child
            .try_wait()
            .map_err(|source| GitxError::Io {
                context: format!("waiting for {context}"),
                source,
            })?
            .is_some()
        {
            let status = child.wait().map_err(|source| GitxError::Io {
                context: format!("collecting status from {context}"),
                source,
            })?;
            if !status.success() {
                return Err(GitxError::Message(format!(
                    "{context} exited with status {}",
                    status.code().unwrap_or(1)
                )));
            }
            let size = stdout
                .metadata()
                .map_err(|source| GitxError::Io {
                    context: format!("measuring output from {context}"),
                    source,
                })?
                .len();
            if size > LOCAL_CATALOG_OUTPUT_LIMIT {
                return Err(GitxError::Message(format!(
                    "{context} produced more than {} MiB of catalog data",
                    LOCAL_CATALOG_OUTPUT_LIMIT / (1024 * 1024)
                )));
            }
            stdout
                .seek(SeekFrom::Start(0))
                .map_err(|source| GitxError::Io {
                    context: format!("rewinding output from {context}"),
                    source,
                })?;
            let mut output = String::new();
            stdout
                .read_to_string(&mut output)
                .map_err(|source| GitxError::Io {
                    context: format!("reading output from {context}"),
                    source,
                })?;
            return Ok(output);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(GitxError::Message(format!(
                "{context} exceeded the {} second local catalog limit",
                LOCAL_CATALOG_COMMAND_TIMEOUT.as_secs()
            )));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn matches_query(query: &str, values: impl IntoIterator<Item = String>) -> bool {
    let query = query.trim().to_ascii_lowercase();
    values
        .into_iter()
        .any(|value| value.to_ascii_lowercase().contains(&query))
}

fn installed_compatibility(manager: &str) -> Compatibility {
    Compatibility::compatible(
        format!("installed through {manager} on this host"),
        Vec::new(),
    )
}

fn parse_dpkg_packages(output: &str, query: &str, limit: usize) -> Vec<CatalogCandidate> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let name = fields.next()?.trim();
            let version = fields.next()?.trim();
            let description = fields
                .next()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            (!name.is_empty()
                && matches_query(
                    query,
                    [
                        name.to_string(),
                        description.unwrap_or_default().to_string(),
                    ],
                ))
            .then(|| {
                let mut information = BTreeMap::new();
                information.insert("Manager".to_string(), "dpkg".to_string());
                information.insert("State".to_string(), "installed".to_string());
                CatalogCandidate {
                    source: "Debian installed package".to_string(),
                    origin: "dpkg status database on this device".to_string(),
                    name: name.to_string(),
                    version: (!version.is_empty()).then(|| version.to_string()),
                    description: description.map(str::to_string),
                    homepage: None,
                    updated_at: None,
                    compatibility: installed_compatibility("dpkg"),
                    information,
                    action: CatalogAction::new("Inspect", format!("dpkg -s {}", shell_quote(name))),
                }
            })
        })
        .take(limit)
        .collect()
}

fn parse_pacman_packages(output: &str, query: &str, limit: usize) -> Vec<CatalogCandidate> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next()?;
            let version = fields.next();
            matches_query(query, [name.to_string()]).then(|| {
                let mut information = BTreeMap::new();
                information.insert("Manager".to_string(), "pacman".to_string());
                information.insert("State".to_string(), "installed".to_string());
                CatalogCandidate {
                    source: "Arch installed package".to_string(),
                    origin: "pacman local database on this device".to_string(),
                    name: name.to_string(),
                    version: version.map(str::to_string),
                    description: None,
                    homepage: None,
                    updated_at: None,
                    compatibility: installed_compatibility("pacman"),
                    information,
                    action: CatalogAction::new(
                        "Inspect",
                        format!("pacman -Qi {}", shell_quote(name)),
                    ),
                }
            })
        })
        .take(limit)
        .collect()
}

#[derive(Debug, Clone)]
struct PacmanSyncPackage {
    repository: String,
    name: String,
    version: Option<String>,
    description: Option<String>,
}

fn literal_regex_pattern(query: &str) -> String {
    query.chars().fold(String::new(), |mut pattern, character| {
        if matches!(
            character,
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$'
        ) {
            pattern.push('\\');
        }
        pattern.push(character);
        pattern
    })
}

fn parse_pacman_sync_packages(output: &str, limit: usize) -> Vec<CatalogCandidate> {
    let mut packages = Vec::<PacmanSyncPackage>::new();
    let mut current = None::<PacmanSyncPackage>;
    for line in output.lines() {
        if line.starts_with(char::is_whitespace) {
            if let Some(package) = &mut current {
                let description = line.trim();
                if !description.is_empty() {
                    package.description = Some(description.to_string());
                }
            }
            continue;
        }
        if let Some(package) = current.take() {
            packages.push(package);
        }
        let mut fields = line.split_whitespace();
        let Some(repository_name) = fields.next() else {
            continue;
        };
        let Some((repository, name)) = repository_name.split_once('/') else {
            continue;
        };
        let version = fields.next().map(str::to_string);
        if !repository.is_empty() && !name.is_empty() {
            current = Some(PacmanSyncPackage {
                repository: repository.to_string(),
                name: name.to_string(),
                version,
                description: None,
            });
        }
    }
    if let Some(package) = current {
        packages.push(package);
    }

    packages
        .into_iter()
        .take(limit)
        .map(|package| {
            let mut information = BTreeMap::new();
            information.insert("Manager".to_string(), "pacman".to_string());
            information.insert("Repository".to_string(), package.repository.clone());
            information.insert(
                "State".to_string(),
                "available in local sync database".to_string(),
            );
            CatalogCandidate {
                source: "pacman sync cache".to_string(),
                origin: format!(
                    "pacman local sync database, repository {}",
                    package.repository
                ),
                name: package.name.clone(),
                version: package.version,
                description: package.description,
                homepage: None,
                updated_at: None,
                compatibility: Compatibility::compatible(
                    "available in this host's local pacman sync database",
                    vec!["pacman and administrator privileges to install".to_string()],
                ),
                information,
                action: CatalogAction::new(
                    "Install",
                    format!("sudo pacman -S {}", shell_quote(&package.name)),
                ),
            }
        })
        .collect()
}

fn parse_snap_packages(output: &str, query: &str, limit: usize) -> Vec<CatalogCandidate> {
    output
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next()?;
            let version = fields.next();
            matches_query(query, [name.to_string()]).then(|| {
                let mut information = BTreeMap::new();
                information.insert("Manager".to_string(), "snapd".to_string());
                information.insert("State".to_string(), "installed".to_string());
                CatalogCandidate {
                    source: "Snap installed package".to_string(),
                    origin: "snapd local package state on this device".to_string(),
                    name: name.to_string(),
                    version: version.map(str::to_string),
                    description: None,
                    homepage: None,
                    updated_at: None,
                    compatibility: installed_compatibility("snapd"),
                    information,
                    action: CatalogAction::new(
                        "Inspect",
                        format!("snap info {}", shell_quote(name)),
                    ),
                }
            })
        })
        .take(limit)
        .collect()
}

fn parse_rpm_packages(output: &str, query: &str, limit: usize) -> Vec<CatalogCandidate> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let name = fields.next()?.trim();
            let version = fields.next()?.trim();
            let description = fields
                .next()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            (!name.is_empty()
                && matches_query(
                    query,
                    [
                        name.to_string(),
                        description.unwrap_or_default().to_string(),
                    ],
                ))
            .then(|| {
                let mut information = BTreeMap::new();
                information.insert("Manager".to_string(), "rpm".to_string());
                information.insert("State".to_string(), "installed".to_string());
                CatalogCandidate {
                    source: "RPM installed package".to_string(),
                    origin: "RPM database on this device".to_string(),
                    name: name.to_string(),
                    version: (!version.is_empty()).then(|| version.to_string()),
                    description: description.map(str::to_string),
                    homepage: None,
                    updated_at: None,
                    compatibility: installed_compatibility("rpm"),
                    information,
                    action: CatalogAction::new("Inspect", format!("rpm -qi {}", shell_quote(name))),
                }
            })
        })
        .take(limit)
        .collect()
}

fn parse_brew_packages(output: &str, query: &str, limit: usize) -> Vec<CatalogCandidate> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next()?;
            let versions = fields.collect::<Vec<_>>();
            matches_query(query, [name.to_string()]).then(|| {
                let mut information = BTreeMap::new();
                information.insert("Manager".to_string(), "Homebrew".to_string());
                information.insert("State".to_string(), "installed".to_string());
                CatalogCandidate {
                    source: "Homebrew installed package".to_string(),
                    origin: "Homebrew cellar on this device".to_string(),
                    name: name.to_string(),
                    version: versions.first().map(|value| (*value).to_string()),
                    description: None,
                    homepage: None,
                    updated_at: None,
                    compatibility: installed_compatibility("Homebrew"),
                    information,
                    action: CatalogAction::new(
                        "Inspect",
                        format!("brew info {}", shell_quote(name)),
                    ),
                }
            })
        })
        .take(limit)
        .collect()
}

fn parse_chocolatey_packages(output: &str, query: &str, limit: usize) -> Vec<CatalogCandidate> {
    output
        .lines()
        .filter_map(|line| {
            let (name, version) = line.split_once('|')?;
            let name = name.trim();
            let version = version.trim();
            (!name.is_empty() && matches_query(query, [name.to_string()])).then(|| {
                let mut information = BTreeMap::new();
                information.insert("Manager".to_string(), "Chocolatey".to_string());
                information.insert("State".to_string(), "installed".to_string());
                CatalogCandidate {
                    source: "Chocolatey installed package".to_string(),
                    origin: "Chocolatey local package state on this device".to_string(),
                    name: name.to_string(),
                    version: (!version.is_empty()).then(|| version.to_string()),
                    description: None,
                    homepage: None,
                    updated_at: None,
                    compatibility: installed_compatibility("Chocolatey"),
                    information,
                    action: CatalogAction::new(
                        "Inspect",
                        format!("choco info {} --local-only", shell_quote(name)),
                    ),
                }
            })
        })
        .take(limit)
        .collect()
}

#[derive(Debug, Clone)]
struct AptSearchEntry {
    name: String,
    description: Option<String>,
}

fn parse_apt_search(output: &str, limit: usize) -> Vec<AptSearchEntry> {
    output
        .lines()
        .filter_map(|line| {
            let (name, description) = line
                .split_once(" - ")
                .map(|(name, description)| (name, Some(description)))
                .unwrap_or((line, None));
            let name = name.trim();
            (!name.is_empty()).then(|| AptSearchEntry {
                name: name.to_string(),
                description: description
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
            })
        })
        .take(limit)
        .collect()
}

#[derive(Debug, Default)]
struct AptDetails {
    version: Option<String>,
    description: Option<String>,
    homepage: Option<String>,
}

fn parse_apt_details(output: &str) -> BTreeMap<String, AptDetails> {
    let mut packages = BTreeMap::new();
    let mut fields = BTreeMap::<String, String>::new();
    let mut last_key = None::<String>;

    let commit = |fields: &mut BTreeMap<String, String>,
                  packages: &mut BTreeMap<String, AptDetails>| {
        let Some(name) = fields.remove("Package") else {
            fields.clear();
            return;
        };
        let details = AptDetails {
            version: fields.remove("Version"),
            description: fields.remove("Description"),
            homepage: fields.remove("Homepage"),
        };
        packages.entry(name).or_insert(details);
        fields.clear();
    };

    for line in output.lines().chain(std::iter::once("")) {
        if line.trim().is_empty() {
            commit(&mut fields, &mut packages);
            last_key = None;
            continue;
        }
        if let Some(continuation) = line.strip_prefix(' ') {
            if let Some(key) = &last_key
                && let Some(value) = fields.get_mut(key)
                && continuation.trim() != "."
            {
                value.push(' ');
                value.push_str(continuation.trim());
            }
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_string();
        fields.insert(key.clone(), value.trim().to_string());
        last_key = Some(key);
    }
    packages
}

#[derive(Debug, Clone)]
struct FlatpakRow {
    application: String,
    version: Option<String>,
    description: Option<String>,
    origin: Option<String>,
}

fn parse_flatpak_rows(output: &str, query: &str, limit: usize) -> Vec<FlatpakRow> {
    output
        .lines()
        .filter_map(|line| {
            let fields = line.split('\t').collect::<Vec<_>>();
            let application = fields.first()?.trim();
            if application.is_empty() {
                return None;
            }
            let version = fields
                .get(1)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty());
            let description = fields
                .get(2)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty());
            let origin = fields
                .get(3)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty());
            matches_query(
                query,
                [
                    application.to_string(),
                    description.unwrap_or_default().to_string(),
                    origin.unwrap_or_default().to_string(),
                ],
            )
            .then(|| FlatpakRow {
                application: application.to_string(),
                version: version.map(str::to_string),
                description: description.map(str::to_string),
                origin: origin.map(str::to_string),
            })
        })
        .take(limit)
        .collect()
}

fn flatpak_candidate(host: &HostPlatform, app: FlatpakRow, installed: bool) -> CatalogCandidate {
    let origin_name = app.origin.as_deref().unwrap_or("unknown remote");
    let mut information = BTreeMap::new();
    information.insert("Remote".to_string(), origin_name.to_string());
    information.insert(
        "State".to_string(),
        if installed {
            "installed".to_string()
        } else {
            "available in cached remote summary".to_string()
        },
    );
    let compatibility = if host.os == "linux" {
        if installed {
            Compatibility::compatible(
                "installed Flatpak application on this Linux host",
                Vec::new(),
            )
        } else {
            Compatibility::compatible(
                "available from a Flatpak remote summary cached on this Linux host",
                vec!["Flatpak and the named remote to install".to_string()],
            )
        }
    } else {
        Compatibility::incompatible(
            "Flatpak application discovery is intended for Linux hosts",
            vec!["Linux".to_string()],
            vec!["Flatpak".to_string()],
        )
    };
    let action = if installed {
        CatalogAction::new(
            "Run",
            format!("flatpak run {}", shell_quote(&app.application)),
        )
    } else if let Some(origin) = &app.origin {
        CatalogAction::new(
            "Install",
            format!(
                "flatpak install {} {}",
                shell_quote(origin),
                shell_quote(&app.application)
            ),
        )
    } else {
        CatalogAction::new(
            "Inspect",
            format!("flatpak info {}", shell_quote(&app.application)),
        )
    };

    CatalogCandidate {
        source: if installed {
            "Flatpak installed app".to_string()
        } else {
            "Flatpak cached remote".to_string()
        },
        origin: if installed {
            format!("Flatpak installation, remote {origin_name}")
        } else {
            format!("Flatpak cached remote summary, remote {origin_name}")
        },
        name: app.application,
        version: app.version,
        description: app.description,
        homepage: None,
        updated_at: None,
        compatibility,
        information,
        action,
    }
}

#[derive(Debug, Clone)]
struct CargoInstall {
    name: String,
    version: Option<String>,
    binaries: Vec<String>,
}

fn parse_cargo_installs(output: &str, query: &str, limit: usize) -> Vec<CatalogCandidate> {
    let mut installs = Vec::new();
    let mut current = None::<CargoInstall>;
    for line in output.lines() {
        if !line.starts_with(char::is_whitespace) && line.trim_end().ends_with(':') {
            if let Some(install) = current.take() {
                installs.push(install);
            }
            let header = line.trim().trim_end_matches(':');
            let (name, version) = header
                .rsplit_once(" v")
                .map(|(name, version)| (name.trim(), Some(version.trim().to_string())))
                .unwrap_or((header, None));
            if !name.is_empty() {
                current = Some(CargoInstall {
                    name: name.to_string(),
                    version,
                    binaries: Vec::new(),
                });
            }
        } else if let Some(install) = &mut current {
            let binary = line.trim();
            if !binary.is_empty() {
                install.binaries.push(binary.to_string());
            }
        }
    }
    if let Some(install) = current {
        installs.push(install);
    }

    installs
        .into_iter()
        .filter(|install| {
            matches_query(
                query,
                std::iter::once(install.name.clone()).chain(install.binaries.clone()),
            )
        })
        .take(limit)
        .map(|install| {
            let mut information = BTreeMap::new();
            if !install.binaries.is_empty() {
                information.insert("Binaries".to_string(), install.binaries.join(", "));
            }
            let action = install
                .binaries
                .first()
                .map(|binary| {
                    CatalogAction::new("Run", format!("{} -- <args>", shell_quote(binary)))
                })
                .unwrap_or_else(|| {
                    CatalogAction::new(
                        "Inspect",
                        format!("cargo install --list {}", shell_quote(&install.name)),
                    )
                });
            CatalogCandidate {
                source: "Cargo installed package".to_string(),
                origin: "Cargo install records on this device".to_string(),
                name: install.name,
                version: install.version,
                description: None,
                homepage: None,
                updated_at: None,
                compatibility: installed_compatibility("Cargo"),
                information,
                action,
            }
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct PythonPackage {
    name: String,
    version: String,
}

fn parse_python_packages(
    output: &str,
    python: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<CatalogCandidate>> {
    let packages: Vec<PythonPackage> =
        serde_json::from_str(output).map_err(|source| GitxError::Json {
            context: "decoding installed Python package list".to_string(),
            source,
        })?;
    Ok(packages
        .into_iter()
        .filter(|package| matches_query(query, [package.name.clone()]))
        .take(limit)
        .map(|package| {
            let mut information = BTreeMap::new();
            information.insert("Interpreter".to_string(), python.to_string());
            information.insert("State".to_string(), "installed".to_string());
            CatalogCandidate {
                source: "Python installed package".to_string(),
                origin: format!("{python} environment on this device"),
                name: package.name.clone(),
                version: Some(package.version),
                description: None,
                homepage: None,
                updated_at: None,
                compatibility: installed_compatibility("pip"),
                information,
                action: CatalogAction::new(
                    "Inspect",
                    format!("{python} -m pip show {}", shell_quote(&package.name)),
                ),
            }
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct NpmGlobalList {
    #[serde(default)]
    dependencies: BTreeMap<String, NpmGlobalPackage>,
}

#[derive(Debug, Deserialize)]
struct NpmGlobalPackage {
    version: Option<String>,
}

fn parse_npm_global_packages(
    output: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<CatalogCandidate>> {
    let list: NpmGlobalList = serde_json::from_str(output).map_err(|source| GitxError::Json {
        context: "decoding globally installed npm package list".to_string(),
        source,
    })?;
    Ok(list
        .dependencies
        .into_iter()
        .filter(|(name, _)| matches_query(query, [name.clone()]))
        .take(limit)
        .map(|(name, package)| {
            let mut information = BTreeMap::new();
            information.insert("Manager".to_string(), "npm".to_string());
            information.insert("State".to_string(), "globally installed".to_string());
            CatalogCandidate {
                source: "npm global package".to_string(),
                origin: "npm global package tree on this device".to_string(),
                name: name.clone(),
                version: package.version,
                description: None,
                homepage: None,
                updated_at: None,
                compatibility: installed_compatibility("npm"),
                information,
                action: CatalogAction::new(
                    "Inspect",
                    format!("npm ls --global --depth=0 {}", shell_quote(&name)),
                ),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{
        literal_regex_pattern, parse_apt_details, parse_apt_search, parse_cargo_installs,
        parse_dpkg_packages, parse_flatpak_rows, parse_npm_global_packages,
        parse_pacman_sync_packages, parse_python_packages, parse_snap_packages,
    };

    #[test]
    fn parses_dpkg_status_rows_with_origin_and_description() {
        let results =
            parse_dpkg_packages("ripgrep\t14.1.0-1\tfast line-oriented search\n", "grep", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "ripgrep");
        assert_eq!(results[0].origin, "dpkg status database on this device");
        assert_eq!(
            results[0].description.as_deref(),
            Some("fast line-oriented search")
        );
    }

    #[test]
    fn parses_apt_cache_search_and_control_stanzas() {
        let found = parse_apt_search("ripgrep - fast search\nripgrep-doc - docs\n", 10);
        assert_eq!(found.len(), 2);
        let details = parse_apt_details(
            "Package: ripgrep\nVersion: 14.1.0-1\nHomepage: https://example.test/ripgrep\nDescription: Fast search\n more detail\n\n",
        );
        let ripgrep = details.get("ripgrep").unwrap();
        assert_eq!(ripgrep.version.as_deref(), Some("14.1.0-1"));
        assert_eq!(
            ripgrep.description.as_deref(),
            Some("Fast search more detail")
        );
        assert_eq!(
            ripgrep.homepage.as_deref(),
            Some("https://example.test/ripgrep")
        );
    }

    #[test]
    fn parses_flatpak_rows_and_keeps_the_remote() {
        let rows = parse_flatpak_rows("org.gnome.Builder\t46.2\tIDE\tflathub\n", "builder", 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].origin.as_deref(), Some("flathub"));
    }

    #[test]
    fn parses_pacman_sync_rows_and_escapes_a_literal_search() {
        assert_eq!(literal_regex_pattern("tool+test"), "tool\\+test");
        let results = parse_pacman_sync_packages(
            "extra/ripgrep 14.1.0-1 [installed]\n    Recursively searches directories\n",
            10,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "ripgrep");
        assert_eq!(
            results[0].information.get("Repository").map(String::as_str),
            Some("extra")
        );
    }

    #[test]
    fn parses_installed_snap_rows() {
        let results = parse_snap_packages(
            "Name     Version  Rev  Tracking  Publisher  Notes\nhello    2.10     29   latest    canonical  -\n",
            "hello",
            10,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].version.as_deref(), Some("2.10"));
        assert_eq!(
            results[0].origin,
            "snapd local package state on this device"
        );
    }

    #[test]
    fn parses_cargo_installs_and_their_binaries() {
        let results = parse_cargo_installs("ripgrep v14.1.0:\n    rg\n", "rg", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "ripgrep");
        assert_eq!(results[0].version.as_deref(), Some("14.1.0"));
        assert_eq!(
            results[0].information.get("Binaries").map(String::as_str),
            Some("rg")
        );
    }

    #[test]
    fn parses_python_and_npm_json_without_network_data() {
        let python = parse_python_packages(
            r#"[{"name":"ruff","version":"0.9.1"}]"#,
            "python3",
            "ruff",
            10,
        )
        .unwrap();
        assert_eq!(python[0].origin, "python3 environment on this device");

        let npm = parse_npm_global_packages(
            r#"{"dependencies":{"prettier":{"version":"3.5.0"}}}"#,
            "prett",
            10,
        )
        .unwrap();
        assert_eq!(npm[0].version.as_deref(), Some("3.5.0"));
    }
}
