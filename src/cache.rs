use std::{
    env, fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::Command,
};

use sha2::{Digest, Sha256};

use crate::{
    artifact::{self, ImportOutcome},
    display::terminal_safe,
    error::{GitxError, Result},
    git, source,
};

const CACHE_MARKER: &str = ".gitx-cache-v1";
const CACHE_MARKER_CONTENTS: &str = "gitx-cache-v1\n";
const CACHE_MARKER_TEMP_PREFIX: &str = ".gitx-cache-marker-";

#[derive(Debug, Clone)]
pub struct CachePaths {
    root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalCacheKind {
    Npm,
    Pnpm,
    Bun,
}

impl ExternalCacheKind {
    pub fn all() -> Vec<Self> {
        vec![Self::Npm, Self::Bun, Self::Pnpm]
    }

    pub fn fastest_first() -> Vec<Self> {
        vec![Self::Npm]
    }

    fn name(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Bun => "bun",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PackageArtifactImportRequest {
    pub package: String,
    pub version: String,
    pub integrity: String,
    pub sources: Vec<ExternalCacheKind>,
    pub stop_after_first_import: bool,
}

impl CachePaths {
    #[cfg(test)]
    pub fn from_root_for_tests(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn discover() -> Self {
        if let Ok(root) = env::var("GITX_CACHE_DIR") {
            return Self {
                root: PathBuf::from(root),
            };
        }

        if let Ok(root) = env::var("XDG_CACHE_HOME") {
            return Self {
                root: PathBuf::from(root).join("gitx"),
            };
        }

        let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());

        Self {
            root: PathBuf::from(home).join(".cache").join("gitx"),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn npm_metadata_dir(&self) -> PathBuf {
        self.root.join("npm").join("metadata")
    }

    pub fn npm_backend_cache_dir(&self) -> PathBuf {
        self.root.join("npm").join("backend-cache")
    }

    pub fn npm_tags_dir(&self) -> PathBuf {
        self.root.join("npm").join("tags")
    }

    pub fn npm_versions_dir(&self) -> PathBuf {
        self.root.join("npm").join("versions")
    }

    pub fn npm_tarballs_dir(&self) -> PathBuf {
        self.root.join("npm").join("tarballs")
    }

    pub fn package_metadata_path(&self, package: &str) -> PathBuf {
        self.npm_metadata_dir()
            .join(format!("{}.json", sanitize_key(package)))
    }

    pub fn package_tag_path(&self, package: &str) -> PathBuf {
        self.npm_tags_dir()
            .join(format!("{}.json", sanitize_key(package)))
    }

    pub fn package_versions_dir(&self, package: &str) -> PathBuf {
        self.npm_versions_dir().join(sanitize_key(package))
    }

    pub fn package_version_path(&self, package: &str, version: &str) -> PathBuf {
        self.package_versions_dir(package)
            .join(format!("{}.json", sanitize_key(version)))
    }

    pub fn git_snapshots_dir(&self) -> PathBuf {
        self.root.join("git").join("snapshots")
    }

    pub fn git_remote_dir(&self, remote: &str) -> PathBuf {
        self.git_snapshots_dir().join(remote_cache_key(remote))
    }

    pub fn ensure_base_dirs(&self) -> Result<()> {
        self.ensure_cache_root()?;
        for path in [
            self.npm_metadata_dir(),
            self.npm_backend_cache_dir(),
            self.npm_tags_dir(),
            self.npm_versions_dir(),
            self.npm_tarballs_dir(),
            self.git_snapshots_dir(),
        ] {
            self.ensure_managed_dir_inner(&path)?;
        }

        Ok(())
    }

    pub(crate) fn ensure_managed_dir(&self, path: &Path) -> Result<()> {
        self.ensure_cache_root()?;
        self.ensure_managed_dir_inner(path)
    }

    /// Confirm that an existing exact-version cache can be searched without
    /// creating cache directories or claiming an arbitrary filesystem path.
    pub fn has_safe_npm_version_cache(&self) -> Result<bool> {
        validate_cache_root_for_use(&self.root)?;
        if !self.root.exists() {
            return Ok(false);
        }

        let root_metadata = fs::symlink_metadata(&self.root).map_err(|source| GitxError::Io {
            context: format!("inspecting cache root {}", self.root.display()),
            source,
        })?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(GitxError::Message(format!(
                "cache root {} is not a safe directory",
                self.root.display()
            )));
        }
        validate_cache_marker(&self.root)?;

        let versions = self.npm_versions_dir();
        if !versions.exists() {
            return Ok(false);
        }
        self.validate_existing_managed_dir(&versions)?;
        Ok(true)
    }

    fn ensure_cache_root(&self) -> Result<()> {
        validate_cache_root_for_use(&self.root)?;
        fs::create_dir_all(&self.root).map_err(|source| GitxError::Io {
            context: format!("creating cache root {}", self.root.display()),
            source,
        })?;
        let metadata = fs::symlink_metadata(&self.root).map_err(|source| GitxError::Io {
            context: format!("inspecting cache root {}", self.root.display()),
            source,
        })?;
        if !metadata.is_dir() {
            return Err(GitxError::Message(format!(
                "cache root {} is not a directory",
                self.root.display()
            )));
        }

        ensure_cache_marker(&self.root)
    }

    fn ensure_managed_dir_inner(&self, path: &Path) -> Result<()> {
        if path == self.root || !path.starts_with(&self.root) {
            return Err(GitxError::Message(format!(
                "cache directory {} is outside the managed cache root",
                path.display()
            )));
        }

        fs::create_dir_all(path).map_err(|source| GitxError::Io {
            context: format!("creating cache directory {}", path.display()),
            source,
        })?;
        self.validate_existing_managed_dir(path)
    }

    fn validate_existing_managed_dir(&self, path: &Path) -> Result<()> {
        if path == self.root || !path.starts_with(&self.root) {
            return Err(GitxError::Message(format!(
                "cache directory {} is outside the managed cache root",
                path.display()
            )));
        }
        let metadata = fs::symlink_metadata(path).map_err(|source| GitxError::Io {
            context: format!("inspecting cache directory {}", path.display()),
            source,
        })?;
        let canonical_root = self.root.canonicalize().map_err(|source| GitxError::Io {
            context: format!("resolving cache root {}", self.root.display()),
            source,
        })?;
        let resolved = path.canonicalize().map_err(|source| GitxError::Io {
            context: format!("resolving cache directory {}", path.display()),
            source,
        })?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || !resolved.starts_with(&canonical_root)
            || resolved == canonical_root
        {
            return Err(GitxError::Message(format!(
                "refusing unsafe cache directory {}",
                path.display()
            )));
        }

        Ok(())
    }
}

pub fn list_cache() -> Result<String> {
    let paths = CachePaths::discover();
    paths.ensure_base_dirs()?;

    let metadata_count = count_files(&paths.npm_metadata_dir())?;
    let tag_count = count_files(&paths.npm_tags_dir())?;
    let version_count = count_files_recursive(&paths.npm_versions_dir())?;
    let tarball_count = count_files(&paths.npm_tarballs_dir())?;
    let backend_entries = count_entries(&paths.npm_backend_cache_dir())?;
    let git_snapshots = list_git_snapshots(&paths)?;

    let mut output = vec![
        format!("Cache root: {}", display_path(paths.root())),
        format!("NPM metadata entries: {metadata_count}"),
        format!("NPM tag pointers: {tag_count}"),
        format!("NPM exact versions: {version_count}"),
        format!("NPM tarballs: {tarball_count}"),
        format!("NPM backend cache entries: {backend_entries}"),
        format!("Git snapshots: {}", git_snapshots.len()),
    ];

    for (remote, commit) in git_snapshots {
        output.push(format!(
            "  {} @ {}",
            terminal_safe(&remote),
            terminal_safe(&commit)
        ));
    }

    Ok(output.join("\n"))
}

pub fn import_package_artifact(request: PackageArtifactImportRequest) -> Result<String> {
    let paths = CachePaths::discover();
    paths.ensure_base_dirs()?;

    let sources = if request.sources.is_empty() {
        ExternalCacheKind::all()
    } else {
        request.sources
    };
    let target = artifact::tarball_path(paths.root(), &request.integrity)?;
    let mut output = vec![
        format!("Import target: {}@{}", request.package, request.version),
        format!("GitX cache: {}", display_path(paths.root())),
    ];

    if target.exists() {
        artifact::verify_file(&target, &request.integrity)?;
        output.push(format!("Already cached: {}", display_path(&target)));
        return Ok(output.join("\n"));
    }

    let mut imported = false;
    for source in sources {
        match source {
            ExternalCacheKind::Npm => {
                let result = import_from_npm_cache(&paths, &request.integrity)?;
                output.extend(result.lines);
                imported |= result.imported;
                if result.imported && request.stop_after_first_import {
                    break;
                }
            }
            ExternalCacheKind::Pnpm => {
                if request.stop_after_first_import {
                    output.push(
                        "pnpm: skipped in auto import; no original tarball import support yet"
                            .to_string(),
                    );
                } else {
                    output.extend(report_unpacked_store(
                        ExternalCacheKind::Pnpm,
                        pnpm_store_dirs(),
                        "pnpm stores unpacked file content, not the original npm tarball",
                    ));
                }
            }
            ExternalCacheKind::Bun => {
                if request.stop_after_first_import {
                    output.push(
                        "bun: skipped in auto import; no original tarball import support yet"
                            .to_string(),
                    );
                } else {
                    output.extend(report_unpacked_store(
                        ExternalCacheKind::Bun,
                        bun_cache_dirs(),
                        "bun stores unpacked package directories, not the original npm tarball",
                    ));
                }
            }
        }
    }

    if !imported {
        output.push(
            "No importable tarball was found; external caches were left untouched.".to_string(),
        );
    }

    Ok(output.join("\n"))
}

#[derive(Debug, Default)]
struct ImportAttempt {
    imported: bool,
    lines: Vec<String>,
}

fn import_from_npm_cache(paths: &CachePaths, integrity: &str) -> Result<ImportAttempt> {
    let hex = artifact::integrity_sha512_hex(integrity)?;
    let cache_dirs = npm_cache_dirs(paths);
    let mut attempt = ImportAttempt::default();

    if cache_dirs.is_empty() {
        attempt
            .lines
            .push("npm: no cache directory discovered".to_string());
        return Ok(attempt);
    }

    for cache_dir in &cache_dirs {
        let candidate = npm_cacache_content_path(cache_dir, &hex);
        if !candidate.is_file() {
            continue;
        }

        match artifact::import_verified_tarball(paths.root(), &candidate, integrity)? {
            ImportOutcome::AlreadyCached(path) => {
                attempt
                    .lines
                    .push(format!("npm: already cached at {}", display_path(&path)));
            }
            ImportOutcome::Imported(path) => {
                attempt.lines.push(format!(
                    "npm: imported {} -> {}",
                    display_path(&candidate),
                    display_path(&path)
                ));
            }
        }
        attempt.imported = true;
        return Ok(attempt);
    }

    attempt.lines.push(format!(
        "npm: checked {} cache dir(s); no matching tarball",
        cache_dirs.len()
    ));
    Ok(attempt)
}

fn npm_cache_dirs(paths: &CachePaths) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    push_unique_path(&mut dirs, paths.npm_backend_cache_dir());

    if let Ok(cache) = env::var("npm_config_cache") {
        push_unique_path(&mut dirs, PathBuf::from(cache));
    }

    if let Ok(home) = env::var("HOME") {
        push_unique_path(&mut dirs, PathBuf::from(home).join(".npm"));
    }

    if let Some(cache) = command_stdout("npm", ["config", "get", "cache"]) {
        push_unique_path(&mut dirs, PathBuf::from(cache));
    }

    dirs
}

fn pnpm_store_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(store) = env::var("npm_config_store_dir") {
        push_unique_path(&mut dirs, PathBuf::from(store));
    }

    if let Some(store) = command_stdout("pnpm", ["store", "path"]) {
        push_unique_path(&mut dirs, PathBuf::from(store));
    }

    if let Ok(home) = env::var("HOME") {
        push_unique_path(
            &mut dirs,
            PathBuf::from(home).join(".local/share/pnpm/store"),
        );
    }

    dirs
}

fn bun_cache_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(cache) = env::var("BUN_INSTALL_CACHE_DIR") {
        push_unique_path(&mut dirs, PathBuf::from(cache));
    }

    if let Ok(home) = env::var("HOME") {
        push_unique_path(&mut dirs, PathBuf::from(home).join(".bun/install/cache"));
    }

    dirs
}

fn report_unpacked_store(kind: ExternalCacheKind, dirs: Vec<PathBuf>, reason: &str) -> Vec<String> {
    if dirs.is_empty() {
        return vec![format!("{}: no cache directory discovered", kind.name())];
    }

    vec![format!(
        "{}: discovered {} cache dir(s), but skipped import: {reason}",
        kind.name(),
        dirs.len()
    )]
}

fn command_stdout<const N: usize>(program: &str, args: [&str; N]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn npm_cacache_content_path(cache_dir: &Path, sha512_hex: &str) -> PathBuf {
    let (first, rest) = sha512_hex.split_at(2);
    let (second, remaining) = rest.split_at(2);

    cache_dir
        .join("_cacache")
        .join("content-v2")
        .join("sha512")
        .join(first)
        .join(second)
        .join(remaining)
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if path.as_os_str().is_empty() || paths.iter().any(|existing| existing == &path) {
        return;
    }

    paths.push(path);
}

fn list_git_snapshots(paths: &CachePaths) -> Result<Vec<(String, String)>> {
    let snapshots_dir = paths.git_snapshots_dir();
    if !snapshots_dir.exists() {
        return Ok(Vec::new());
    }

    let mut snapshots = Vec::new();
    for remote_entry in read_dir_entries(&snapshots_dir)? {
        if !remote_entry.is_dir() {
            continue;
        }

        for commit_entry in read_dir_entries(&remote_entry)? {
            let commit_name = file_name_string(&commit_entry);
            if commit_entry.is_dir()
                && git::is_commit_dir_name(&commit_name)
                && let Some((remote, commit)) = git::snapshot_identity(&remote_entry, &commit_name)
            {
                snapshots.push((remote, commit));
            }
        }
    }

    snapshots.sort();
    Ok(snapshots)
}

fn read_dir_entries(path: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path).map_err(|source| GitxError::Io {
        context: format!("reading cache directory {}", path.display()),
        source,
    })? {
        let entry = entry.map_err(|source| GitxError::Io {
            context: format!("reading cache directory entry in {}", path.display()),
            source,
        })?;
        entries.push(entry.path());
    }

    Ok(entries)
}

fn file_name_string(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

pub fn clear_cache() -> Result<String> {
    let paths = CachePaths::discover();
    validate_cache_root_for_use(paths.root())?;

    if paths.root().exists() {
        validate_cache_root_for_removal(paths.root())?;
        validate_cache_marker(paths.root())?;
        fs::remove_dir_all(paths.root()).map_err(|source| GitxError::Io {
            context: format!("clearing cache {}", paths.root().display()),
            source,
        })?;
    }

    Ok(format!("Cleared cache: {}", display_path(paths.root())))
}

pub fn remove_cache_key(key: &str) -> Result<String> {
    if key.is_empty() || key.chars().any(char::is_control) {
        return Err(GitxError::Message(
            "cache keys cannot be empty or contain control characters".to_string(),
        ));
    }

    let paths = CachePaths::discover();
    paths.ensure_base_dirs()?;
    let mut removed = Vec::new();

    if let Some((package, version)) = split_package_version_key(key) {
        let version_path = paths.package_version_path(package, version);
        if version_path.exists() {
            fs::remove_file(&version_path).map_err(|source| GitxError::Io {
                context: format!("removing cache file {}", version_path.display()),
                source,
            })?;
            removed.push(format!("version {package}@{version}"));
        }
    }

    let metadata_path = paths.package_metadata_path(key);

    if metadata_path.exists() {
        fs::remove_file(&metadata_path).map_err(|source| GitxError::Io {
            context: format!("removing cache file {}", metadata_path.display()),
            source,
        })?;

        removed.push(format!("metadata {key}"));
    }

    let tag_path = paths.package_tag_path(key);
    if tag_path.exists() {
        fs::remove_file(&tag_path).map_err(|source| GitxError::Io {
            context: format!("removing cache file {}", tag_path.display()),
            source,
        })?;

        removed.push(format!("tag pointers {key}"));
    }

    let versions_dir = paths.package_versions_dir(key);
    if versions_dir.exists() {
        fs::remove_dir_all(&versions_dir).map_err(|source| GitxError::Io {
            context: format!("removing cache directory {}", versions_dir.display()),
            source,
        })?;

        removed.push(format!("versions {key}"));
    }

    removed.extend(remove_git_snapshots(&paths, key)?);

    if !removed.is_empty() {
        return Ok(format!("Removed cached {}", removed.join(", ")));
    }

    Err(GitxError::Message(format!(
        "no cache entry found for `{key}`; cache root is {}",
        paths.root().display()
    )))
}

fn display_path(path: &Path) -> String {
    terminal_safe(&path.display().to_string())
}

fn ensure_cache_marker(root: &Path) -> Result<()> {
    let marker = root.join(CACHE_MARKER);
    if marker.exists() {
        return validate_cache_marker(root);
    }

    let claimable = fs::read_dir(root)
        .map_err(|source| GitxError::Io {
            context: format!("reading cache root {}", root.display()),
            source,
        })?
        .all(|entry| {
            entry.is_ok_and(|entry| {
                let name = entry.file_name();
                name.to_str().is_some_and(|name| {
                    matches!(name, "npm" | "git") || name.starts_with(CACHE_MARKER_TEMP_PREFIX)
                })
            })
        });
    if !claimable {
        return Err(GitxError::Message(format!(
            "refusing to claim non-GitX directory as cache root {}",
            root.display()
        )));
    }

    let mut temporary = tempfile::Builder::new()
        .prefix(CACHE_MARKER_TEMP_PREFIX)
        .tempfile_in(root)
        .map_err(|source| GitxError::Io {
            context: format!("creating cache marker in {}", root.display()),
            source,
        })?;
    temporary
        .write_all(CACHE_MARKER_CONTENTS.as_bytes())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| GitxError::Io {
            context: format!("writing cache marker in {}", root.display()),
            source,
        })?;
    match temporary.persist_noclobber(&marker) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            validate_cache_marker(root)
        }
        Err(error) => Err(GitxError::Io {
            context: format!("persisting cache marker {}", marker.display()),
            source: error.error,
        }),
    }
}

fn validate_cache_marker(root: &Path) -> Result<()> {
    let marker = root.join(CACHE_MARKER);
    let valid = fs::symlink_metadata(&marker)
        .ok()
        .filter(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        .and_then(|_| fs::read_to_string(&marker).ok())
        .is_some_and(|contents| contents == CACHE_MARKER_CONTENTS);
    if !valid {
        return Err(GitxError::Message(format!(
            "refusing cache operation without a valid GitX marker in {}",
            root.display()
        )));
    }

    Ok(())
}

fn count_files(path: &Path) -> Result<usize> {
    if !path.exists() {
        return Ok(0);
    }

    let mut count = 0;
    for entry in fs::read_dir(path).map_err(|source| GitxError::Io {
        context: format!("reading cache directory {}", path.display()),
        source,
    })? {
        let entry = entry.map_err(|source| GitxError::Io {
            context: format!("reading cache directory entry in {}", path.display()),
            source,
        })?;
        if entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            count += 1;
        }
    }

    Ok(count)
}

fn count_entries(path: &Path) -> Result<usize> {
    if !path.exists() {
        return Ok(0);
    }

    let count = fs::read_dir(path)
        .map_err(|source| GitxError::Io {
            context: format!("reading cache directory {}", path.display()),
            source,
        })?
        .count();

    Ok(count)
}

fn count_files_recursive(path: &Path) -> Result<usize> {
    if !path.exists() {
        return Ok(0);
    }

    let mut count = 0;
    for entry in fs::read_dir(path).map_err(|source| GitxError::Io {
        context: format!("reading cache directory {}", path.display()),
        source,
    })? {
        let entry = entry.map_err(|source| GitxError::Io {
            context: format!("reading cache directory entry in {}", path.display()),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| GitxError::Io {
            context: format!("reading cache file type {}", entry.path().display()),
            source,
        })?;
        if file_type.is_dir() {
            count += count_files_recursive(&entry.path())?;
        } else if file_type.is_file() {
            count += 1;
        }
    }

    Ok(count)
}

fn remove_git_snapshots(paths: &CachePaths, key: &str) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    let snapshots_dir = paths.git_snapshots_dir();
    if !snapshots_dir.exists() {
        return Ok(removed);
    }

    // a git source key removes snapshots for that remote, optionally
    // narrowed to a single pinned commit
    if let Ok(parsed) = source::Source::parse(key)
        && let Some(remote) = parsed.resolved_remote()
    {
        let remote_dir = paths.git_remote_dir(remote);
        if remote_dir.exists() {
            match parsed.resolved_commit() {
                Some(commit) => {
                    removed.extend(remove_snapshots_by_prefix(&remote_dir, commit, remote)?);
                }
                None => {
                    fs::remove_dir_all(&remote_dir).map_err(|source| GitxError::Io {
                        context: format!("removing cache directory {}", remote_dir.display()),
                        source,
                    })?;
                    removed.push(format!("git snapshots for {remote}"));
                }
            }
        }
        return Ok(removed);
    }

    // a bare hex key removes matching commits across all remotes
    if key.len() >= 7 && key.chars().all(|character| character.is_ascii_hexdigit()) {
        for remote_entry in read_dir_entries(&snapshots_dir)? {
            if !remote_entry.is_dir() {
                continue;
            }
            let remote_name = file_name_string(&remote_entry);
            removed.extend(remove_snapshots_by_prefix(
                &remote_entry,
                key,
                &remote_name,
            )?);
        }
    }

    Ok(removed)
}

fn remove_snapshots_by_prefix(
    remote_dir: &Path,
    prefix: &str,
    remote_label: &str,
) -> Result<Vec<String>> {
    let prefix = prefix.to_ascii_lowercase();
    let mut removed = Vec::new();

    for entry in read_dir_entries(remote_dir)? {
        let name = file_name_string(&entry);
        if !entry.is_dir() || !git::is_commit_dir_name(&name) || !name.starts_with(&prefix) {
            continue;
        }

        fs::remove_dir_all(&entry).map_err(|source| GitxError::Io {
            context: format!("removing cache directory {}", entry.display()),
            source,
        })?;

        let marker = git::snapshot_marker_path(remote_dir, &name);
        if marker.exists() {
            fs::remove_file(&marker).map_err(|source| GitxError::Io {
                context: format!("removing cache file {}", marker.display()),
                source,
            })?;
        }

        removed.push(format!("git snapshot {remote_label} @ {name}"));
    }

    Ok(removed)
}

fn remote_cache_key(remote: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(remote.as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

pub fn sanitize_key(key: &str) -> String {
    let sanitized: String = key
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => character,
            '@' => '+',
            '/' => '=',
            _ => '_',
        })
        .collect();

    if sanitized.is_empty() || matches!(sanitized.as_str(), "." | "..") {
        format!("_{sanitized}")
    } else {
        sanitized
    }
}

fn validate_cache_root_for_removal(root: &Path) -> Result<()> {
    validate_cache_root_for_use(root)?;

    let canonical = root.canonicalize().map_err(|source| GitxError::Io {
        context: format!("resolving cache root {}", root.display()),
        source,
    })?;
    let metadata = fs::symlink_metadata(root).map_err(|source| GitxError::Io {
        context: format!("inspecting cache root before removal {}", root.display()),
        source,
    })?;
    let current = env::current_dir()
        .and_then(|path| path.canonicalize())
        .map_err(|source| GitxError::Io {
            context: "resolving current directory before clearing cache".to_string(),
            source,
        })?;
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| resolve_with_existing_ancestor(&path))
        .transpose()?;

    let unsafe_root = metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || canonical.parent().is_none()
        || current.starts_with(&canonical)
        || home
            .as_ref()
            .is_some_and(|home| home.starts_with(&canonical));
    if unsafe_root {
        return Err(GitxError::Message(format!(
            "refusing to clear unsafe cache root {}",
            canonical.display()
        )));
    }

    Ok(())
}

fn validate_cache_root_for_use(root: &Path) -> Result<()> {
    if root.as_os_str().is_empty()
        || root.to_string_lossy().chars().any(char::is_control)
        || root
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(GitxError::Message(format!(
            "refusing unsafe cache root {}",
            root.display()
        )));
    }

    let absolute = if root.is_absolute() {
        root.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|source| GitxError::Io {
                context: "resolving current directory for cache root".to_string(),
                source,
            })?
            .join(root)
    };
    let resolved = resolve_with_existing_ancestor(&absolute)?;
    let current = env::current_dir()
        .and_then(|path| path.canonicalize())
        .map_err(|source| GitxError::Io {
            context: "resolving current directory for cache root".to_string(),
            source,
        })?;
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| resolve_with_existing_ancestor(&path))
        .transpose()?;

    if resolved.parent().is_none()
        || resolved == current
        || home.as_ref().is_some_and(|home| resolved == *home)
    {
        return Err(GitxError::Message(format!(
            "refusing unsafe cache root {}",
            root.display()
        )));
    }

    Ok(())
}

fn resolve_with_existing_ancestor(path: &Path) -> Result<PathBuf> {
    let mut ancestor = path;
    while !ancestor.exists() {
        ancestor = ancestor.parent().ok_or_else(|| {
            GitxError::Message(format!("cannot resolve cache path {}", path.display()))
        })?;
    }

    let canonical_ancestor = ancestor.canonicalize().map_err(|source| GitxError::Io {
        context: format!("resolving cache path ancestor {}", ancestor.display()),
        source,
    })?;
    let tail = path
        .strip_prefix(ancestor)
        .map_err(|_| GitxError::Message(format!("cannot resolve cache path {}", path.display())))?;

    Ok(canonical_ancestor.join(tail))
}

fn split_package_version_key(key: &str) -> Option<(&str, &str)> {
    let index = key.rfind('@').filter(|index| *index > 0)?;
    let (package, version_with_at) = key.split_at(index);
    let version = version_with_at.strip_prefix('@')?;

    if package.is_empty() || version.is_empty() {
        None
    } else {
        Some((package, version))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CachePaths, ExternalCacheKind, npm_cacache_content_path, remote_cache_key, sanitize_key,
        split_package_version_key,
    };
    use std::path::Path;

    #[test]
    fn sanitizes_scoped_package_names() {
        assert_eq!(sanitize_key("@eslint/js"), "+eslint=js");
        assert_eq!(sanitize_key(".."), "_..");
        assert_eq!(sanitize_key(""), "_");
    }

    #[test]
    fn hashes_git_remotes_without_scheme_collisions() {
        let https = remote_cache_key("https://example.test/owner/tool.git");
        let ssh = remote_cache_key("ssh://example.test/owner/tool.git");

        assert_ne!(https, ssh);
        assert_eq!(https.len(), 64);
        assert!(https.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn rejects_an_empty_cache_root() {
        let paths = CachePaths::from_root_for_tests("");

        assert!(paths.ensure_base_dirs().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_cache_directory_symlinks_outside_the_root() {
        use std::{fs, os::unix::fs::symlink};

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("cache");
        let outside = temp.path().join("outside");
        fs::create_dir_all(root.join("npm")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join("npm/metadata")).unwrap();
        let paths = CachePaths::from_root_for_tests(root);

        assert!(paths.ensure_base_dirs().is_err());
        assert!(fs::read_dir(outside).unwrap().next().is_none());
    }

    #[test]
    fn splits_scoped_package_version_keys() {
        assert_eq!(
            split_package_version_key("@eslint/js@9.30.0"),
            Some(("@eslint/js", "9.30.0"))
        );
    }

    #[test]
    fn maps_sha512_hex_to_npm_cacache_content_path() {
        let path = npm_cacache_content_path(Path::new("/cache"), "abcdef");

        assert_eq!(
            path,
            Path::new("/cache")
                .join("_cacache")
                .join("content-v2")
                .join("sha512")
                .join("ab")
                .join("cd")
                .join("ef")
        );
    }

    #[test]
    fn orders_external_cache_imports_fastest_first() {
        assert_eq!(
            ExternalCacheKind::fastest_first(),
            vec![ExternalCacheKind::Npm]
        );
    }
}
