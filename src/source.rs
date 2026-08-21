use std::path::PathBuf;

use serde::Serialize;

use crate::error::{GitxError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    Local,
    Github,
    Git,
    Npm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Local {
        raw: String,
        path: PathBuf,
    },
    Github {
        raw: String,
        owner: String,
        repo: String,
        remote: String,
        reference: Option<RefSpec>,
    },
    Git {
        raw: String,
        remote: String,
        reference: Option<RefSpec>,
    },
    Npm {
        raw: String,
        package: String,
        version: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefSpec {
    Named(String),
    Commit(String),
}

impl Source {
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();

        if trimmed.is_empty() {
            return Err(GitxError::UnsupportedSource(input.to_string()));
        }

        if trimmed.chars().any(char::is_control) {
            return Err(GitxError::Message(
                "source cannot contain control characters".to_string(),
            ));
        }

        if trimmed.starts_with("github:") {
            return parse_github(trimmed);
        }

        if trimmed.starts_with("gitlab:") {
            return parse_hosted_git(trimmed, "gitlab", "https://gitlab.com");
        }

        if trimmed.starts_with("codeberg:") {
            return parse_hosted_git(trimmed, "codeberg", "https://codeberg.org");
        }

        if trimmed.starts_with("npm:") {
            return parse_npm(trimmed);
        }

        if looks_like_git_url(trimmed) {
            return parse_git_url(trimmed);
        }

        if looks_like_local_path(trimmed) {
            return Ok(Source::Local {
                raw: trimmed.to_string(),
                path: PathBuf::from(trimmed),
            });
        }

        Err(GitxError::UnsupportedSource(trimmed.to_string()))
    }

    pub fn raw(&self) -> &str {
        match self {
            Source::Local { raw, .. }
            | Source::Github { raw, .. }
            | Source::Git { raw, .. }
            | Source::Npm { raw, .. } => raw,
        }
    }

    pub fn kind(&self) -> SourceKind {
        match self {
            Source::Local { .. } => SourceKind::Local,
            Source::Github { .. } => SourceKind::Github,
            Source::Git { .. } => SourceKind::Git,
            Source::Npm { .. } => SourceKind::Npm,
        }
    }

    pub fn resolved_remote(&self) -> Option<&str> {
        match self {
            Source::Github { remote, .. } | Source::Git { remote, .. } => Some(remote),
            Source::Local { .. } | Source::Npm { .. } => None,
        }
    }

    pub fn requested_ref(&self) -> Option<&str> {
        self.reference().and_then(|reference| match reference {
            RefSpec::Named(name) => Some(name.as_str()),
            RefSpec::Commit(_) => None,
        })
    }

    pub fn resolved_commit(&self) -> Option<&str> {
        self.reference().and_then(|reference| match reference {
            RefSpec::Commit(commit) => Some(commit.as_str()),
            RefSpec::Named(_) => None,
        })
    }

    pub fn reference(&self) -> Option<&RefSpec> {
        match self {
            Source::Github { reference, .. } | Source::Git { reference, .. } => reference.as_ref(),
            Source::Local { .. } | Source::Npm { .. } => None,
        }
    }

    pub fn local_path(&self) -> Option<&PathBuf> {
        match self {
            Source::Local { path, .. } => Some(path),
            Source::Github { .. } | Source::Git { .. } | Source::Npm { .. } => None,
        }
    }

    pub fn npm_package(&self) -> Option<&str> {
        match self {
            Source::Npm { package, .. } => Some(package),
            Source::Local { .. } | Source::Github { .. } | Source::Git { .. } => None,
        }
    }

    pub fn npm_version(&self) -> Option<&str> {
        match self {
            Source::Npm { version, .. } => version.as_deref(),
            Source::Local { .. } | Source::Github { .. } | Source::Git { .. } => None,
        }
    }
}

fn parse_github(input: &str) -> Result<Source> {
    let body = input
        .strip_prefix("github:")
        .expect("caller checked github prefix");
    let (repo_part, reference) = split_github_reference(body);
    let mut pieces = repo_part.split('/');
    let Some(owner) = pieces.next().filter(|part| !part.is_empty()) else {
        return Err(GitxError::InvalidGithubSource {
            input: input.to_string(),
        });
    };
    let Some(repo) = pieces.next().filter(|part| !part.is_empty()) else {
        return Err(GitxError::InvalidGithubSource {
            input: input.to_string(),
        });
    };

    if pieces.next().is_some() {
        return Err(GitxError::InvalidGithubSource {
            input: input.to_string(),
        });
    }

    let repo = repo.strip_suffix(".git").unwrap_or(repo).to_string();
    let remote = format!("https://github.com/{owner}/{repo}.git");

    Ok(Source::Github {
        raw: input.to_string(),
        owner: owner.to_string(),
        repo,
        remote,
        reference,
    })
}

/// GitLab supports nested groups, and a generic Git source already carries
/// the right execution semantics. Keep the convenient forge prefix as syntax
/// only, so no host-specific runtime branch is needed after parsing.
fn parse_hosted_git(input: &str, host: &str, web_base: &str) -> Result<Source> {
    let body = input.strip_prefix(&format!("{host}:")).ok_or_else(|| {
        GitxError::InvalidHostedGitSource {
            host: host.to_string(),
            input: input.to_string(),
        }
    })?;
    let (path, reference) = split_github_reference(body);
    let path = path.strip_suffix(".git").unwrap_or(path);
    let segments: Vec<_> = path.split('/').collect();
    if segments.len() < 2
        || segments
            .iter()
            .any(|segment| segment.is_empty() || matches!(*segment, "." | ".."))
    {
        return Err(GitxError::InvalidHostedGitSource {
            host: host.to_string(),
            input: input.to_string(),
        });
    }

    Ok(Source::Git {
        raw: input.to_string(),
        remote: format!("{}/{path}.git", web_base.trim_end_matches('/')),
        reference,
    })
}

fn parse_git_url(input: &str) -> Result<Source> {
    let (remote, reference) = split_git_url_reference(input);

    Ok(Source::Git {
        raw: input.to_string(),
        remote: remote.to_string(),
        reference,
    })
}

fn parse_npm(input: &str) -> Result<Source> {
    let body = input
        .strip_prefix("npm:")
        .expect("caller checked npm prefix");
    let Some((package, version)) = split_npm_package_version(body) else {
        return Err(GitxError::InvalidNpmSource {
            input: input.to_string(),
        });
    };

    Ok(Source::Npm {
        raw: input.to_string(),
        package: package.to_string(),
        version: version.map(str::to_string),
    })
}

fn split_github_reference(input: &str) -> (&str, Option<RefSpec>) {
    if let Some((repo, commit)) = input.rsplit_once('#')
        && !commit.is_empty()
    {
        return (repo, Some(RefSpec::Commit(commit.to_string())));
    }

    if let Some((repo, reference)) = input.rsplit_once('@')
        && !reference.is_empty()
    {
        return (repo, Some(RefSpec::Named(reference.to_string())));
    }

    (input, None)
}

fn split_git_url_reference(input: &str) -> (&str, Option<RefSpec>) {
    if let Some((remote, commit)) = input.rsplit_once('#')
        && !commit.is_empty()
    {
        return (remote, Some(RefSpec::Commit(commit.to_string())));
    }

    if let Some((remote, reference)) = input.rsplit_once('@')
        && !reference.is_empty()
        && remote.ends_with(".git")
    {
        return (remote, Some(RefSpec::Named(reference.to_string())));
    }

    (input, None)
}

fn split_npm_package_version(input: &str) -> Option<(&str, Option<&str>)> {
    if input.is_empty() {
        return None;
    }

    let version_separator = input.rfind('@').filter(|index| *index > 0);
    let (package, version) = match version_separator {
        Some(index) => {
            let (package, version_with_at) = input.split_at(index);
            let version = version_with_at.strip_prefix('@')?;
            if version.is_empty() {
                return None;
            }
            (package, Some(version))
        }
        None => (input, None),
    };

    if package.is_empty() {
        return None;
    }

    if package.starts_with('@') {
        let (_, name) = package.split_once('/')?;
        if name.is_empty() {
            return None;
        }
    }

    Some((package, version))
}

fn looks_like_git_url(input: &str) -> bool {
    input.starts_with("git@")
        || input.starts_with("ssh://")
        || input.starts_with("git://")
        || input.starts_with("file://")
        || input.ends_with(".git")
        || input.contains(".git@")
        || input.contains(".git#")
}

fn looks_like_local_path(input: &str) -> bool {
    input.starts_with("./")
        || input.starts_with("../")
        || input.starts_with('/')
        || input == "."
        || input == ".."
        || PathBuf::from(input).exists()
}

#[cfg(test)]
mod tests {
    use super::{RefSpec, Source};

    #[test]
    fn parses_github_ref() {
        let source = Source::parse("github:owner/repo@main").unwrap();

        match source {
            Source::Github {
                owner,
                repo,
                remote,
                reference,
                ..
            } => {
                assert_eq!(owner, "owner");
                assert_eq!(repo, "repo");
                assert_eq!(remote, "https://github.com/owner/repo.git");
                assert_eq!(reference, Some(RefSpec::Named("main".to_string())));
            }
            other => panic!("unexpected source: {other:?}"),
        }
    }

    #[test]
    fn parses_github_commit() {
        let source = Source::parse("github:owner/repo#abc123").unwrap();

        assert_eq!(source.resolved_commit(), Some("abc123"));
        assert_eq!(source.requested_ref(), None);
    }

    #[test]
    fn parses_ssh_git_ref_without_confusing_user_at_host() {
        let source = Source::parse("git@github.com:owner/repo.git@main").unwrap();

        match source {
            Source::Git {
                remote, reference, ..
            } => {
                assert_eq!(remote, "git@github.com:owner/repo.git");
                assert_eq!(reference, Some(RefSpec::Named("main".to_string())));
            }
            other => panic!("unexpected source: {other:?}"),
        }
    }

    #[test]
    fn parses_scoped_npm_package() {
        let source = Source::parse("npm:@eslint/js@9.30.0").unwrap();

        match source {
            Source::Npm {
                package, version, ..
            } => {
                assert_eq!(package, "@eslint/js");
                assert_eq!(version.as_deref(), Some("9.30.0"));
            }
            other => panic!("unexpected source: {other:?}"),
        }
    }

    #[test]
    fn parses_gitlab_subgroup_and_codeberg_shorthand_as_git_sources() {
        let gitlab = Source::parse("gitlab:group/subgroup/tool@main").unwrap();
        assert_eq!(
            gitlab.resolved_remote(),
            Some("https://gitlab.com/group/subgroup/tool.git")
        );
        assert_eq!(gitlab.requested_ref(), Some("main"));

        let codeberg = Source::parse("codeberg:owner/tool#abc1234").unwrap();
        assert_eq!(
            codeberg.resolved_remote(),
            Some("https://codeberg.org/owner/tool.git")
        );
        assert_eq!(codeberg.resolved_commit(), Some("abc1234"));
    }
}
