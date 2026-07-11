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

## Quick start

You need Rust and Cargo. Remote Git sources also require `git`, and execution requires the runtime declared by the tool. Package mode delegates materialization to an installed npm, Bun, or pnpm backend.

From this repository:

```bash
cargo test
cargo run -- inspect ./fixtures/valid-node-tool
cargo run -- inspect ./fixtures/valid-node-tool --json
cargo run -- run ./fixtures/minimal-tool --dry-run -- --example
```

Build the standalone binary with:

```bash
cargo build --release
./target/release/gitx --help
```

## Common workflows

| Goal | Command |
| --- | --- |
| Inspect without executing | `gitx inspect <source> [--json]` |
| Preview a repository tool | `gitx run <source> [--yes] --dry-run -- <args>` |
| Run a repository tool | `gitx run <source> [--yes] -- <args>` |
| Find an npm package | `gitx search <query> [--best]` |
| Preview an npm binary | `gitx x <package>[@version] --yes --dry-run -- <args>` |
| Run an npm binary | `gitx x <package>[@version] [--yes] -- <args>` |
| Create a pinned user-level shim | `gitx install <source>` |
| List or remove shims | `gitx installed list`; `gitx uninstall <tool>` |
| Inspect or clean local state | `gitx cache list`; `gitx cache remove <key>`; `gitx cache clear` |
| Check detected JS/package toolchains | `gitx toolchains` |

`gitx npx`, `gitx bunx`, and `gitx dlx` provide basic compatibility entry points. Use `gitx <command> --help` for the complete options.

## Security boundary

GitX improves visibility; it is not an OS sandbox. Repository tools run with the invoking user's privileges, and permissions in `gitx.toml` are declarations used for warnings rather than enforced access controls. Inspect output and package scans are decision aids, not proof that code is safe.

GitX currently:

- resolves named Git refs to commits and caches commit snapshots;
- shows the manifest, runtime, entry point, declared permissions, and warnings before repository execution;
- resolves npm dist-tags to exact versions, records integrity metadata, and disables install lifecycle scripts by default where the selected backend supports that policy;
- supports `--dry-run` for preflight-only checks and `--yes` for non-interactive confirmation.

`--yes` skips confirmation; it does not add isolation or weaken deny-level package checks.

## Current limits

- npm execution still delegates dependency materialization to npm, Bun, or pnpm; GitX-native tarball execution is not implemented.
- npm exact versions and dist-tags are supported, but semver ranges are not.
- cached Git snapshots are commit-pinned but writable, and repository tools run directly from the cached tree.
- local-path installs and an installed-tool update command are not implemented.
- optional CDN services, stronger policy enforcement, and sandbox wrappers remain future work.

## Documentation

| Document | Use it for |
| --- | --- |
| [Product Scope](docs/product-scope.md) | Product contract, guarantees, boundaries, and success criteria |
| [Architecture](docs/architecture.md) | Resolution, manifest, execution, and security design |
| [Runner Compatibility](docs/runner-compatibility.md) | npm-compatible workflows, backend policy, scanning, and cache direction |
| [Current Roadmap](docs/phase-1-roadmap.md) | Ordered implementation milestones and acceptance criteria |
| [Archived MVP Timeline](docs/mvp-timeline.md) | Historical six-week proposal; not current status |
| [Cache and CDN Design](docs/cdn-cache.md) | Implemented local cache and proposed remote cache |
| [Deferred Idea Bank](docs/idea-bank.md) | Unscheduled product ideas and open questions |
| [Example Manifest](examples/gitx.toml) | Annotated-by-example `gitx.toml` shape |
