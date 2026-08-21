use std::{
    path::Path,
    process::{Command, Stdio},
};

use crate::{
    cache::CachePaths,
    display::terminal_safe,
    error::{GitxError, Result},
    git::{self, GitCheckout},
    inspect,
    manifest::{MANIFEST_FILE, Manifest, resolve_entry},
    npm::{self, PackageBackend, PackageRunRequest},
    prompt, report,
    source::{Source, SourceKind},
    toolchain,
};

#[derive(Debug, Clone)]
pub struct RunRequest {
    pub source: String,
    pub args: Vec<String>,
    pub yes: bool,
    pub preview: bool,
}

pub fn run_source(request: RunRequest) -> Result<()> {
    let source = Source::parse(&request.source)?;

    match source.kind() {
        SourceKind::Npm => npm::run_package(PackageRunRequest {
            target: request.source.clone(),
            packages: Vec::new(),
            args: request.args,
            backend: PackageBackend::Auto,
            yes: request.yes,
            allow_scripts: false,
            allow_dangerous_scripts: false,
            preview: request.preview,
        }),
        SourceKind::Local => {
            let path = source.local_path().expect("local source has a path");
            if !path.is_dir() {
                return Err(GitxError::Message(format!(
                    "local source `{}` is not a directory",
                    path.display()
                )));
            }
            let dir = path.canonicalize().map_err(|io_error| GitxError::Io {
                context: format!("resolving local source {}", path.display()),
                source: io_error,
            })?;
            run_tool(&source, &dir, None, &request)
        }
        SourceKind::Github | SourceKind::Git => {
            let paths = CachePaths::discover();
            paths.ensure_base_dirs()?;
            let checkout = git::checkout_source(&paths, &source)?;
            let dir = checkout.dir.clone();
            run_tool(&source, &dir, Some(&checkout), &request)
        }
    }
}

fn run_tool(
    source: &Source,
    dir: &Path,
    checkout: Option<&GitCheckout>,
    request: &RunRequest,
) -> Result<()> {
    let manifest = Manifest::from_path(dir.join(MANIFEST_FILE))?;
    let entry_path = resolve_entry(dir, &manifest.entry)?;

    let mut warnings = Vec::new();
    if let Some(checkout) = checkout
        && let Some(name) = &checkout.requested_ref
    {
        warnings.push(format!(
            "Ref `{name}` is mutable; this run is pinned to commit {}.",
            checkout.commit
        ));
    }
    warnings.extend(inspect::transport_warnings(source));
    warnings.extend(inspect::manifest_warnings(&manifest));

    print_preflight(source, dir, checkout, &manifest, &warnings);
    if request.preview {
        return Ok(());
    }
    prompt::confirm_if_needed(&warnings, request.yes)?;
    report::step(format!(
        "Starting {} with {}",
        manifest.name, manifest.runtime
    ));

    let mut command = runtime_command(&manifest.runtime)?;
    command.arg(&entry_path).args(&request.args);
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = command.status().map_err(|io_error| GitxError::Io {
        context: format!("running entrypoint {}", entry_path.display()),
        source: io_error,
    })?;

    if status.success() {
        Ok(())
    } else {
        Err(GitxError::ToolExit {
            command: manifest.name.clone(),
            code: status.code().unwrap_or(1),
        })
    }
}

fn runtime_command(runtime: &str) -> Result<Command> {
    let candidates: &[&str] = match runtime {
        "node" => &["node"],
        "bun" => &["bun"],
        "deno" => &["deno"],
        "python" => &["python3", "python"],
        "shell" => &["sh"],
        other => {
            return Err(GitxError::Message(format!(
                "runtime `{other}` is not supported for execution yet"
            )));
        }
    };

    for candidate in candidates {
        if toolchain::exists_on_path(candidate) {
            return Ok(Command::new(candidate));
        }
    }

    Err(GitxError::MissingToolchain(candidates.join(", ")))
}

fn print_preflight(
    source: &Source,
    dir: &Path,
    checkout: Option<&GitCheckout>,
    manifest: &Manifest,
    warnings: &[String],
) {
    eprintln!("GitX run preflight");
    eprintln!("Source: {}", terminal_safe(source.raw()));

    if let Some(checkout) = checkout {
        eprintln!("Remote: {}", terminal_safe(&checkout.remote));
        eprintln!("Commit: {}", checkout.commit);
        eprintln!(
            "Snapshot: {} ({})",
            checkout.dir.display(),
            if checkout.cache_hit {
                "cached"
            } else {
                "fetched"
            }
        );
    } else {
        eprintln!("Directory: {}", dir.display());
    }

    eprintln!("Tool: {}", manifest.name);
    eprintln!("Runtime: {}", manifest.runtime);
    eprintln!("Entrypoint: {}", manifest.entry);
    eprintln!("Declared permissions (not enforced):");
    eprintln!("  network: {}", manifest.permissions.network);
    eprintln!("  read: {}", display_list(&manifest.permissions.read));
    eprintln!("  write: {}", display_list(&manifest.permissions.write));
    eprintln!("  env: {}", display_list(&manifest.permissions.env));
    eprintln!("  subprocess: {}", manifest.permissions.subprocess);
    eprintln!("Risk: {}", inspect::risk_level(warnings));
    eprintln!(
        "Policy: {}",
        if warnings.is_empty() { "allow" } else { "ask" }
    );

    if warnings.is_empty() {
        eprintln!("Warnings: none");
    } else {
        eprintln!("Warnings:");
        for warning in warnings {
            eprintln!("  - {}", terminal_safe(warning));
        }
    }
}

fn display_list(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}
