use std::{io, path::PathBuf};

use clap::{ArgAction, Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};

use crate::{
    cache,
    display::{self, ColorMode},
    error::{GitxError, Result},
    inspect, install,
    npm::{self, PackageBackend, PackageRunRequest},
    report::{self, ProgressMode},
    run::{self, RunRequest},
    search::{self, SearchFormat, SearchRequest, SearchSource},
    toolchain,
};

#[derive(Debug, Parser)]
#[command(name = "gitx")]
#[command(version)]
#[command(
    about = "Run and inspect tools from Git repos and package sources with preflight risk visibility."
)]
#[command(
    after_help = "Start here:\n  gitx inspect ./my-tool\n  gitx run github:owner/repo@v1.0.0 -- --tool-flag\n  gitx exec prettier@3.6.2 -- --check README.md\n  gitx search formatter --source npm,github --details 1\n  gitx search formatter --device\n\nSources: local directories (`./tool`), GitX and platform-local catalogs, GitHub/GitLab/Codeberg repos, generic Git URLs, npm (`npm:package@version`), configured Bun registries, and the AUR catalog on Arch Linux.\nRun `gitx <command> --help` for command-specific examples and options."
)]
pub struct Cli {
    /// Control ANSI colour in human-readable terminal output.
    #[arg(long, global = true, value_enum, default_value_t = ColorModeArg::Auto)]
    color: ColorModeArg,

    /// Print resolution, safety-check, and backend hand-off steps to standard error. Repeat for source-level detail.
    #[arg(short = 'v', long, global = true, action = ArgAction::Count)]
    verbose: u8,

    /// Render transfer and Git activity on standard error: auto, always, or never.
    #[arg(long, global = true, value_enum, default_value_t = ProgressModeArg::Auto)]
    progress: ProgressModeArg,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ColorModeArg {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ProgressModeArg {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Inspect a source without running it.
    #[command(
        after_help = "Examples:\n  gitx inspect ./my-tool\n  gitx inspect github:owner/repo@v1.0.0\n  gitx inspect npm:prettier@3.6.2 --format json"
    )]
    Inspect(InspectCommand),

    /// Run a repo-defined command.
    #[command(
        after_help = "Examples:\n  gitx run ./my-tool -- --fix\n  gitx run github:owner/repo@v1.0.0 -- --check\n  gitx run ./my-tool --preview"
    )]
    Run(RunCommand),

    /// Run a package binary with GitX preflight checks.
    #[command(
        after_help = "Examples:\n  gitx exec prettier@3.6.2 -- --check README.md\n  gitx exec --package @angular/cli@20.0.0 --bin ng -- new app\n  gitx exec vite --preview"
    )]
    Exec(ExecCommand),

    /// Search local tools, registries, Git hosts, and platform catalogs.
    #[command(
        after_help = "Examples:\n  gitx search formatter --interactive\n  gitx search formatter --source cache --select 1\n  gitx search formatter --source bun,github --details 1\n  gitx search formatter --source device --details 1\n  gitx search formatter --device\n  gitx search formatter --catalog company-tools\n  gitx search formatter --source gitlab,codeberg --format json\n  gitx search formatter --offline --source installed,cache\n\nDefault sources include device state, npm, the configured Bun registry, GitHub, GitLab, Codeberg, and AUR on Arch Linux. `--source` may be repeated or comma-separated. `--source device` expands to strict on-device catalogs; `--device` is the strict local-only shortcut. `--catalog` runs an explicitly selected user catalog; `--device`, `--offline`, and `--source device` accept only file-backed catalog snapshots."
    )]
    Search(SearchCommand),

    /// Install a source as a persistent GitX-managed shim.
    #[command(
        after_help = "Examples:\n  gitx install prettier@3.6.2\n  gitx install github:owner/repo@v1.0.0 --as repo-tool"
    )]
    Install(InstallCommand),

    /// Re-resolve an installed tool and show its exact update diff.
    #[command(
        after_help = "Examples:\n  gitx update prettier\n  gitx update repo-tool --check\n\n`--check` resolves and compares without writing the install record or shim."
    )]
    Update(UpdateCommand),

    /// List persistent GitX-managed shims.
    List,

    /// Remove a persistent GitX-managed shim.
    Remove(RemoveCommand),

    /// Manage the GitX local cache.
    Cache(CacheCommand),

    /// Check the local runtimes GitX can use.
    Doctor,

    /// Generate a shell completion script to standard output.
    #[command(
        after_help = "Examples:\n  gitx completion bash > ~/.local/share/bash-completion/completions/gitx\n  gitx completion zsh > ~/.zfunc/_gitx"
    )]
    Completion(CompletionCommand),
}

#[derive(Debug, Args)]
struct InspectCommand {
    /// Source to inspect, such as github:owner/repo@main or ./local-tool.
    source: String,

    /// Output format for people (`text`) or scripts (`json`).
    #[arg(long, value_enum, default_value_t = InspectFormat::Text)]
    format: InspectFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum InspectFormat {
    Text,
    Json,
}

#[derive(Debug, Args)]
struct RunCommand {
    /// Source to run.
    source: String,

    /// Run without an interactive confirmation prompt.
    #[arg(short = 'y', long)]
    yes: bool,

    /// Resolve and show the preflight card without executing.
    #[arg(short = 'n', long)]
    preview: bool,

    /// Arguments forwarded to the tool after `--`.
    #[arg(last = true)]
    args: Vec<String>,
}

#[derive(Debug, Args)]
struct SearchCommand {
    /// Search query.
    query: String,

    /// Mark the strongest matching result as a suggestion; it never executes a tool.
    #[arg(long)]
    recommend: bool,

    /// Maximum number of results to show (1-20).
    #[arg(short = 'n', long, default_value_t = 10, value_parser = parse_search_limit)]
    limit: usize,

    /// Sources to search; repeat this option or separate sources with commas.
    #[arg(
        long = "source",
        value_enum,
        value_delimiter = ',',
        value_name = "SOURCE"
    )]
    sources: Vec<SearchSourceArg>,

    /// Project directory whose node_modules/.bin directory should be searched.
    #[arg(long, default_value = ".", value_name = "DIR")]
    project: PathBuf,

    /// Select one or more displayed results and print their next commands.
    #[arg(
        long,
        value_name = "SELECTION",
        conflicts_with_all = ["interactive", "details"]
    )]
    select: Option<String>,

    /// Prompt to select displayed results; never executes a selected tool.
    #[arg(long, conflicts_with_all = ["select", "details"])]
    interactive: bool,

    /// Show full metadata for one numbered result; JSON output already includes every field.
    #[arg(
        long,
        value_name = "RESULT",
        conflicts_with_all = ["select", "interactive"]
    )]
    details: Option<String>,

    /// Do not contact registry, Git-host, or AUR providers.
    #[arg(long)]
    offline: bool,

    /// Use only on-device GitX state and available local package databases or caches.
    #[arg(long, visible_alias = "device-only", conflicts_with = "sources")]
    device: bool,

    /// User catalog id from ~/.config/gitx/catalogs.toml; repeat or comma-separate ids.
    #[arg(long = "catalog", value_delimiter = ',', value_name = "ID")]
    catalogs: Vec<String>,

    /// Output format for people (`text`) or scripts (`json`).
    #[arg(long, value_enum, default_value_t = SearchFormatArg::Text)]
    format: SearchFormatArg,
}

fn parse_search_limit(input: &str) -> std::result::Result<usize, String> {
    let limit = input
        .parse::<usize>()
        .map_err(|_| "limit must be a whole number from 1 to 20".to_string())?;

    if (1..=20).contains(&limit) {
        Ok(limit)
    } else {
        Err("limit must be a whole number from 1 to 20".to_string())
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SearchSourceArg {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SearchFormatArg {
    Text,
    Json,
}

#[derive(Debug, Args)]
struct ExecCommand {
    /// Package to run, or a binary name when --package is supplied.
    target: Option<String>,

    /// Repeatable package to make available when the binary differs from the package name.
    #[arg(short = 'p', long = "package", value_name = "PACKAGE")]
    packages: Vec<String>,

    /// Explicit binary name; use with one or more --package options.
    #[arg(long, value_name = "BIN", conflicts_with = "target")]
    bin: Option<String>,

    /// Package manager backend used after GitX preflight checks.
    #[arg(long, default_value = "auto")]
    backend: Backend,

    /// Arguments forwarded to the package binary after `--`.
    #[arg(last = true)]
    args: Vec<String>,

    /// Run without an interactive confirmation prompt.
    #[arg(short = 'y', long)]
    yes: bool,

    /// Allow install lifecycle scripts in the delegated backend.
    #[arg(long)]
    allow_scripts: bool,

    /// Override deny-level findings in install lifecycle scripts.
    #[arg(long, requires = "allow_scripts")]
    allow_dangerous_scripts: bool,

    /// Resolve and show the preflight card without executing.
    #[arg(short = 'n', long)]
    preview: bool,
}

#[derive(Debug, Args)]
struct InstallCommand {
    /// Source to install.
    source: String,

    /// Name of the generated shim; defaults to the tool or binary name.
    #[arg(long = "as", value_name = "NAME")]
    name: Option<String>,
}

#[derive(Debug, Args)]
struct UpdateCommand {
    /// Name of the installed tool to update.
    tool: String,

    /// Resolve and show the update diff without changing the installed tool.
    #[arg(long)]
    check: bool,
}

#[derive(Debug, Args)]
struct RemoveCommand {
    /// Installed tool name to remove.
    tool: String,
}

#[derive(Debug, Args)]
struct CompletionCommand {
    /// Shell to generate completions for.
    #[arg(value_enum)]
    shell: Shell,
}

#[derive(Debug, Args)]
struct CacheCommand {
    #[command(subcommand)]
    command: CacheSubcommand,
}

#[derive(Debug, Subcommand)]
enum CacheSubcommand {
    /// List cached artifacts.
    List,

    /// Clear the local cache.
    Clear,

    /// Remove one cached artifact.
    Remove {
        /// Commit, package, or tool cache key to remove.
        key: String,
    },

    /// Import a verified package artifact from existing local package-manager caches.
    Import {
        /// Package artifact to import, such as cowsay@1.6.0.
        package: String,

        /// Existing package-manager cache to read from.
        #[arg(long, default_value = "auto")]
        from: CacheImportSource,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Backend {
    Auto,
    Bun,
    Npm,
    Pnpm,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CacheImportSource {
    Auto,
    All,
    Npm,
    Pnpm,
    Bun,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    display::set_color_mode(cli.color.into());
    report::set_verbosity(cli.verbose);
    report::set_progress_mode(cli.progress.into());
    let output = execute(cli)?;

    if let Some(output) = output {
        println!("{output}");
    }

    Ok(())
}

impl From<ProgressModeArg> for ProgressMode {
    fn from(value: ProgressModeArg) -> Self {
        match value {
            ProgressModeArg::Auto => Self::Auto,
            ProgressModeArg::Always => Self::Always,
            ProgressModeArg::Never => Self::Never,
        }
    }
}

impl From<ColorModeArg> for ColorMode {
    fn from(value: ColorModeArg) -> Self {
        match value {
            ColorModeArg::Auto => Self::Auto,
            ColorModeArg::Always => Self::Always,
            ColorModeArg::Never => Self::Never,
        }
    }
}

fn execute(cli: Cli) -> Result<Option<String>> {
    let Some(command) = cli.command else {
        return Ok(Some(Cli::command().render_help().to_string()));
    };

    match command {
        Command::Inspect(command) => {
            report::step(format!("Inspecting {}", command.source));
            inspect::inspect_source(&command.source, command.format == InspectFormat::Json)
                .map(Some)
        }
        Command::Run(command) => {
            report::step(format!("Preparing {} for execution", command.source));
            run::run_source(RunRequest {
                source: command.source,
                args: command.args,
                yes: command.yes,
                preview: command.preview,
            })?;
            Ok(None)
        }
        Command::Exec(command) => {
            let ExecCommand {
                target,
                packages,
                bin,
                args,
                backend,
                yes,
                allow_scripts,
                allow_dangerous_scripts,
                preview,
            } = command;
            if bin.is_some() && packages.is_empty() {
                return Err(GitxError::Message(
                    "--bin requires at least one --package option".to_string(),
                ));
            }
            let target = target.or(bin).ok_or_else(|| {
                GitxError::Message(
                    "gitx exec requires a package target, or --bin together with --package"
                        .to_string(),
                )
            })?;
            report::step(format!("Preparing package command {target}"));
            npm::run_package(PackageRunRequest {
                target,
                packages,
                args,
                backend: backend.into(),
                yes,
                allow_scripts,
                allow_dangerous_scripts,
                preview,
            })?;
            Ok(None)
        }
        Command::Search(command) => {
            report::step(format!("Searching for {}", command.query));
            search::search(SearchRequest {
                query: command.query,
                sources: command.sources.into_iter().map(Into::into).collect(),
                project: command.project,
                limit: command.limit,
                recommend: command.recommend,
                select: command.select,
                interactive: command.interactive,
                details: command.details,
                offline: command.offline,
                device_only: command.device,
                catalogs: command.catalogs,
                format: command.format.into(),
            })
            .map(Some)
        }
        Command::Install(command) => {
            report::step(format!(
                "Preparing {} for managed installation",
                command.source
            ));
            install::install(&command.source, command.name.as_deref()).map(Some)
        }
        Command::Update(command) => {
            report::step(format!("Resolving update for {}", command.tool));
            install::update(&command.tool, command.check).map(Some)
        }
        Command::List => {
            report::step("Reading managed installs");
            install::list().map(Some)
        }
        Command::Remove(command) => {
            report::step(format!("Removing managed install {}", command.tool));
            install::remove(&command.tool).map(Some)
        }
        Command::Cache(command) => {
            report::step("Reading GitX cache");
            run_cache(command).map(Some)
        }
        Command::Doctor => {
            report::step("Checking available runtimes");
            Ok(Some(toolchain::render_doctor()))
        }
        Command::Completion(command) => {
            let mut cli = Cli::command();
            generate(command.shell, &mut cli, "gitx", &mut io::stdout());
            Ok(None)
        }
    }
}

fn run_cache(command: CacheCommand) -> Result<String> {
    match command.command {
        CacheSubcommand::List => cache::list_cache(),
        CacheSubcommand::Clear => cache::clear_cache(),
        CacheSubcommand::Remove { key } => cache::remove_cache_key(&key),
        CacheSubcommand::Import { package, from } => {
            let inspection = npm::inspect_package_spec(&package)?;
            let integrity = inspection.integrity.ok_or_else(|| {
                GitxError::Message(format!(
                    "package {}@{} does not expose an npm integrity hash",
                    inspection.package, inspection.version
                ))
            })?;

            cache::import_package_artifact(cache::PackageArtifactImportRequest {
                package: inspection.package,
                version: inspection.version,
                integrity,
                sources: from.sources(),
                stop_after_first_import: from.stop_after_first_import(),
            })
        }
    }
}

impl From<Backend> for PackageBackend {
    fn from(value: Backend) -> Self {
        match value {
            Backend::Auto => PackageBackend::Auto,
            Backend::Bun => PackageBackend::Bun,
            Backend::Npm => PackageBackend::Npm,
            Backend::Pnpm => PackageBackend::Pnpm,
        }
    }
}

impl From<SearchSourceArg> for SearchSource {
    fn from(value: SearchSourceArg) -> Self {
        match value {
            SearchSourceArg::All => SearchSource::All,
            SearchSourceArg::Device => SearchSource::Device,
            SearchSourceArg::Npm => SearchSource::Npm,
            SearchSourceArg::Bun => SearchSource::Bun,
            SearchSourceArg::Github => SearchSource::Github,
            SearchSourceArg::Gitlab => SearchSource::Gitlab,
            SearchSourceArg::Codeberg => SearchSource::Codeberg,
            SearchSourceArg::Aur => SearchSource::Aur,
            SearchSourceArg::Installed => SearchSource::Installed,
            SearchSourceArg::Cache => SearchSource::Cache,
            SearchSourceArg::Project => SearchSource::Project,
            SearchSourceArg::System => SearchSource::System,
            SearchSourceArg::Apt => SearchSource::Apt,
            SearchSourceArg::Pacman => SearchSource::Pacman,
            SearchSourceArg::Flatpak => SearchSource::Flatpak,
            SearchSourceArg::Snap => SearchSource::Snap,
            SearchSourceArg::Cargo => SearchSource::Cargo,
            SearchSourceArg::Python => SearchSource::Python,
            SearchSourceArg::NpmGlobal => SearchSource::NpmGlobal,
        }
    }
}

impl From<SearchFormatArg> for SearchFormat {
    fn from(value: SearchFormatArg) -> Self {
        match value {
            SearchFormatArg::Text => SearchFormat::Text,
            SearchFormatArg::Json => SearchFormat::Json,
        }
    }
}

impl CacheImportSource {
    fn sources(self) -> Vec<cache::ExternalCacheKind> {
        match self {
            Self::Auto => cache::ExternalCacheKind::fastest_first(),
            Self::All => cache::ExternalCacheKind::all(),
            Self::Npm => vec![cache::ExternalCacheKind::Npm],
            Self::Pnpm => vec![cache::ExternalCacheKind::Pnpm],
            Self::Bun => vec![cache::ExternalCacheKind::Bun],
        }
    }

    fn stop_after_first_import(self) -> bool {
        matches!(self, Self::Auto)
    }
}
