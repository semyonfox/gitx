# Current Roadmap

> This is the living implementation plan. The filename is retained so existing links do not break; the original Phase 1 foundation is already present in the current worktree.

Milestones are ordered by dependency, not by calendar estimate. Move work into the verified baseline only after the code and tests support the claim.

## Where To Look

- [README](../README.md): current commands and quickstart.
- [Product Scope](product-scope.md): product promise and MVP boundary.
- [Package Execution](runner-compatibility.md): package-runner behavior, search, installs, and remaining work.
- [Catalog Search](catalogs.md): provider behavior, compatibility states, update checks, and the next adapters.
- [Architecture](architecture.md): component boundaries and security model.
- [Archived MVP Timeline](mvp-timeline.md): the original six-week proposal.
- [Deferred Idea Bank](idea-bank.md): unscheduled options and open product questions.

## Verified Baseline

This is a report of code in the worktree, not a claim that GitX has been published or stabilized. The [README](../README.md) owns the command-by-command inventory; these are only the facts needed to explain the roadmap's starting point.

- The original Rust foundation is present: typed local, Git, and npm sources; manifest inspection with JSON output; local and remote execution; and commit-pinned snapshots whose cached files remain writable.
- The prototype extends beyond that foundation with local-first multi-source search, npm execution, delegated package-manager backends, metadata scanning, mutable-tag/exact-version caching, and explicit installed-tool updates.
- Search covers managed installs, project bins, GitX cached npm records, local system package state, local APT and pacman metadata, Flatpak installs, Snap, Cargo installs, Python packages, npm global packages, npm, the registry selected by Bun configuration, GitHub, GitLab, Codeberg, and the AUR on Arch-based Linux. Outside strict local modes it can also ask Flatpak for a cached remote summary. The device adapters are host-aware, run at most four at once, read existing local state only, and stop after five seconds. Remote lookups also use at most four workers and a 15-second request limit. Every result exposes a source, concrete origin, version when known, description when known, compatibility state, provider facts, and a non-executing next action.
- Users can select a file-backed or command-backed catalog in `~/.config/gitx/catalogs.toml`. File snapshots work with `--offline` and `--device`; command catalogs are blocked there because GitX cannot constrain a child process's network behavior.
- Exact npm inspection and execution enforce published OS and CPU constraints. They show engine requirements without yet semver-checking the local runtime. `gitx update --check` prints pin and metadata changes without writing; a metadata-only update does not rewrite a shim.
- The canonical everyday CLI is `inspect`, `run`, `exec`, `search`, `install`, `update`, `list`, `remove`, `cache`, `doctor`, and `completion`. Command help includes examples, package binaries can be selected explicitly with `--bin`, installs can be named with `--as`, and completion scripts are generated on demand.
- Git and npm sources can create persistent shims; `gitx update` re-resolves their original source without changing an installed name. Cache commands manage Git snapshots, npm records, backend data, and imported integrity-verified tarballs.
- Tarball download, integrity verification, and import primitives exist, but GitX does not yet materialize and execute npm dependency graphs itself.
- Inspect JSON carries decision-schema version 1, has a checked contract fixture, explicitly reports that permissions are unenforced, and aligns `allow`, `ask`, and npm deny findings.
- Canonical entrypoint containment is enforced during inspection, installation, and execution, including symlink targets.
- Git remotes use collision-resistant cache keys, unsafe remote-helper transports are rejected, and checkout ignores global filters and hooks after authenticated fetches complete.
- Npm metadata caches and integrity-addressed tarball imports use atomic writes, recover from corrupt records, retain old exact versions when tags move, and tolerate concurrent imports.
- Hermetic Git tests cover named refs, exact commits, cache hits, moved refs, and recovery from incomplete snapshots; CLI tests cover JSON contracts, previews, exit-code propagation, cache removal/import, installed-shim forwarding and updates, multi-source search, and destructive-path guards.
- Formatting, strict Clippy, multi-OS tests, MSRV tests, and RustSec auditing are defined in CI.
- `cargo test --all-targets` passes for the current worktree.

## Active Milestones

### 1. Stabilize Contracts And Tests

- Freeze evolution rules for decision-schema version 1; the first exact contract fixture is now checked in.
- Extend hermetic remote-Git coverage to failed fetches and the ref-moved-during-fallback path.
- Cover affirmative and negative interactive confirmation through a real pseudo-terminal; the non-interactive refusal path, cache removal/import, and installed-shim forwarding now have hermetic CLI coverage.
- Remove stale Phase 1 terminology from tests and command documentation.
- Add fixed-response tests for every remote catalog adapter, including malformed response and provider-timeout paths.

Exit condition: the core local, Git, cache, and install flows have deterministic integration coverage, and decision-schema changes require an intentional versioning decision.

### 2. Broaden catalogs without fake install claims

- Add native catalog adapters in host order: dnf/zypper/apk on Linux, richer
  Homebrew formula and cask metadata, then winget and Scoop on Windows. Local
  system state, APT, pacman, Flatpak, Snap, Cargo, Python, npm global, and
  Chocolatey are already covered where present.
- Add language registry adapters for crates.io, PyPI, RubyGems, and Go module
  discovery, with the exact next action and version fields each provider can
  support.
- Add private-registry and private-forge authentication handoff without reading
  tokens into install records, terminal output, or JSON.
- Define how GitX tests system catalog adapters without requiring a package
  manager, network access, or a specific host in CI.

Exit condition: every new provider has fixed-response tests, bounded latency,
offline semantics, a compatibility rule, and a next action that GitX can state
honestly.

### 3. Finish The GitX-Owned Npm Artifact Path

- Connect integrity-verified tarball download and import code to package execution.
- Materialize package bins and dependency graphs in GitX's content-addressed cache.
- Feed archive file summaries into the scanner, not metadata alone.
- Add npm semver-range resolution while preserving exact-version and dist-tag semantics.
- Use GitX artifacts directly on cache hits; retain package-manager subprocesses only where the native path is not ready.

Exit condition: a pinned package can be resolved, verified, materialized, scanned, and executed through a tested GitX-owned path without starting another package manager on a warm cache hit.

### 4. Extend Managed Installs

- Define and implement safe local-path install semantics; local installs are currently rejected.
- Add richer installed-tool metadata without storing secrets or scan output that cannot be reproduced.
- Add hermetic Git-update, interrupted-replacement, and failed-install rollback coverage.
- Implement Windows-native shims while preserving explicit update semantics.

Exit condition: every managed command points to an immutable recorded target, updates are explicit, and interrupted operations do not leave partial installs.

### 5. Make The Security Contract Honest And Enforceable

- Add structured backend-enforcement details alongside the existing explicit unenforced-permissions field and human-readable policy output.
- Keep the now-aligned `allow`, `ask`, and `deny` decisions consistent as new scan sources land.
- Test lifecycle-script blocking and every override for each delegated backend.
- Isolate execution from writable cached Git snapshots so a previous run cannot alter a later run's source.
- Define the smallest realistic permission-enforcement boundary before claiming isolation.
- Smoke-test private Git sources through existing user credentials without storing credentials in GitX.

Exit condition: no risk label implies containment that GitX does not provide, and every security override is explicit in the command and output.

### 6. Prepare A Reproducible Release

- Run formatting, linting, unit, integration, and smoke checks in CI across the supported OS/runtime matrix.
- Finish hardening concurrent Git snapshot markers and cross-platform cache replacement; npm records, tarballs, install files, and terminal error rendering are already hardened.
- Document repeatable demos for local tools, pinned and mutable Git refs, npm execution, cache reuse, and managed installs.
- Choose distribution channels and complete the GitX naming-conflict check before publishing binaries.

Exit condition: a tagged build can be reproduced, installed on each supported platform, and exercised from clean state using the documented demos.

## Not On The Active Roadmap

The current milestones do not commit GitX to a central registry, hosted team catalog, full cross-platform sandbox, signed-release policy, lazy repository mount, or broader source-control replacement. Those belong in the [Deferred Idea Bank](idea-bank.md) until they have evidence, scope, and acceptance criteria. The optional hosted cache has its own [Cache and CDN Design](cdn-cache.md), but it is not required for the local-first runner.

## Maintenance Rule

Keep this file actionable:

- Verified, tested behavior belongs in the baseline or README.
- Scheduled implementation belongs in one active milestone.
- Unscheduled possibilities belong in the idea bank.
- Superseded forecasts remain historical rather than being rewritten as progress reports.
