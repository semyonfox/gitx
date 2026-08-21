use std::{
    collections::BTreeMap,
    env, fs,
    io::Write,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    cache::{CachePaths, sanitize_key},
    display::terminal_safe,
    error::{GitxError, Result},
    git,
    manifest::{MANIFEST_FILE, Manifest, resolve_entry},
    npm, report,
    source::Source,
};

#[derive(Debug, Clone)]
pub struct InstallPaths {
    home: PathBuf,
}

impl InstallPaths {
    pub fn discover() -> Self {
        if let Ok(home) = env::var("GITX_HOME") {
            return Self {
                home: PathBuf::from(home),
            };
        }

        let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());

        Self {
            home: PathBuf::from(home).join(".gitx"),
        }
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.home.join("bin")
    }

    pub fn records_dir(&self) -> PathBuf {
        self.home.join("installed")
    }

    pub fn shim_path(&self, name: &str) -> PathBuf {
        self.bin_dir().join(name)
    }

    pub fn record_path(&self, name: &str) -> PathBuf {
        self.records_dir().join(format!("{name}.json"))
    }

    fn ensure_dirs(&self) -> Result<()> {
        self.validate_root()?;
        for path in [self.bin_dir(), self.records_dir()] {
            fs::create_dir_all(&path).map_err(|source| GitxError::Io {
                context: format!("creating install directory {}", path.display()),
                source,
            })?;
        }
        // Re-resolve after creation so a pre-existing symlink cannot turn the
        // managed install root into the filesystem root, home, or cwd.
        self.validate_root()?;
        self.validate_managed_dirs()?;

        Ok(())
    }

    fn validate_managed_dirs(&self) -> Result<()> {
        let canonical_root = self.home.canonicalize().map_err(|source| GitxError::Io {
            context: format!("resolving install root {}", self.home.display()),
            source,
        })?;

        for path in [self.bin_dir(), self.records_dir()] {
            if !path.exists() {
                continue;
            }
            let metadata = fs::symlink_metadata(&path).map_err(|source| GitxError::Io {
                context: format!("inspecting managed install directory {}", path.display()),
                source,
            })?;
            let resolved = path.canonicalize().map_err(|source| GitxError::Io {
                context: format!("resolving managed install directory {}", path.display()),
                source,
            })?;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || !resolved.starts_with(&canonical_root)
                || resolved == canonical_root
            {
                return Err(GitxError::Message(format!(
                    "refusing unsafe managed install directory {}",
                    path.display()
                )));
            }
        }

        Ok(())
    }

    fn validate_root(&self) -> Result<()> {
        if self.home.as_os_str().is_empty() {
            return Err(GitxError::Message(
                "GITX_HOME cannot be an empty path".to_string(),
            ));
        }

        if self.home.to_string_lossy().chars().any(char::is_control) {
            return Err(GitxError::Message(
                "GITX_HOME cannot contain control characters".to_string(),
            ));
        }

        if self
            .home
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(GitxError::Message(format!(
                "GITX_HOME cannot contain parent-directory traversal: {}",
                self.home.display()
            )));
        }

        let absolute = if self.home.is_absolute() {
            self.home.clone()
        } else {
            env::current_dir()
                .map_err(|source| GitxError::Io {
                    context: "resolving current directory for GITX_HOME".to_string(),
                    source,
                })?
                .join(&self.home)
        };
        let resolved = resolve_with_existing_ancestor(&absolute)?;
        let current = env::current_dir()
            .and_then(|path| path.canonicalize())
            .map_err(|source| GitxError::Io {
                context: "resolving current directory for GITX_HOME".to_string(),
                source,
            })?;
        let user_home = env::var_os("HOME")
            .map(PathBuf::from)
            .map(|path| resolve_with_existing_ancestor(&path))
            .transpose()?;

        if resolved.parent().is_none()
            || resolved == current
            || user_home.as_ref().is_some_and(|home| resolved == *home)
        {
            return Err(GitxError::Message(format!(
                "refusing unsafe GITX_HOME {}",
                self.home.display()
            )));
        }

        Ok(())
    }
}

fn resolve_with_existing_ancestor(path: &Path) -> Result<PathBuf> {
    let mut ancestor = path;
    while !ancestor.exists() {
        ancestor = ancestor.parent().ok_or_else(|| {
            GitxError::Message(format!("cannot resolve install path {}", path.display()))
        })?;
    }

    let canonical_ancestor = ancestor.canonicalize().map_err(|source| GitxError::Io {
        context: format!("resolving install path ancestor {}", ancestor.display()),
        source,
    })?;
    let tail = path.strip_prefix(ancestor).map_err(|_| {
        GitxError::Message(format!("cannot resolve install path {}", path.display()))
    })?;

    Ok(canonical_ancestor.join(tail))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallRecord {
    name: String,
    kind: String,
    source: String,
    pinned: String,
    runtime: Option<String>,
    integrity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entrypoint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    binaries: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    platforms: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    requirements: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    metadata: BTreeMap<String, String>,
    installed_at: u64,
}

/// A managed tool that can be surfaced by local search without executing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledToolMatch {
    pub name: String,
    pub kind: String,
    pub pinned: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub runtime: Option<String>,
    pub entrypoint: Option<String>,
    pub binaries: Vec<String>,
    pub platforms: Vec<String>,
    pub requirements: Vec<String>,
    pub metadata: BTreeMap<String, String>,
}

pub fn install(input: &str, name_override: Option<&str>) -> Result<String> {
    if !cfg!(unix) {
        return Err(GitxError::Message(
            "managed install shims are currently supported only on Unix-like systems".to_string(),
        ));
    }

    if let Some(name) = name_override {
        validate_tool_name(name)?;
    }

    let paths = InstallPaths::discover();
    paths.ensure_dirs()?;

    let source = parse_install_source(input)?;
    let mut notes = Vec::new();

    report::step("Resolving the source to an immutable install record");
    let mut record = resolve_install_record(&source, &mut notes)?;

    if let Some(name) = name_override {
        record.name = name.to_string();
    }

    validate_tool_name(&record.name)?;

    let shim_path = paths.shim_path(&record.name);
    let record_path = paths.record_path(&record.name);
    if shim_path.exists() || record_path.exists() {
        return Err(GitxError::Message(format!(
            "`{}` is already installed; remove it before replacing the pinned target",
            record.name
        )));
    }
    report::step("Writing the managed install record and shim");
    write_record(&paths, &record)?;
    if let Err(error) = write_shim(&shim_path, &shim_command(&record)) {
        let _ = fs::remove_file(&record_path);
        return Err(error);
    }

    let mut output = vec![format!(
        "Installed {} -> {}",
        record.name,
        shim_path.display()
    )];
    output.push(format!("Pinned: {}", record.pinned));
    output.extend(notes.into_iter().map(|note| format!("Note: {note}")));

    let bin_dir = paths.bin_dir();
    if !path_env_contains(&bin_dir) {
        output.push(format!(
            "Note: add {} to PATH to use installed tools directly",
            bin_dir.display()
        ));
    }

    Ok(output.join("\n"))
}

/// Re-resolve the source originally used for an installed tool and replace its
/// immutable pin only when the resolved identity changed.
pub fn update(name: &str, check: bool) -> Result<String> {
    if !cfg!(unix) {
        return Err(GitxError::Message(
            "managed install shims are currently supported only on Unix-like systems".to_string(),
        ));
    }

    validate_tool_name(name)?;
    let paths = InstallPaths::discover();
    paths.validate_root()?;
    if !paths.home.exists() {
        return Err(GitxError::Message(format!(
            "`{name}` is not installed; see `gitx list`"
        )));
    }
    paths.validate_managed_dirs()?;

    let record_path = paths.record_path(name);
    let shim_path = paths.shim_path(name);
    if !record_path.exists() {
        return Err(GitxError::Message(format!(
            "`{name}` is not installed; see `gitx list`"
        )));
    }
    report::step("Reading the installed record and shim");
    let (previous, previous_record_body) = read_record_with_body(&record_path)?;
    if previous.name != name {
        return Err(GitxError::Message(format!(
            "install record {} belongs to `{}` instead of `{name}`",
            record_path.display(),
            previous.name
        )));
    }
    ensure_regular_file(&shim_path, "installed shim")?;

    report::step("Resolving the original source again");
    let source = parse_install_source(&previous.source)?;
    let mut notes = Vec::new();
    let mut next = resolve_install_record(&source, &mut notes)?;
    if next.kind != previous.kind {
        return Err(GitxError::Message(format!(
            "refusing to update `{name}` because its source changed from {} to {}",
            previous.kind, next.kind
        )));
    }

    // Updating a pin must not silently rename a command, including an install
    // that originally used `--as`.
    next.name = previous.name.clone();
    next.source = previous.source.clone();
    let changes = record_changes(&previous, &next);
    let changed = !changes.is_empty();
    let pin_changed = previous.pinned != next.pinned;

    let mut output = vec![format!("Update {}", terminal_safe(name))];
    output.push(format!("Source: {}", terminal_safe(&previous.source)));
    output.push(format!("Current: {}", terminal_safe(&previous.pinned)));
    output.push(format!("Resolved: {}", terminal_safe(&next.pinned)));
    if changed {
        output.push("Changes:".to_string());
        output.extend(changes.iter().map(UpdateChange::render));
    }
    output.extend(notes.into_iter().map(|note| format!("Note: {note}")));

    if check {
        output.push(if changed {
            if pin_changed {
                "Action: would replace the pinned target and record the shown metadata".to_string()
            } else {
                "Action: would refresh the recorded metadata; the pinned target is unchanged"
                    .to_string()
            }
        } else {
            "Action: no change; the current pin is already up to date".to_string()
        });
        return Ok(output.join("\n"));
    }

    if !changed {
        output.push("No update needed.".to_string());
        return Ok(output.join("\n"));
    }

    if pin_changed {
        report::step("Writing the new immutable pin and shim");
        replace_record_and_shim(&paths, &next, &previous_record_body)?;
        output.push(format!("Updated pin: {}", terminal_safe(&next.pinned)));
    } else {
        report::step("Refreshing managed install metadata");
        replace_record(&paths, &next)?;
        output.push("Updated metadata.".to_string());
    }
    Ok(output.join("\n"))
}

/// Find installed commands by name, pin, or source kind without touching the
/// network or executing a managed shim.
pub fn search(query: &str, limit: usize) -> Result<Vec<InstalledToolMatch>> {
    let paths = InstallPaths::discover();
    paths.validate_root()?;
    if !paths.home.exists() {
        return Ok(Vec::new());
    }
    paths.validate_managed_dirs()?;

    let normalized = query.trim().to_ascii_lowercase();
    let mut matches: Vec<_> = read_records(&paths)?
        .into_iter()
        .filter(|record| {
            let haystack = std::iter::once(record.name.as_str())
                .chain(std::iter::once(record.kind.as_str()))
                .chain(std::iter::once(record.pinned.as_str()))
                .chain(record.description.as_deref())
                .chain(record.homepage.as_deref())
                .chain(record.license.as_deref())
                .chain(record.entrypoint.as_deref())
                .chain(record.binaries.iter().map(String::as_str))
                .chain(record.metadata.keys().map(String::as_str))
                .chain(record.metadata.values().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase();
            haystack.contains(&normalized)
        })
        .map(|record| InstalledToolMatch {
            name: record.name,
            kind: record.kind,
            pinned: record.pinned,
            description: record.description,
            homepage: record.homepage,
            license: record.license,
            runtime: record.runtime,
            entrypoint: record.entrypoint,
            binaries: record.binaries,
            platforms: record.platforms,
            requirements: record.requirements,
            metadata: record.metadata,
        })
        .collect();
    matches.sort_by(|left, right| left.name.cmp(&right.name));
    matches.truncate(limit);
    Ok(matches)
}

pub fn remove(name: &str) -> Result<String> {
    validate_tool_name(name)?;
    let paths = InstallPaths::discover();
    paths.validate_root()?;
    if paths.home.exists() {
        paths.validate_managed_dirs()?;
    }
    let shim_path = paths.shim_path(name);
    let record_path = paths.record_path(name);
    let mut removed = false;

    for path in [&shim_path, &record_path] {
        if path.exists() {
            fs::remove_file(path).map_err(|source| GitxError::Io {
                context: format!("removing {}", path.display()),
                source,
            })?;
            removed = true;
        }
    }

    if removed {
        Ok(format!("Removed {name}"))
    } else {
        Err(GitxError::Message(format!(
            "`{name}` is not installed; see `gitx list`"
        )))
    }
}

pub fn list() -> Result<String> {
    let paths = InstallPaths::discover();
    paths.validate_root()?;
    if paths.home.exists() {
        paths.validate_managed_dirs()?;
    }
    let mut records = read_records(&paths)?;

    if records.is_empty() {
        return Ok("No tools installed.".to_string());
    }

    records.sort_by(|left, right| left.name.cmp(&right.name));

    let mut output = vec![format!("{:<24} {:<6} {}", "name", "kind", "pinned target")];
    for record in records {
        output.push(format!(
            "{:<24} {:<6} {}",
            terminal_safe(&record.name),
            terminal_safe(&record.kind),
            terminal_safe(&record.pinned)
        ));
    }

    Ok(output.join("\n"))
}

fn resolve_install_record(source: &Source, notes: &mut Vec<String>) -> Result<InstallRecord> {
    match source {
        Source::Npm { .. } => install_npm(source.raw(), source.raw(), notes),
        Source::Github { .. } | Source::Git { .. } => install_git(source, notes),
        Source::Local { .. } => Err(GitxError::Message(
            "installing local paths is not supported yet; run them directly with `gitx run`"
                .to_string(),
        )),
    }
}

fn read_records(paths: &InstallPaths) -> Result<Vec<InstallRecord>> {
    let records_dir = paths.records_dir();
    if !records_dir.exists() {
        return Ok(Vec::new());
    }

    let mut records = Vec::new();
    for entry in fs::read_dir(&records_dir).map_err(|source| GitxError::Io {
        context: format!("reading install records {}", records_dir.display()),
        source,
    })? {
        let entry = entry.map_err(|source| GitxError::Io {
            context: format!("reading install record entry in {}", records_dir.display()),
            source,
        })?;
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        records.push(read_record(&path)?);
    }

    Ok(records)
}

fn read_record(path: &Path) -> Result<InstallRecord> {
    Ok(read_record_with_body(path)?.0)
}

fn read_record_with_body(path: &Path) -> Result<(InstallRecord, Vec<u8>)> {
    ensure_regular_file(path, "install record")?;
    let body = fs::read(path).map_err(|source| GitxError::Io {
        context: format!("reading install record {}", path.display()),
        source,
    })?;
    let record =
        serde_json::from_slice::<InstallRecord>(&body).map_err(|source| GitxError::Json {
            context: format!("decoding install record {}", path.display()),
            source,
        })?;
    validate_record(&record)?;
    Ok((record, body))
}

fn validate_record(record: &InstallRecord) -> Result<()> {
    validate_tool_name(&record.name)?;
    if !matches!(record.kind.as_str(), "npm" | "git") {
        return Err(GitxError::Message(format!(
            "invalid installed tool kind `{}`",
            record.kind
        )));
    }
    if record.source.trim().is_empty()
        || record.pinned.trim().is_empty()
        || record.source.chars().any(char::is_control)
        || record.pinned.chars().any(char::is_control)
    {
        return Err(GitxError::Message(format!(
            "invalid install record for `{}`",
            record.name
        )));
    }

    let optional_text = [
        record.description.as_deref(),
        record.homepage.as_deref(),
        record.license.as_deref(),
        record.entrypoint.as_deref(),
        record.runtime.as_deref(),
        record.integrity.as_deref(),
    ];
    if optional_text
        .into_iter()
        .flatten()
        .any(|value| value.chars().any(char::is_control))
        || record
            .binaries
            .iter()
            .chain(record.platforms.iter())
            .chain(record.requirements.iter())
            .any(|value| value.chars().any(char::is_control))
        || record.metadata.iter().any(|(key, value)| {
            key.is_empty()
                || key.chars().any(char::is_control)
                || value.chars().any(char::is_control)
        })
    {
        return Err(GitxError::Message(format!(
            "invalid metadata in install record for `{}`",
            record.name
        )));
    }

    Ok(())
}

fn ensure_regular_file(path: &Path, kind: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|source| GitxError::Io {
        context: format!("inspecting {kind} {}", path.display()),
        source,
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(GitxError::Message(format!(
            "refusing unsafe {kind} {}",
            path.display()
        )));
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpdateChange {
    field: String,
    current: String,
    resolved: String,
}

impl UpdateChange {
    fn render(&self) -> String {
        format!(
            "  {}: {} -> {}",
            terminal_safe(&self.field),
            terminal_safe(&self.current),
            terminal_safe(&self.resolved)
        )
    }
}

fn record_changes(current: &InstallRecord, resolved: &InstallRecord) -> Vec<UpdateChange> {
    let mut changes = Vec::new();
    add_change(&mut changes, "Kind", &current.kind, &resolved.kind);
    add_change(&mut changes, "Pin", &current.pinned, &resolved.pinned);
    add_optional_change(
        &mut changes,
        "Runtime",
        current.runtime.as_deref(),
        resolved.runtime.as_deref(),
    );
    add_optional_change(
        &mut changes,
        "Integrity",
        current.integrity.as_deref(),
        resolved.integrity.as_deref(),
    );
    add_optional_change(
        &mut changes,
        "Description",
        current.description.as_deref(),
        resolved.description.as_deref(),
    );
    add_optional_change(
        &mut changes,
        "Homepage",
        current.homepage.as_deref(),
        resolved.homepage.as_deref(),
    );
    add_optional_change(
        &mut changes,
        "License",
        current.license.as_deref(),
        resolved.license.as_deref(),
    );
    add_optional_change(
        &mut changes,
        "Entrypoint",
        current.entrypoint.as_deref(),
        resolved.entrypoint.as_deref(),
    );
    add_list_change(
        &mut changes,
        "Binaries",
        &current.binaries,
        &resolved.binaries,
    );
    add_list_change(
        &mut changes,
        "Platform constraints",
        &current.platforms,
        &resolved.platforms,
    );
    add_list_change(
        &mut changes,
        "Runtime requirements",
        &current.requirements,
        &resolved.requirements,
    );

    let keys: std::collections::BTreeSet<_> = current
        .metadata
        .keys()
        .chain(resolved.metadata.keys())
        .collect();
    for key in keys {
        add_optional_change(
            &mut changes,
            &format!("Metadata.{key}"),
            current.metadata.get(key).map(String::as_str),
            resolved.metadata.get(key).map(String::as_str),
        );
    }

    changes
}

fn add_change(changes: &mut Vec<UpdateChange>, field: &str, current: &str, resolved: &str) {
    if current != resolved {
        changes.push(UpdateChange {
            field: field.to_string(),
            current: current.to_string(),
            resolved: resolved.to_string(),
        });
    }
}

fn add_optional_change(
    changes: &mut Vec<UpdateChange>,
    field: &str,
    current: Option<&str>,
    resolved: Option<&str>,
) {
    if current != resolved {
        changes.push(UpdateChange {
            field: field.to_string(),
            current: current.unwrap_or("<none>").to_string(),
            resolved: resolved.unwrap_or("<none>").to_string(),
        });
    }
}

fn add_list_change(
    changes: &mut Vec<UpdateChange>,
    field: &str,
    current: &[String],
    resolved: &[String],
) {
    if current != resolved {
        changes.push(UpdateChange {
            field: field.to_string(),
            current: render_list(current),
            resolved: render_list(resolved),
        });
    }
}

fn render_list(values: &[String]) -> String {
    if values.is_empty() {
        "<none>".to_string()
    } else {
        values.join(", ")
    }
}

fn install_npm(input: &str, raw: &str, notes: &mut Vec<String>) -> Result<InstallRecord> {
    let inspection = npm::inspect_package_spec(input)?;
    if inspection.has_deny_findings {
        return Err(GitxError::Message(format!(
            "refusing to install {}@{} because its metadata has deny-level lifecycle-script findings; inspect it and use an explicit one-off execution override only if you accept that risk",
            inspection.package, inspection.version
        )));
    }
    if inspection.compatibility.status == crate::catalog::CompatibilityStatus::Incompatible {
        return Err(GitxError::Message(format!(
            "refusing to install {}@{} because it is incompatible with this host: {}",
            inspection.package, inspection.version, inspection.compatibility.summary
        )));
    }
    let name = inspection
        .bin
        .clone()
        .ok_or_else(|| {
            GitxError::Message(format!(
                "package `{}` has no single bin to install; install is limited to single-bin packages for now",
                inspection.package
            ))
        })?;

    if inspection.was_mutable {
        notes.push(format!(
            "resolved through a mutable dist-tag; pinned to exact version {}",
            inspection.version
        ));
    }
    notes.extend(
        inspection
            .warnings
            .iter()
            .map(|warning| format!("preflight: {warning}")),
    );

    Ok(InstallRecord {
        name: sanitize_key(&name),
        kind: "npm".to_string(),
        source: raw.to_string(),
        pinned: format!("npm:{}@{}", inspection.package, inspection.version),
        runtime: Some("node".to_string()),
        integrity: inspection.integrity,
        description: inspection.description,
        homepage: inspection.homepage,
        license: inspection.license,
        entrypoint: inspection.bin,
        binaries: inspection.bins,
        platforms: inspection.platforms,
        requirements: inspection.requirements,
        metadata: BTreeMap::new(),
        installed_at: unix_now(),
    })
}

fn install_git(source: &Source, notes: &mut Vec<String>) -> Result<InstallRecord> {
    let cache_paths = CachePaths::discover();
    cache_paths.ensure_base_dirs()?;

    let checkout = git::checkout_source(&cache_paths, source)?;
    let manifest = Manifest::from_path(checkout.dir.join(MANIFEST_FILE))?;
    resolve_entry(&checkout.dir, &manifest.entry)?;

    if let Some(name) = &checkout.requested_ref {
        notes.push(format!(
            "ref `{name}` is mutable; pinned to commit {}",
            checkout.commit
        ));
    }

    let pinned = match source {
        Source::Github { owner, repo, .. } => {
            format!("github:{owner}/{repo}#{}", checkout.commit)
        }
        _ => format!("{}#{}", checkout.remote, checkout.commit),
    };

    Ok(InstallRecord {
        name: sanitize_key(&manifest.name),
        kind: "git".to_string(),
        source: source.raw().to_string(),
        pinned,
        runtime: Some(manifest.runtime.clone()),
        integrity: None,
        description: manifest.description.clone(),
        homepage: manifest
            .metadata
            .get("homepage")
            .or_else(|| manifest.metadata.get("repository"))
            .or_else(|| manifest.metadata.get("url"))
            .cloned(),
        license: manifest.metadata.get("license").cloned(),
        entrypoint: Some(manifest.entry.clone()),
        binaries: Vec::new(),
        platforms: manifest
            .metadata
            .get("platforms")
            .map(|platforms| {
                platforms
                    .split(',')
                    .map(str::trim)
                    .filter(|platform| !platform.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        requirements: Vec::new(),
        metadata: manifest.metadata.clone(),
        installed_at: unix_now(),
    })
}

fn parse_install_source(input: &str) -> Result<Source> {
    match Source::parse(input) {
        Ok(source) => Ok(source),
        // bare names like `prettier` install from npm
        Err(GitxError::UnsupportedSource(_)) => Source::parse(&format!("npm:{input}")),
        Err(error) => Err(error),
    }
}

fn shim_command(record: &InstallRecord) -> String {
    match record.kind.as_str() {
        "npm" => format!(
            "exec gitx exec {} --yes -- \"$@\"",
            shell_quote(record.pinned.trim_start_matches("npm:"))
        ),
        _ => format!(
            "exec gitx run {} --yes -- \"$@\"",
            shell_quote(&record.pinned)
        ),
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn validate_tool_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && !matches!(name, "." | "..")
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '+' | '=')
        });

    if valid {
        Ok(())
    } else {
        Err(GitxError::Message(format!(
            "invalid installed tool name `{name}`; expected letters, numbers, `.`, `-`, `_`, `+`, or `=`"
        )))
    }
}

fn write_shim(path: &Path, command: &str) -> Result<()> {
    write_atomic_new(path, shim_body(command).as_bytes(), true)
}

fn write_record(paths: &InstallPaths, record: &InstallRecord) -> Result<()> {
    let path = paths.record_path(&record.name);
    let body = encode_record(&path, record)?;

    write_atomic_new(&path, &body, false)
}

fn replace_record(paths: &InstallPaths, record: &InstallRecord) -> Result<()> {
    let path = paths.record_path(&record.name);
    let body = encode_record(&path, record)?;
    write_atomic_replace(&path, &body, false)
}

fn replace_record_and_shim(
    paths: &InstallPaths,
    record: &InstallRecord,
    previous_record_body: &[u8],
) -> Result<()> {
    let record_path = paths.record_path(&record.name);
    let shim_path = paths.shim_path(&record.name);
    let record_body = encode_record(&record_path, record)?;
    let shim_body = shim_body(&shim_command(record));

    write_atomic_replace(&record_path, &record_body, false)?;
    if let Err(update_error) = write_atomic_replace(&shim_path, shim_body.as_bytes(), true) {
        if let Err(rollback_error) = write_atomic_replace(&record_path, previous_record_body, false)
        {
            return Err(GitxError::Message(format!(
                "updating `{}` failed: {update_error}; restoring the previous install record also failed: {rollback_error}",
                record.name
            )));
        }
        return Err(update_error);
    }

    Ok(())
}

fn encode_record(path: &Path, record: &InstallRecord) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(record).map_err(|source| GitxError::Json {
        context: format!("encoding install record {}", path.display()),
        source,
    })
}

fn shim_body(command: &str) -> String {
    format!("#!/bin/sh\n# generated by gitx install; do not edit\n{command}\n")
}

fn write_atomic_new(path: &Path, body: &[u8], executable: bool) -> Result<()> {
    write_atomic(path, body, executable, true)
}

fn write_atomic_replace(path: &Path, body: &[u8], executable: bool) -> Result<()> {
    write_atomic(path, body, executable, false)
}

fn write_atomic(path: &Path, body: &[u8], executable: bool, no_clobber: bool) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| GitxError::Message(format!("invalid install path {}", path.display())))?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| GitxError::Io {
            context: format!("creating temporary install file in {}", parent.display()),
            source,
        })?;
    temporary
        .write_all(body)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| GitxError::Io {
            context: format!("writing temporary install file for {}", path.display()),
            source,
        })?;

    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o755))
            .map_err(|source| GitxError::Io {
                context: format!("marking shim executable {}", path.display()),
                source,
            })?;
    }

    #[cfg(not(unix))]
    let _ = executable;

    if no_clobber {
        temporary
            .persist_noclobber(path)
            .map(|_| ())
            .map_err(|error| GitxError::Io {
                context: format!("installing {} without overwriting it", path.display()),
                source: error.error,
            })
    } else {
        temporary
            .persist(path)
            .map(|_| ())
            .map_err(|error| GitxError::Io {
                context: format!("replacing installed file {}", path.display()),
                source: error.error,
            })
    }
}

fn path_env_contains(dir: &Path) -> bool {
    let Some(paths) = env::var_os("PATH") else {
        return false;
    };

    env::split_paths(&paths).any(|entry| entry == dir)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use super::{InstallPaths, shell_quote, validate_tool_name, write_atomic_new};

    #[test]
    fn shell_quotes_pinned_sources_without_command_injection() {
        assert_eq!(
            shell_quote("github:owner/repo#abc"),
            "'github:owner/repo#abc'"
        );
        assert_eq!(
            shell_quote("file:///tmp/it's-a-tool#abc"),
            "'file:///tmp/it'\"'\"'s-a-tool#abc'"
        );
    }

    #[test]
    fn installed_tool_names_cannot_escape_managed_directories() {
        for invalid in ["", ".", "..", "../tool", "/tmp/tool", "tool/name"] {
            assert!(validate_tool_name(invalid).is_err(), "accepted {invalid:?}");
        }
        for valid in ["prettier", "+eslint=js", "tool-1.0"] {
            assert!(validate_tool_name(valid).is_ok(), "rejected {valid:?}");
        }
    }

    #[test]
    fn atomic_install_writes_do_not_replace_existing_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("tool");

        write_atomic_new(&path, b"first", false).unwrap();
        assert!(write_atomic_new(&path, b"second", false).is_err());

        assert_eq!(fs::read(path).unwrap(), b"first");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_install_roots_symlinked_to_the_current_directory() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        symlink(env::current_dir().unwrap(), &root).unwrap();

        let paths = InstallPaths { home: root };
        assert!(paths.validate_root().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_managed_directory_symlinks_that_escape_the_install_root() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, home.join("bin")).unwrap();
        let paths = InstallPaths { home };

        assert!(paths.ensure_dirs().is_err());
        assert!(fs::read_dir(outside).unwrap().next().is_none());
    }
}
