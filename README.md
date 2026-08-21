# GitX

GitX is a local-first CLI for inspecting and running tools from Git repositories with explicit source resolution and a preflight risk summary. It also puts npm-style package execution behind the same resolve-inspect-run workflow.

> **Status:** GitX is an early Rust prototype. Build it from source, expect interfaces to change, and review the [security boundary](#security-boundary) before running untrusted code.

## Why GitX?

Git makes source history, authorship, review, tags, and private-repository access visible. NPX makes one-off tools convenient. GitX combines those strengths: resolve a source to an immutable identity, show what will run, cache it, and then execute only after the preflight policy allows it.

The Git-native workflow is the product's core:

```bash
gitx inspect github:owner/repo@main
gitx run github:owner/repo@v1.0.0 -- --tool-flags
gitx run github:owner/repo#abc1234 -- --tool-flags
```

See [Product Scope](docs/product-scope.md) for the durable product contract.

## Install

GitX needs Git plus Rust 1.88 or newer with Cargo. Copy and run:

```bash
git clone https://github.com/semyonfox/gitx.git
cd gitx
cargo install --path . --locked
gitx --version
```

`cargo install` builds the release binary and installs it in
`$CARGO_HOME/bin`, usually `~/.cargo/bin`. Rustup normally adds that directory
to `PATH`. If `gitx --version` is not found, run
`export PATH="$HOME/.cargo/bin:$PATH"` and retry. Install Rust with
[rustup](https://rustup.rs/) if `cargo --version` fails.

## Quick start

Remote Git sources require `git`, and execution requires the runtime declared
by the tool. Package mode delegates materialization to an installed npm, Bun,
or pnpm backend.

From this repository:

```bash
cargo test
cargo run -- inspect ./fixtures/valid-node-tool
cargo run -- inspect ./fixtures/valid-node-tool --format json
cargo run -- run ./fixtures/minimal-tool --preview -- --example
```

Build the standalone binary with:

```bash
cargo build --release
./target/release/gitx --help
```

## GitHub tool workflow

GitX can inspect, run, and persistently install a repository that contains a
valid `gitx.toml` manifest:

```bash
gitx inspect github:owner/repo@main
gitx run github:owner/repo@v1.0.0 -- --tool-flag
gitx install github:owner/repo@main --as repo-tool
gitx update repo-tool --check
```

`install` resolves the ref to a commit and writes a user-level shim. `update
--check` shows the proposed commit and metadata changes without writing.

## Common workflows

| Goal | Command |
| --- | --- |
| Inspect without executing | `gitx inspect <source> [--format json]` |
| Preview a repository tool | `gitx run <source> [--yes] --preview -- <args>` |
| Run a repository tool | `gitx run <source> [--yes] -- <args>` |
| Search local tools, caches, registries, Git hosts, and platform catalogs | `gitx search <query> [--source <source>] [--details <result>]` |
| Preview a package binary | `gitx exec <package>[@version] --preview -- <args>` |
| Run a package binary | `gitx exec <package>[@version] [--yes] -- <args>` |
| Run a named binary from a package | `gitx exec --package <package>[@version] --bin <binary> -- <args>` |
| Create a pinned user-level shim | `gitx install <source> [--as <name>]` |
| Re-resolve an installed shim explicitly | `gitx update <tool> [--check]` |
| List or remove shims | `gitx list`; `gitx remove <tool>` |
| Inspect or clean local state | `gitx cache list`; `gitx cache remove <key>`; `gitx cache clear` |
| Check detected Git, runtime, and package toolchains | `gitx doctor` |
| Enable tab completion | `gitx completion <shell>` |

GitX 0.3 intentionally has one command grammar: the commands in the table are the supported interface. For `run` and `exec`, `--preview` (or `-n`) resolves and shows the preflight without executing. For `update`, `--check` resolves and shows the diff without writing. Completion supports Bash, Zsh, Fish, PowerShell, and Elvish. Put tool arguments after `--` so GitX can reliably distinguish them from its own safety options. Use `gitx <command> --help` for copyable examples and the complete options.

Use `-v` or `--verbose` to send high-level stages to standard error, and `-vv` for provider-level search detail. `--progress auto|always|never` controls GitX's animated transfer and Git activity display. GitX reports exact bytes only for transfers it owns, such as a cache import. Package execution remains delegated to npm, Bun, or pnpm, whose own download output stays visible. This keeps `--format json` safe for scripts because its data remains on standard output.

`search` starts with GitX state and available on-device catalogs: installed packages, local APT or pacman metadata, installed Flatpaks, Snap, Cargo, Python, and npm global packages where their local manager is present. Outside strict local modes, Flatpak can also consult its cached remote summary. It then queries npm, the Bun registry selected by Bun configuration, GitHub, GitLab, and Codeberg. On Arch Linux it includes the AUR. Use `--device` for a strict device-only view, or `--source device` to keep the device sources local while adding a specific remote source. Every candidate says both its adapter `Source` and concrete `Origin`. Use repeatable or comma-separated `--source` values to narrow the search, `--details 1` for the complete result card, or `--format json` for scripts. `--select 1-3` and `--interactive` only print the next action. They never execute it. `update --check` resolves the original source and prints a pin and metadata diff without changing files.

For an internal repository, local cache, or package source GitX does not natively support, define an explicit user catalog in `~/.config/gitx/catalogs.toml` and search it with `--catalog <id>`. File-backed JSON or JSON Lines snapshots work in `--offline` and `--device`; configured commands do not, because GitX cannot prove that an arbitrary child command stays off the network. See [Catalog Search](docs/catalogs.md#user-catalogs) for the compact schema.

Previews never execute source code and do not require confirmation. Package lifecycle scripts require `--allow-scripts`; overriding a deny-level lifecycle-script finding additionally requires `--allow-dangerous-scripts`.

## Security boundary

GitX improves visibility; it is not an OS sandbox. Repository tools run with the invoking user's privileges, and permissions in `gitx.toml` are declarations used for warnings rather than enforced access controls. Inspect output and package scans are decision aids, not proof that code is safe.

GitX currently:

- resolves named Git refs to commits and caches commit snapshots;
- rejects unknown manifest fields and entry points whose canonical target escapes the tool directory;
- shows the manifest, runtime, entry point, declared permissions, and warnings before repository execution;
- resolves npm dist-tags to exact versions, records integrity metadata, and disables install lifecycle scripts by default where the selected backend supports that policy;
- checks published npm OS and CPU constraints against the current host and blocks an exact package that cannot run here;
- supports `--preview` for preflight-only checks and `--yes` for non-interactive confirmation.

`--yes` skips confirmation; it does not add isolation or weaken deny-level package checks. `gitx inspect --format json` emits decision-schema version `1` through its `schemaVersion` field.

## Current limits

- npm execution still delegates dependency materialization to npm, Bun, or pnpm; GitX-native tarball execution is not implemented.
- catalog search can discover local package state, local APT and pacman metadata, Flatpak state, a configured Bun registry, Git hosts, and the AUR, but GitX executes only its supported Git and npm source forms. System packages remain reviewable catalog results, not managed installs.
- npm exact versions and dist-tags are supported, but semver ranges are not.
- cached Git snapshots are commit-pinned but writable, and repository tools run directly from the cached tree.
- managed install shims are currently Unix-only, and local-path installs are not implemented; `gitx update` supports explicit re-resolution for Git and npm shims.
- optional CDN services, stronger policy enforcement, and sandbox wrappers remain future work.
- the crates.io name `gitx` is already owned by another project, so this package is marked non-publishable until a distribution name is chosen.

## Development checks

The repository CI runs formatting, strict Clippy, tests on Linux, macOS, and Windows, a Rust 1.88 compatibility job, and a RustSec advisory audit. Run the local equivalent with:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
```

## Documentation

| Document | Use it for |
| --- | --- |
| [Product Scope](docs/product-scope.md) | Product contract, guarantees, boundaries, and success criteria |
| [Architecture](docs/architecture.md) | Resolution, manifest, execution, and security design |
| [Package Execution](docs/runner-compatibility.md) | npm workflows, backend policy, search, installs, and cache direction |
| [CLI Guide](docs/cli-guide.md) | Complete command tree, every option, source selection, cache boundary, and common workflows |
| [Catalog Search](docs/catalogs.md) | Providers, Bun registry selection, platform compatibility, update checks, and planned adapters |
| [Current Roadmap](docs/phase-1-roadmap.md) | Ordered implementation milestones and acceptance criteria |
| [Repository Audit](docs/repository-audit-2026-08-20.md) | Verified state, completed hardening, remaining findings, and recommended next work |
| [Timing Report](docs/benchmark-2026-08-20.md) | Measured GitX overhead against native package managers and provider searches |
| [Archived MVP Timeline](docs/mvp-timeline.md) | Historical six-week proposal; not current status |
| [Cache and CDN Design](docs/cdn-cache.md) | Implemented local cache and proposed remote cache |
| [Deferred Idea Bank](docs/idea-bank.md) | Unscheduled product ideas and open questions |
| [Example Manifest](examples/gitx.toml) | Annotated-by-example `gitx.toml` shape |
