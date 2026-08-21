use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::error::{GitxError, Result};

pub const MANIFEST_FILE: &str = "gitx.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema: u32,
    pub name: String,
    pub description: Option<String>,
    pub runtime: String,
    pub entry: String,
    pub permissions: Permissions,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    pub network: bool,
    pub read: Vec<String>,
    pub write: Vec<String>,
    pub env: Vec<String>,
    pub subprocess: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    schema: Option<u32>,
    name: Option<String>,
    description: Option<String>,
    runtime: Option<String>,
    entry: Option<String>,
    permissions: Option<RawPermissions>,
    metadata: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawPermissions {
    network: Option<bool>,
    read: Option<Vec<String>>,
    write: Option<Vec<String>>,
    env: Option<Vec<String>>,
    subprocess: Option<bool>,
}

impl Manifest {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        if !path.exists() {
            return Err(GitxError::ManifestMissing {
                path: path.to_path_buf(),
            });
        }

        let contents = fs::read_to_string(path).map_err(|source| GitxError::ReadManifest {
            path: path.to_path_buf(),
            source,
        })?;

        let raw = toml::from_str::<RawManifest>(&contents).map_err(|source| {
            GitxError::ParseManifest {
                path: path.to_path_buf(),
                source,
            }
        })?;

        validate_manifest(raw, path)
    }
}

impl From<RawPermissions> for Permissions {
    fn from(raw: RawPermissions) -> Self {
        Self {
            network: raw.network.unwrap_or(false),
            read: raw.read.unwrap_or_default(),
            write: raw.write.unwrap_or_default(),
            env: raw.env.unwrap_or_default(),
            subprocess: raw.subprocess.unwrap_or(false),
        }
    }
}

fn validate_manifest(raw: RawManifest, path: &Path) -> Result<Manifest> {
    let schema = required(raw.schema, "schema", path)?;

    if schema != 1 {
        return Err(invalid_manifest(
            path,
            format!("unsupported schema `{schema}`; expected `1`"),
        ));
    }

    let manifest = Manifest {
        schema,
        name: required_text(raw.name, "name", path)?,
        description: raw.description,
        runtime: required_text(raw.runtime, "runtime", path)?,
        entry: required_text(raw.entry, "entry", path)?,
        permissions: raw.permissions.unwrap_or_default().into(),
        metadata: raw.metadata.unwrap_or_default(),
    };

    validate_manifest_text(&manifest, path)?;
    Ok(manifest)
}

fn required<T>(value: Option<T>, field: &str, path: &Path) -> Result<T> {
    value.ok_or_else(|| invalid_manifest(path, format!("missing required field `{field}`")))
}

fn required_text(value: Option<String>, field: &str, path: &Path) -> Result<String> {
    let value = required(value, field, path)?;
    if value.trim().is_empty() {
        return Err(invalid_manifest(
            path,
            format!("field `{field}` cannot be empty"),
        ));
    }
    Ok(value)
}

fn validate_manifest_text(manifest: &Manifest, path: &Path) -> Result<()> {
    for (field, value) in [
        ("name", manifest.name.as_str()),
        ("runtime", manifest.runtime.as_str()),
        ("entry", manifest.entry.as_str()),
    ] {
        validate_single_line(field, value, path)?;
    }

    if matches!(manifest.name.as_str(), "." | "..") {
        return Err(invalid_manifest(
            path,
            "field `name` cannot be `.` or `..`".to_string(),
        ));
    }

    for (field, values) in [
        ("permissions.read", &manifest.permissions.read),
        ("permissions.write", &manifest.permissions.write),
        ("permissions.env", &manifest.permissions.env),
    ] {
        for value in values {
            validate_single_line(field, value, path)?;
        }
    }

    Ok(())
}

fn validate_single_line(field: &str, value: &str, path: &Path) -> Result<()> {
    if value.chars().any(char::is_control) {
        return Err(invalid_manifest(
            path,
            format!("field `{field}` cannot contain control characters"),
        ));
    }
    Ok(())
}

/// Resolve an entrypoint to a file whose final canonical target remains inside
/// the tool root. This rejects both lexical traversal and symlink escapes.
pub fn resolve_entry(root: &Path, entry: &str) -> Result<PathBuf> {
    let relative = Path::new(entry);

    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(GitxError::Message(format!(
            "entrypoint `{entry}` must stay inside the tool directory"
        )));
    }

    let canonical_root = root.canonicalize().map_err(|source| GitxError::Io {
        context: format!("resolving tool directory {}", root.display()),
        source,
    })?;
    let joined = canonical_root.join(relative);
    let resolved = joined.canonicalize().map_err(|source| GitxError::Io {
        context: format!("resolving entrypoint {}", joined.display()),
        source,
    })?;

    if !resolved.starts_with(&canonical_root) {
        return Err(GitxError::Message(format!(
            "entrypoint `{entry}` resolves outside the tool directory"
        )));
    }

    if !resolved.is_file() {
        return Err(GitxError::Message(format!(
            "entrypoint `{entry}` is not a file in {}",
            canonical_root.display()
        )));
    }

    Ok(resolved)
}

fn invalid_manifest(path: &Path, message: String) -> GitxError {
    GitxError::InvalidManifest {
        path: path.to_path_buf(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{Manifest, Permissions, resolve_entry};

    #[test]
    fn parses_valid_manifest() {
        let manifest = Manifest::from_path("fixtures/valid-node-tool/gitx.toml").unwrap();

        assert_eq!(manifest.schema, 1);
        assert_eq!(manifest.name, "repo-audit");
        assert_eq!(manifest.runtime, "node");
        assert_eq!(manifest.entry, "src/main.js");
        assert_eq!(manifest.permissions.env, vec!["GITHUB_TOKEN"]);
    }

    #[test]
    fn applies_permission_defaults() {
        let manifest = Manifest::from_path("fixtures/minimal-tool/gitx.toml").unwrap();

        assert_eq!(manifest.permissions, Permissions::default());
    }

    #[test]
    fn rejects_missing_required_field() {
        let error = Manifest::from_path("fixtures/invalid-missing-entry/gitx.toml")
            .unwrap_err()
            .to_string();

        assert!(error.contains("missing required field `entry`"));
    }

    #[test]
    fn rejects_unknown_fields_and_empty_required_text() {
        let temp = tempfile::tempdir().unwrap();
        let unknown = temp.path().join("unknown.toml");
        fs::write(
            &unknown,
            "schema = 1\nname = \"demo\"\nruntime = \"node\"\nentry = \"index.js\"\nsubproces = true\n",
        )
        .unwrap();
        let empty = temp.path().join("empty.toml");
        fs::write(
            &empty,
            "schema = 1\nname = \"   \"\nruntime = \"node\"\nentry = \"index.js\"\n",
        )
        .unwrap();

        assert!(
            Manifest::from_path(unknown)
                .unwrap_err()
                .to_string()
                .contains("unknown field")
        );
        assert!(
            Manifest::from_path(empty)
                .unwrap_err()
                .to_string()
                .contains("field `name` cannot be empty")
        );
    }

    #[test]
    fn resolves_entrypoints_inside_the_tool_root() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("index.js");
        fs::write(&entry, "console.log('ok');\n").unwrap();

        assert_eq!(
            resolve_entry(temp.path(), "index.js").unwrap(),
            entry.canonicalize().unwrap()
        );
        assert!(resolve_entry(temp.path(), "../index.js").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_entrypoint_symlinks_that_escape_the_tool_root() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let tool = parent.path().join("tool");
        fs::create_dir(&tool).unwrap();
        let outside = parent.path().join("outside.js");
        fs::write(&outside, "console.log('outside');\n").unwrap();
        symlink(&outside, tool.join("index.js")).unwrap();

        let error = resolve_entry(&tool, "index.js").unwrap_err().to_string();

        assert!(error.contains("resolves outside the tool directory"));
    }
}
