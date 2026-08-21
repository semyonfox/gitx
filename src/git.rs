use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    cache::CachePaths,
    error::{GitxError, Result},
    report,
    source::{RefSpec, Source},
    toolchain,
};

#[derive(Debug, Clone)]
pub struct GitCheckout {
    pub remote: String,
    pub requested_ref: Option<String>,
    pub commit: String,
    pub dir: PathBuf,
    pub cache_hit: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotRecord {
    remote: String,
    commit: String,
    requested_ref: Option<String>,
    fetched_at: u64,
}

/// Resolve a git source to an exact commit and materialize a pinned snapshot
/// of that commit in the GitX cache.
pub fn checkout_source(paths: &CachePaths, source: &Source) -> Result<GitCheckout> {
    report::step("Resolving a Git source");
    let remote = source
        .resolved_remote()
        .ok_or_else(|| GitxError::Message(format!("`{}` is not a git source", source.raw())))?;
    validate_remote(remote)?;

    if !toolchain::exists_on_path("git") {
        return Err(GitxError::MissingToolchain("git".to_string()));
    }

    match source.reference() {
        Some(RefSpec::Commit(commit)) => checkout_commit(paths, remote, commit, None),
        Some(RefSpec::Named(name)) => {
            validate_ref_name(name)?;
            let commit = resolve_ref(remote, name)?;
            checkout_commit(paths, remote, &commit, Some(name))
        }
        None => {
            let commit = resolve_ref(remote, "HEAD")?;
            checkout_commit(paths, remote, &commit, Some("HEAD"))
        }
    }
}

/// Resolve a named ref to a full commit hash with `git ls-remote`.
/// Tags win over branches, matching git's own ref disambiguation order.
pub fn resolve_ref(remote: &str, name: &str) -> Result<String> {
    validate_remote(remote)?;
    validate_ref_name(name)?;
    let peeled_pattern = format!("{name}^{{}}");
    let output = run_git(
        None,
        &["ls-remote", "--", remote, name, &peeled_pattern],
        &format!("resolving `{name}` on {remote}"),
    )?;

    let mut head = None;
    let mut peeled_tag = None;
    let mut tag = None;
    let mut branch = None;
    let mut other = None;

    for line in output.lines() {
        let mut parts = line.split_whitespace();
        let (Some(sha), Some(refname)) = (parts.next(), parts.next()) else {
            continue;
        };

        let slot = if refname.ends_with("^{}") {
            &mut peeled_tag
        } else if refname == "HEAD" {
            &mut head
        } else if refname.starts_with("refs/tags/") {
            &mut tag
        } else if refname.starts_with("refs/heads/") {
            &mut branch
        } else {
            &mut other
        };

        slot.get_or_insert(sha.to_string());
    }

    peeled_tag
        .or(tag)
        .or(branch)
        .or(head)
        .or(other)
        .ok_or_else(|| GitxError::Message(format!("remote {remote} has no ref matching `{name}`")))
}

fn checkout_commit(
    paths: &CachePaths,
    remote: &str,
    commit: &str,
    requested_ref: Option<&str>,
) -> Result<GitCheckout> {
    let commit = commit.to_ascii_lowercase();

    if !(7..=40).contains(&commit.len())
        || !commit
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(GitxError::Message(format!(
            "`{commit}` is not a valid commit hash; expected 7 to 40 hex characters"
        )));
    }

    let remote_dir = paths.git_remote_dir(remote);

    if let Some((full_commit, dir)) = find_cached_snapshot(&remote_dir, remote, &commit)? {
        report::step("Using a cached Git snapshot");
        return Ok(GitCheckout {
            remote: remote.to_string(),
            requested_ref: requested_ref.map(str::to_string),
            commit: full_commit,
            dir,
            cache_hit: true,
        });
    }

    paths.ensure_managed_dir(&remote_dir)?;
    report::step("Fetching a pinned Git snapshot");

    let work = remote_dir.join(format!(".fetch-{}-{}", commit, std::process::id()));
    if work.exists() {
        remove_dir(&work)?;
    }
    fs::create_dir_all(&work).map_err(|source| GitxError::Io {
        context: format!("creating fetch workspace {}", work.display()),
        source,
    })?;

    let result = fetch_into(&work, remote, &commit, requested_ref);
    let full_commit = match result {
        Ok(full_commit) => full_commit,
        Err(error) => {
            let _ = fs::remove_dir_all(&work);
            return Err(error);
        }
    };

    remove_dir(&work.join(".git"))?;

    let final_dir = remote_dir.join(&full_commit);
    if final_dir.exists() {
        if snapshot_is_valid(&remote_dir, remote, &full_commit) {
            // Another process completed the same snapshot first.
            remove_dir(&work)?;
        } else {
            // Never bless a directory left behind before its completion marker.
            remove_dir(&final_dir)?;
            let marker = snapshot_marker_path(&remote_dir, &full_commit);
            if marker.exists() {
                fs::remove_file(&marker).map_err(|source| GitxError::Io {
                    context: format!("removing corrupt snapshot marker {}", marker.display()),
                    source,
                })?;
            }
            move_snapshot(&work, &final_dir)?;
        }
    } else {
        move_snapshot(&work, &final_dir)?;
    }

    let record = SnapshotRecord {
        remote: remote.to_string(),
        commit: full_commit.clone(),
        requested_ref: requested_ref.map(str::to_string),
        fetched_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default(),
    };
    let marker = snapshot_marker_path(&remote_dir, &full_commit);
    let body = serde_json::to_string_pretty(&record).map_err(|source| GitxError::Json {
        context: format!("encoding snapshot record {}", marker.display()),
        source,
    })?;
    fs::write(&marker, body).map_err(|source| GitxError::Io {
        context: format!("writing snapshot record {}", marker.display()),
        source,
    })?;

    Ok(GitCheckout {
        remote: remote.to_string(),
        requested_ref: requested_ref.map(str::to_string),
        commit: full_commit,
        dir: final_dir,
        cache_hit: false,
    })
}

/// Fetch a single commit into a fresh workspace and check it out,
/// mirroring how CI checkouts pin exact commits.
fn fetch_into(
    work: &Path,
    remote: &str,
    commit: &str,
    requested_ref: Option<&str>,
) -> Result<String> {
    run_git(
        Some(work),
        &["init", "--quiet"],
        "initializing fetch workspace",
    )?;
    run_git(
        Some(work),
        &["remote", "add", "origin", remote],
        "configuring fetch remote",
    )?;

    let by_commit = run_git(
        Some(work),
        &["fetch", "--quiet", "--depth", "1", "origin", commit],
        &format!("fetching commit {commit} from {remote}"),
    );

    if let Err(fetch_error) = by_commit {
        // some servers refuse direct commit fetches; fall back to the named
        // ref and verify it still points at the commit we resolved
        let Some(name) = requested_ref else {
            if commit.len() < 40 {
                return Err(GitxError::Message(format!(
                    "could not fetch `{commit}` directly; use the full 40-character commit hash ({fetch_error})"
                )));
            }
            return Err(fetch_error);
        };

        run_git(
            Some(work),
            &["fetch", "--quiet", "--depth", "1", "origin", name],
            &format!("fetching ref `{name}` from {remote}"),
        )?;
        let fetched = run_git(
            Some(work),
            &["rev-parse", "FETCH_HEAD"],
            "reading FETCH_HEAD",
        )?
        .trim()
        .to_string();
        if fetched != commit {
            return Err(GitxError::Message(format!(
                "remote ref `{name}` moved while fetching: expected {commit}, got {fetched}"
            )));
        }
    }

    let isolated_config = work.join(".git").join("gitx-empty-global-config");
    fs::write(&isolated_config, []).map_err(|source| GitxError::Io {
        context: format!(
            "creating isolated checkout config {}",
            isolated_config.display()
        ),
        source,
    })?;
    run_git_isolated(
        Some(work),
        &["checkout", "--quiet", "--detach", "FETCH_HEAD"],
        "checking out fetched commit",
        &isolated_config,
    )?;

    let full_commit = run_git(
        Some(work),
        &["rev-parse", "HEAD"],
        "reading checked out commit",
    )?
    .trim()
    .to_string();

    if !full_commit.starts_with(commit) {
        return Err(GitxError::Message(format!(
            "fetched commit {full_commit} does not match requested {commit}"
        )));
    }

    Ok(full_commit)
}

fn find_cached_snapshot(
    remote_dir: &Path,
    remote: &str,
    commit: &str,
) -> Result<Option<(String, PathBuf)>> {
    if !remote_dir.exists() {
        return Ok(None);
    }

    let entries = fs::read_dir(remote_dir).map_err(|source| GitxError::Io {
        context: format!("reading git cache directory {}", remote_dir.display()),
        source,
    })?;

    let mut matches = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| GitxError::Io {
            context: format!("reading git cache entry in {}", remote_dir.display()),
            source,
        })?;
        let name = entry.file_name().to_string_lossy().to_string();

        if !is_commit_dir_name(&name) || !name.starts_with(commit) {
            continue;
        }

        if entry.path().is_dir() && snapshot_is_valid(remote_dir, remote, &name) {
            matches.push((name, entry.path()));
        }
    }

    matches.sort_by(|left, right| left.0.cmp(&right.0));
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        _ => Err(GitxError::Message(format!(
            "commit prefix `{commit}` is ambiguous in the local cache; use a longer hash"
        ))),
    }
}

fn snapshot_is_valid(remote_dir: &Path, remote: &str, commit: &str) -> bool {
    read_snapshot_record(remote_dir, commit)
        .is_some_and(|record| record.remote == remote && record.commit == commit)
}

pub(crate) fn snapshot_identity(remote_dir: &Path, commit: &str) -> Option<(String, String)> {
    let record = read_snapshot_record(remote_dir, commit)?;
    (record.commit == commit).then_some((record.remote, record.commit))
}

fn read_snapshot_record(remote_dir: &Path, commit: &str) -> Option<SnapshotRecord> {
    let marker = snapshot_marker_path(remote_dir, commit);
    let body = fs::read_to_string(marker).ok()?;
    serde_json::from_str(&body).ok()
}

pub fn is_commit_dir_name(name: &str) -> bool {
    name.len() == 40 && name.chars().all(|character| character.is_ascii_hexdigit())
}

pub fn snapshot_marker_path(remote_dir: &Path, commit: &str) -> PathBuf {
    remote_dir.join(format!("{commit}.complete"))
}

fn remove_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    fs::remove_dir_all(path).map_err(|source| GitxError::Io {
        context: format!("removing {}", path.display()),
        source,
    })
}

fn move_snapshot(work: &Path, final_dir: &Path) -> Result<()> {
    fs::rename(work, final_dir).map_err(|source| GitxError::Io {
        context: format!("moving snapshot into {}", final_dir.display()),
        source,
    })
}

fn validate_remote(remote: &str) -> Result<()> {
    let supported_transport = ["https://", "http://", "ssh://", "git://", "file://"]
        .iter()
        .any(|prefix| remote.starts_with(prefix))
        || remote.starts_with("git@")
        || (!remote.contains("://") && !remote.contains("::") && remote.ends_with(".git"));

    if remote.is_empty()
        || remote.starts_with('-')
        || remote.chars().any(char::is_control)
        || !supported_transport
    {
        return Err(GitxError::Message(format!(
            "invalid or unsupported git remote `{remote}`; expected HTTPS, HTTP, SSH, git, file, git@host:path, or a local .git path"
        )));
    }
    Ok(())
}

fn validate_ref_name(name: &str) -> Result<()> {
    if name.is_empty() || name.starts_with('-') || name.chars().any(char::is_control) {
        return Err(GitxError::Message(format!("invalid git ref `{name}`")));
    }

    let output = Command::new("git")
        .args(["check-ref-format", "--allow-onelevel", name])
        .output()
        .map_err(|source| GitxError::Io {
            context: format!("validating git ref `{name}`"),
            source,
        })?;
    if !output.status.success() {
        return Err(GitxError::Message(format!("invalid git ref `{name}`")));
    }
    Ok(())
}

fn run_git(dir: Option<&Path>, args: &[&str], context: &str) -> Result<String> {
    run_git_command(dir, args, context, None)
}

fn run_git_isolated(
    dir: Option<&Path>,
    args: &[&str],
    context: &str,
    empty_global_config: &Path,
) -> Result<String> {
    run_git_command(dir, args, context, Some(empty_global_config))
}

fn run_git_command(
    dir: Option<&Path>,
    args: &[&str],
    context: &str,
    empty_global_config: Option<&Path>,
) -> Result<String> {
    let mut command = Command::new("git");
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    command
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES");
    if let Some(config) = empty_global_config {
        command
            .env("GIT_CONFIG_GLOBAL", config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS");
    }

    let activity = report::activity(git_activity_label(args));
    let output = command.output().map_err(|source| GitxError::Io {
        context: format!("running git while {context}"),
        source,
    })?;
    activity.finish_quietly();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(GitxError::Git {
            context: context.to_string(),
            stderr: if stderr.is_empty() {
                format!("exited with {}", output.status)
            } else {
                stderr
            },
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn git_activity_label(args: &[&str]) -> &'static str {
    match args.first().copied() {
        Some("ls-remote") => "Resolving remote Git ref",
        Some("fetch") => "Fetching Git snapshot",
        Some("checkout") => "Checking out Git snapshot",
        Some("init") | Some("remote") => "Preparing Git workspace",
        _ => "Checking Git snapshot",
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, process::Command};

    use super::{checkout_source, is_commit_dir_name, snapshot_marker_path, validate_remote};
    use crate::{cache::CachePaths, source::Source};

    #[test]
    fn recognizes_full_commit_dir_names() {
        assert!(is_commit_dir_name(&"a".repeat(40)));
        assert!(!is_commit_dir_name("abc123"));
        assert!(!is_commit_dir_name(&"z".repeat(40)));
    }

    #[test]
    fn rejects_remote_helpers_that_can_execute_during_resolution() {
        assert!(validate_remote("ext::sh -c exploit.git").is_err());
        assert!(validate_remote("custom://example.test/tool.git").is_err());
        assert!(validate_remote("https://example.test/tool.git").is_ok());
        assert!(validate_remote("git@example.test:owner/tool.git").is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn resolves_named_refs_and_reuses_commit_cache() {
        if !git_available() {
            return;
        }

        let fixture = GitFixture::new();
        let cache = tempfile::tempdir().unwrap();
        let paths = CachePaths::from_root_for_tests(cache.path());
        let source = Source::parse(&format!("{}@main", fixture.remote_url)).unwrap();

        let first = checkout_source(&paths, &source).unwrap();
        let second = checkout_source(&paths, &source).unwrap();

        assert_eq!(first.commit, fixture.first_commit);
        assert!(!first.cache_hit);
        assert_eq!(second.commit, first.commit);
        assert!(second.cache_hit);
        assert_eq!(
            fs::read_to_string(second.dir.join("index.js")).unwrap(),
            "v1\n"
        );
    }

    #[test]
    #[cfg(unix)]
    fn refreshes_a_moved_named_ref_without_losing_the_old_snapshot() {
        if !git_available() {
            return;
        }

        let mut fixture = GitFixture::new();
        let cache = tempfile::tempdir().unwrap();
        let paths = CachePaths::from_root_for_tests(cache.path());
        let source = Source::parse(&format!("{}@main", fixture.remote_url)).unwrap();
        let first = checkout_source(&paths, &source).unwrap();

        let second_commit = fixture.push_change("v2\n");
        let second = checkout_source(&paths, &source).unwrap();
        let pinned = Source::parse(&format!("{}#{}", fixture.remote_url, first.commit)).unwrap();
        let old = checkout_source(&paths, &pinned).unwrap();

        assert_eq!(second.commit, second_commit);
        assert_ne!(second.commit, first.commit);
        assert_eq!(
            fs::read_to_string(second.dir.join("index.js")).unwrap(),
            "v2\n"
        );
        assert!(old.cache_hit);
        assert_eq!(
            fs::read_to_string(old.dir.join("index.js")).unwrap(),
            "v1\n"
        );
    }

    #[test]
    #[cfg(unix)]
    fn replaces_an_incomplete_or_corrupt_snapshot_instead_of_blessing_it() {
        if !git_available() {
            return;
        }

        let fixture = GitFixture::new();
        let cache = tempfile::tempdir().unwrap();
        let paths = CachePaths::from_root_for_tests(cache.path());
        let source =
            Source::parse(&format!("{}#{}", fixture.remote_url, fixture.first_commit)).unwrap();
        let first = checkout_source(&paths, &source).unwrap();
        let remote_dir = first.dir.parent().unwrap();

        fs::remove_file(snapshot_marker_path(remote_dir, &first.commit)).unwrap();
        fs::write(first.dir.join("index.js"), "tampered\n").unwrap();

        let repaired = checkout_source(&paths, &source).unwrap();

        assert!(!repaired.cache_hit);
        assert_eq!(
            fs::read_to_string(repaired.dir.join("index.js")).unwrap(),
            "v1\n"
        );
    }

    struct GitFixture {
        _temp: tempfile::TempDir,
        work: std::path::PathBuf,
        remote: std::path::PathBuf,
        remote_url: String,
        first_commit: String,
    }

    impl GitFixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let work = temp.path().join("work");
            let remote = temp.path().join("remote.git");

            git(
                temp.path(),
                &["init", "--quiet", "--initial-branch=main", path(&work)],
            );
            fs::write(
                work.join("gitx.toml"),
                "schema = 1\nname = \"fixture\"\nruntime = \"node\"\nentry = \"index.js\"\n",
            )
            .unwrap();
            fs::write(work.join("index.js"), "v1\n").unwrap();
            git(&work, &["add", "."]);
            git(
                &work,
                &[
                    "-c",
                    "user.name=GitX Tests",
                    "-c",
                    "user.email=gitx@example.invalid",
                    "commit",
                    "--quiet",
                    "-m",
                    "initial",
                ],
            );
            let first_commit = git_output(&work, &["rev-parse", "HEAD"]);
            git(
                temp.path(),
                &["clone", "--quiet", "--bare", path(&work), path(&remote)],
            );
            git(&work, &["remote", "add", "origin", path(&remote)]);

            Self {
                remote_url: format!("file://{}", remote.display()),
                _temp: temp,
                work,
                remote,
                first_commit,
            }
        }

        fn push_change(&mut self, contents: &str) -> String {
            fs::write(self.work.join("index.js"), contents).unwrap();
            git(&self.work, &["add", "index.js"]);
            git(
                &self.work,
                &[
                    "-c",
                    "user.name=GitX Tests",
                    "-c",
                    "user.email=gitx@example.invalid",
                    "commit",
                    "--quiet",
                    "-m",
                    "update",
                ],
            );
            git(&self.work, &["push", "--quiet", "origin", "main"]);
            let commit = git_output(&self.work, &["rev-parse", "HEAD"]);
            assert!(self.remote.exists());
            commit
        }
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    }

    fn path(path: &Path) -> &str {
        path.to_str().expect("temporary test paths are UTF-8")
    }

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_output(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }
}
