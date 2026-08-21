use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{
    catalog::{CatalogAction, CatalogCandidate, Compatibility},
    catalog_local::run_command,
    display::shell_quote,
    error::{GitxError, Result},
};

const CATALOG_FILE_LIMIT: u64 = 16 * 1024 * 1024;

/// Query explicitly requested per-user catalog definitions. A file catalog is
/// safe for device-only searches because GitX reads a local snapshot itself.
/// A command catalog is deliberately excluded from `--offline` and `--device`:
/// GitX cannot prove what an arbitrary child process will contact.
pub fn search(
    requested_ids: &[String],
    query: &str,
    limit: usize,
    local_only: bool,
) -> Result<Vec<CatalogCandidate>> {
    let path = config_path();
    let config = read_config(&path)?;
    let selected = select_catalogs(&config.catalogs, requested_ids, &path)?;
    let mut candidates = Vec::new();

    for catalog in selected {
        let remaining = limit.saturating_sub(candidates.len());
        if remaining == 0 {
            break;
        }
        if local_only && catalog.kind == CatalogKind::Command {
            return Err(GitxError::Message(format!(
                "configured catalog `{}` runs a command and cannot be used with --offline or --device; use a kind = \"file\" snapshot for a strict on-device search",
                catalog.id
            )));
        }
        let data = match catalog.kind {
            CatalogKind::File => read_catalog_file(catalog, &path)?,
            CatalogKind::Command => run_catalog_command(catalog, query)?,
        };
        candidates.extend(entries_to_candidates(
            catalog, &path, data, query, remaining,
        )?);
    }

    Ok(candidates)
}

pub fn config_path() -> PathBuf {
    if let Ok(path) = env::var("GITX_CATALOG_CONFIG") {
        return PathBuf::from(path);
    }
    if let Ok(config_home) = env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(config_home)
            .join("gitx")
            .join("catalogs.toml");
    }
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("gitx")
        .join("catalogs.toml")
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogConfig {
    version: u32,
    #[serde(default, rename = "catalog")]
    catalogs: Vec<CatalogDefinition>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogDefinition {
    id: String,
    #[serde(default)]
    label: Option<String>,
    kind: CatalogKind,
    #[serde(default)]
    path: Option<PathBuf>,
    #[serde(default)]
    command: Option<Vec<String>>,
    #[serde(default)]
    format: CatalogFormat,
    #[serde(default)]
    origin: Option<String>,
    action: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CatalogKind {
    File,
    Command,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CatalogFormat {
    #[default]
    Json,
    Jsonl,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogEntry {
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    origin: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    information: BTreeMap<String, String>,
}

fn read_config(path: &Path) -> Result<CatalogConfig> {
    let contents = fs::read_to_string(path).map_err(|source| GitxError::Io {
        context: format!(
            "reading user catalog configuration {}; set GITX_CATALOG_CONFIG to choose another file",
            path.display()
        ),
        source,
    })?;
    let config = toml::from_str::<CatalogConfig>(&contents).map_err(|source| {
        GitxError::ParseCatalogConfig {
            path: path.to_path_buf(),
            source,
        }
    })?;
    if config.version != 1 {
        return Err(GitxError::Message(format!(
            "catalog configuration {} has version {}; GitX supports version 1",
            path.display(),
            config.version
        )));
    }
    validate_catalogs(&config.catalogs, path)?;
    Ok(config)
}

fn validate_catalogs(catalogs: &[CatalogDefinition], config_path: &Path) -> Result<()> {
    let mut ids = HashSet::new();
    for catalog in catalogs {
        if catalog.id == "all" || !valid_identifier(&catalog.id) {
            return Err(GitxError::Message(format!(
                "configured catalog id `{}` in {} must use letters, numbers, hyphens, or underscores and cannot be `all`",
                catalog.id,
                config_path.display()
            )));
        }
        if !ids.insert(catalog.id.as_str()) {
            return Err(GitxError::Message(format!(
                "configured catalog id `{}` is defined more than once in {}",
                catalog.id,
                config_path.display()
            )));
        }
        if catalog.action.is_empty() || catalog.action.iter().any(|arg| invalid_value(arg)) {
            return Err(GitxError::Message(format!(
                "configured catalog `{}` needs a non-empty action array without control characters",
                catalog.id
            )));
        }
        if catalog.label.as_deref().is_some_and(invalid_value)
            || catalog.origin.as_deref().is_some_and(invalid_value)
        {
            return Err(GitxError::Message(format!(
                "configured catalog `{}` has an invalid label or origin",
                catalog.id
            )));
        }
        match catalog.kind {
            CatalogKind::File if catalog.path.is_none() || catalog.command.is_some() => {
                return Err(GitxError::Message(format!(
                    "configured file catalog `{}` needs path and cannot set command",
                    catalog.id
                )));
            }
            CatalogKind::Command
                if catalog.command.as_ref().is_none_or(Vec::is_empty) || catalog.path.is_some() =>
            {
                return Err(GitxError::Message(format!(
                    "configured command catalog `{}` needs a non-empty command array and cannot set path",
                    catalog.id
                )));
            }
            _ => {}
        }
        if catalog
            .command
            .as_ref()
            .is_some_and(|command| command.iter().any(|arg| invalid_value(arg)))
        {
            return Err(GitxError::Message(format!(
                "configured command catalog `{}` has an argument containing a control character",
                catalog.id
            )));
        }
    }
    Ok(())
}

fn valid_identifier(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphanumeric())
        && chars
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn invalid_value(value: &str) -> bool {
    value.trim().is_empty() || value.chars().any(char::is_control)
}

fn select_catalogs<'a>(
    catalogs: &'a [CatalogDefinition],
    requested_ids: &[String],
    config_path: &Path,
) -> Result<Vec<&'a CatalogDefinition>> {
    let requested_ids = deduplicate_ids(requested_ids)?;
    let all = requested_ids.iter().any(|id| id == "all");
    if all && requested_ids.len() > 1 {
        return Err(GitxError::Message(
            "--catalog all cannot be combined with another catalog id".to_string(),
        ));
    }
    if all {
        return Ok(catalogs.iter().collect());
    }

    requested_ids
        .iter()
        .map(|id| {
            catalogs
                .iter()
                .find(|catalog| catalog.id == *id)
                .ok_or_else(|| {
                    GitxError::Message(format!(
                        "configured catalog `{id}` was not found; check {}",
                        config_path.display()
                    ))
                })
        })
        .collect()
}

fn deduplicate_ids(ids: &[String]) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for id in ids {
        let id = id.trim();
        if !valid_identifier(id) && id != "all" {
            return Err(GitxError::Message(format!(
                "catalog id `{id}` must use letters, numbers, hyphens, or underscores"
            )));
        }
        if seen.insert(id.to_string()) {
            result.push(id.to_string());
        }
    }
    Ok(result)
}

fn read_catalog_file(catalog: &CatalogDefinition, config_path: &Path) -> Result<String> {
    let path = resolve_config_relative_path(
        catalog.path.as_ref().expect("validated file catalog path"),
        config_path,
    );
    let metadata = fs::metadata(&path).map_err(|source| GitxError::Io {
        context: format!("inspecting configured catalog file {}", path.display()),
        source,
    })?;
    if !metadata.is_file() {
        return Err(GitxError::Message(format!(
            "configured catalog file {} is not a regular file",
            path.display()
        )));
    }
    if metadata.len() > CATALOG_FILE_LIMIT {
        return Err(GitxError::Message(format!(
            "configured catalog file {} is larger than the {} MiB catalog limit",
            path.display(),
            CATALOG_FILE_LIMIT / (1024 * 1024)
        )));
    }
    fs::read_to_string(&path).map_err(|source| GitxError::Io {
        context: format!("reading configured catalog file {}", path.display()),
        source,
    })
}

fn run_catalog_command(catalog: &CatalogDefinition, query: &str) -> Result<String> {
    let command = catalog.command.as_ref().expect("validated command catalog");
    let expanded = command
        .iter()
        .map(|argument| expand_template(argument, query, "", ""))
        .collect::<Result<Vec<_>>>()?;
    let (program, args) = expanded
        .split_first()
        .expect("validated command catalog contains a program");
    run_command(
        program,
        args,
        &format!("reading configured catalog `{}`", catalog.id),
    )
}

fn resolve_config_relative_path(path: &Path, config_path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(path)
}

fn entries_to_candidates(
    catalog: &CatalogDefinition,
    config_path: &Path,
    data: String,
    query: &str,
    limit: usize,
) -> Result<Vec<CatalogCandidate>> {
    let entries = parse_entries(&data, catalog.format, &catalog.id)?;
    entries
        .into_iter()
        .filter(|entry| entry_matches(entry, query))
        .take(limit)
        .map(|entry| entry_to_candidate(catalog, config_path, entry, query))
        .collect()
}

fn parse_entries(data: &str, format: CatalogFormat, id: &str) -> Result<Vec<CatalogEntry>> {
    match format {
        CatalogFormat::Json => serde_json::from_str(data).map_err(|source| GitxError::Json {
            context: format!("decoding JSON from configured catalog `{id}`"),
            source,
        }),
        CatalogFormat::Jsonl => data
            .lines()
            .filter(|line| !line.trim().is_empty())
            .enumerate()
            .map(|(index, line)| {
                serde_json::from_str(line).map_err(|source| GitxError::Json {
                    context: format!(
                        "decoding JSON line {} from configured catalog `{id}`",
                        index + 1
                    ),
                    source,
                })
            })
            .collect(),
    }
}

fn entry_matches(entry: &CatalogEntry, query: &str) -> bool {
    let query = query.to_ascii_lowercase();
    [
        entry.name.as_str(),
        entry.description.as_deref().unwrap_or_default(),
        entry.origin.as_deref().unwrap_or_default(),
    ]
    .into_iter()
    .any(|value| value.to_ascii_lowercase().contains(&query))
}

fn entry_to_candidate(
    catalog: &CatalogDefinition,
    config_path: &Path,
    entry: CatalogEntry,
    query: &str,
) -> Result<CatalogCandidate> {
    if invalid_value(&entry.name)
        || entry.version.as_deref().is_some_and(invalid_value)
        || entry.description.as_deref().is_some_and(invalid_value)
        || entry.homepage.as_deref().is_some_and(invalid_value)
        || entry.origin.as_deref().is_some_and(invalid_value)
        || entry.updated_at.as_deref().is_some_and(invalid_value)
        || entry
            .information
            .iter()
            .any(|(key, value)| invalid_value(key) || invalid_value(value))
    {
        return Err(GitxError::Message(format!(
            "configured catalog `{}` has a record with an empty or control-character field",
            catalog.id
        )));
    }
    let mut information = entry.information;
    information.insert("Catalog id".to_string(), catalog.id.clone());
    information.insert(
        "Catalog config".to_string(),
        config_path.display().to_string(),
    );
    information.insert(
        "Catalog kind".to_string(),
        match catalog.kind {
            CatalogKind::File => "file snapshot".to_string(),
            CatalogKind::Command => "command output".to_string(),
        },
    );
    let version = entry.version.unwrap_or_default();
    let action = catalog
        .action
        .iter()
        .map(|argument| expand_template(argument, query, &entry.name, &version))
        .collect::<Result<Vec<_>>>()?;
    let action = action
        .into_iter()
        .map(|argument| shell_quote(&argument))
        .collect::<Vec<_>>()
        .join(" ");
    let source = catalog.label.as_deref().unwrap_or(&catalog.id).to_string();
    let origin = entry
        .origin
        .or_else(|| catalog.origin.clone())
        .unwrap_or_else(|| format!("configured catalog definition in {}", config_path.display()));

    Ok(CatalogCandidate {
        source: format!("configured catalog {source}"),
        origin,
        name: entry.name,
        version: (!version.is_empty()).then_some(version),
        description: entry.description,
        homepage: entry.homepage,
        updated_at: entry.updated_at,
        compatibility: Compatibility::unknown(
            "user-configured catalog; inspect the result before assuming it can run on this host",
        ),
        information,
        action: CatalogAction::new("Use", action),
    })
}

fn expand_template(template: &str, query: &str, name: &str, version: &str) -> Result<String> {
    if template.contains("{version}") && version.is_empty() {
        return Err(GitxError::Message(
            "a configured catalog action uses {version}, but the matching record has no version"
                .to_string(),
        ));
    }
    let value = template
        .replace("{query}", query)
        .replace("{name}", name)
        .replace("{version}", version);
    if invalid_value(&value) {
        return Err(GitxError::Message(
            "a configured catalog template produced an empty or control-character argument"
                .to_string(),
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        CatalogDefinition, CatalogFormat, CatalogKind, entries_to_candidates, parse_entries,
        resolve_config_relative_path, select_catalogs,
    };

    fn catalog(id: &str) -> CatalogDefinition {
        CatalogDefinition {
            id: id.to_string(),
            label: None,
            kind: CatalogKind::File,
            path: Some("catalog.json".into()),
            command: None,
            format: CatalogFormat::Json,
            origin: Some("fixture catalog".to_string()),
            action: vec![
                "toolctl".to_string(),
                "install".to_string(),
                "{name}".to_string(),
            ],
        }
    }

    #[test]
    fn file_catalog_records_origin_and_renders_a_quoted_action() {
        let definition = catalog("company-tools");
        let results = entries_to_candidates(
            &definition,
            Path::new("/tmp/catalogs.toml"),
            r#"[{"name":"tool with spaces","version":"1.2.3","description":"internal tool","origin":"snapshot revision abc"}]"#.to_string(),
            "internal",
            10,
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].origin, "snapshot revision abc");
        assert_eq!(
            results[0].action.command,
            "toolctl install 'tool with spaces'"
        );
    }

    #[test]
    fn catalog_selection_rejects_ambiguous_all() {
        let error = select_catalogs(
            &[catalog("one")],
            &["all".to_string(), "one".to_string()],
            Path::new("catalogs.toml"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("cannot be combined"));
    }

    #[test]
    fn parses_jsonl_catalog_records() {
        let records = parse_entries(
            "{\"name\":\"one\"}\n{\"name\":\"two\"}\n",
            CatalogFormat::Jsonl,
            "fixture",
        )
        .unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn relative_catalog_paths_follow_the_configuration_file() {
        assert_eq!(
            resolve_config_relative_path(
                Path::new("records.json"),
                Path::new("/tmp/gitx/catalogs.toml")
            ),
            Path::new("/tmp/gitx/records.json")
        );
    }
}
