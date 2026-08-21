# Product Scope

This document defines GitX's durable product contract: who it serves, which workflows belong in the product, and what users may and may not infer from its safety signals. For setup and current implementation limits, start with the [README](../README.md). For mechanics, see [Architecture](architecture.md) and [Package Execution](runner-compatibility.md).

## Product definition

GitX is a local-first command runner that resolves a tool to an exact source, explains what will execute, and preserves that identity through caching and execution.

Its identity-defining wedge is **a safer NPX-style experience for Git repositories**. A developer should be able to run a repository tool with less ceremony than cloning and installing it manually, while seeing more trustworthy context than a conventional one-shot package prompt provides.

npm package search, execution, and installation are an in-scope extension. Search also gives users a local-first catalog view across device package state, cached platform metadata, configured Bun registries, major Git forges, and the AUR where it fits the host. A candidate must show both the adapter that found it and the concrete origin that supplied it. GitX should reuse its resolution, preflight, policy, and cache model rather than turn into another package manager.

## Problems and users

GitX addresses three related problems:

- **Weak trust signals for one-shot tools.** Conventional package prompts reveal little about the exact artifact, executable, lifecycle scripts, or requested access.
- **Too much ceremony for repository tools.** Cloning, reading setup instructions, installing dependencies, and cleaning up is disproportionate for a disposable utility.
- **No stable decision surface for agents.** Coding agents need structured facts that let them run, deny, or ask a person instead of blindly following shell instructions.

The initial users are developers running one-off tools, authors publishing small repository-native CLIs, teams using private utility repositories, and people delegating command execution to coding agents.

## Product contract

### Resolve before execution

GitX must distinguish a requested name from the immutable identity it resolves to:

| Source | Requested identity | Execution identity |
| --- | --- | --- |
| Local directory | Filesystem path | Contents present at execution time |
| Git named ref | Remote plus branch or tag | Full commit hash |
| Git commit pin | Remote plus commit | Resolved full commit hash |
| npm dist-tag | Package plus tag | Exact version plus published integrity, when available |
| npm exact version | Package plus version | Exact version plus published integrity, when available |

Mutable refs and dist-tags must be re-resolved and shown as mutable. A previous prompt, cache entry, or install must never silently convert a mutable name into a permanent trust decision. Semver ranges are not part of the current execution contract.

Local paths are intentionally different: GitX can inspect their current manifest, but it cannot promise immutability for files the user may edit between inspection and execution.

### Inspect before execution

`gitx inspect <source>` must not execute source code. It should expose the facts available for a decision, including:

- requested source and normalized source type;
- resolved remote, ref, commit, package, and exact version when applicable;
- manifest presence, tool name, runtime, and entry point;
- declared permissions, their enforcement status, and requested environment variables;
- exact package OS and CPU compatibility plus published runtime requirements when available;
- concrete warnings, risk classification, policy decision, and whether confirmation is required.

Human output should be concise. The target contract is a stable, machine-readable `--format json` decision surface for scripts and agents. The current decision schema is identified by `schemaVersion: 1` and reports `allow`, `ask`, or `deny`; compatible evolution rules still need to be frozen before release. A failed remote resolution may be reported as unresolved with a warning; it must not be presented as a successful verification. The npm execution preflight should additionally show published integrity metadata when it is available.

### Make execution explicit

Repository execution follows this sequence:

```text
parse source -> resolve immutable identity -> materialize/cache -> read manifest
             -> show preflight -> apply confirmation policy -> execute entry point
```

`gitx run` must support local directories and Git sources, forward arguments after `--`, and provide `--preview` (or `-n`) to stop after preflight. Named Git refs may be convenient inputs, but the actual remote run must use the resolved commit snapshot.

`gitx exec` applies the same shape to npm binaries: resolve metadata, choose an available backend, show the exact package and executable, apply scan and lifecycle-script policy, then delegate execution. `--yes` may skip an interactive confirmation, but it must not imply isolation or bypass a deny-level policy. Allowing lifecycle scripts requires a separate, explicit option.

### Keep manifests small and reviewable

Repository tools declare their interface in a root-level `gitx.toml`. Schema 1 requires:

- `schema = 1`;
- `name`;
- `runtime`;
- an `entry` intended to resolve to a file inside the tool directory.

`description` and `[metadata]` are optional. `[permissions]` may declare `network`, `read`, `write`, `env`, and `subprocess`; omitted permissions default to false or empty. See the [example manifest](../examples/gitx.toml).

Inspection, installation, and execution reject absolute paths and lexical `..` components, canonicalize the entrypoint, require a file, and reject symlink targets outside the canonical tool root. Local source files can still change after that check and before or during execution.

Permission declarations are inputs to inspection and policy. They do not currently restrict the child process at the operating-system level.

### Preserve immutable cache identities

Git snapshots are keyed by remote and commit. Package records distinguish mutable dist-tag pointers from exact versions and integrity-addressed artifacts. Cache commands must make local state inspectable and removable.

Verified cache import may copy matching artifacts from existing package-manager caches, but it must validate the expected integrity and leave the source cache untouched. Optional remote/CDN caches may accelerate public artifacts later; GitX's core workflows must remain local-first and usable without that service.

### Make persistent installs traceable

`gitx install` creates a user-level shim for a Git or npm source and records the pinned commit or exact package version. It does not add project dependencies or modify `package.json`. `gitx update <tool>` re-resolves that original source, prints a pin and metadata diff, and changes the record only by explicit user request. `gitx update <tool> --check` never writes. Invocation never updates a shim implicitly.

## Safety boundary

GitX can provide:

- exact-source resolution and visible mutable-to-immutable transitions;
- immutable cache keys and traceable installed shims;
- manifest and registry-metadata inspection;
- deterministic warnings and an agent-readable policy surface;
- confirmation gates and conservative lifecycle-script defaults;
- canonical repository-entrypoint containment at preflight time;
- integrity verification for npm artifacts imported into GitX's artifact cache.

GitX does not currently provide:

- OS-enforced filesystem, network, environment, or subprocess isolation;
- proof that a repository, package, maintainer, runtime, or dependency is trustworthy;
- complete malware detection or a guarantee that an allowed tool is harmless;
- full feature parity with every npm, Bun, pnpm, or Yarn runner;
- protection from a compromised runtime or package-manager backend.

The product must describe these limits plainly. Visibility is useful only when it is not marketed as containment.

## In-scope workflows

The first useful product includes:

- inspecting local, Git, and npm sources without executing them;
- resolving Git refs and caching commit-pinned snapshots;
- running manifest-defined repository entry points;
- producing versioned or explicitly frozen JSON for agent decisions;
- searching managed tools, project bins, cached package records, local package-manager state, local APT and pacman metadata, Flatpak state, configured user snapshots, npm, a configured Bun registry, GitHub, GitLab, Codeberg, and the AUR on Arch Linux without executing a result;
- running npm package binaries through a delegated backend;
- listing, removing, clearing, and safely importing cache entries;
- creating, listing, updating, and removing pinned user-level shims;
- using existing Git authentication for private repositories where system Git supports it.

## Out of scope

These are not requirements for the first useful product:

- replacing Git, npm, or package registries;
- making a hosted registry or CDN mandatory;
- perfect sandboxing or general-purpose malware analysis;
- a GUI, mobile client, package marketplace, or distributed publishing network;
- an organization-wide policy engine or enterprise administration plane;
- automatic updates of pinned tools during invocation;
- complete runtime, package-manager, or platform parity.

Future work may add some of these, but it must not weaken exact resolution, local-first operation, or honest risk reporting.

## Success criteria

GitX has proved its initial value when:

- a developer can inspect and run a compatible public repository tool in under 30 seconds after prerequisites are installed;
- an exact Git commit or npm version identifies the same source on repeat use;
- moving refs and dist-tags are visibly re-resolved instead of inheriting stale trust;
- the preflight gives a person more decision-relevant information than a conventional install prompt;
- once the policy contract is complete, an agent can use inspect JSON to allow, deny, or ask for confirmation without parsing prose;
- private Git sources work through existing system Git authentication;
- users can remove cached and installed state without hunting through implementation-specific directories.

## Prioritization rules

When a proposed feature conflicts with this contract, prefer:

1. traceable source identity over shorthand convenience;
2. an honest warning over unenforced security language;
3. explicit user intent over silent install, update, or execution;
4. local functionality over dependence on a hosted service;
5. a narrow, predictable workflow over imitation of other runners.
