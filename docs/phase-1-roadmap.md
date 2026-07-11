# Current Roadmap

> This is the living implementation plan. The filename is retained so existing links do not break; the original Phase 1 foundation is already present in the current worktree.

Milestones are ordered by dependency, not by calendar estimate. Move work into the verified baseline only after the code and tests support the claim.

## Where To Look

- [README](../README.md): current commands and quickstart.
- [Product Scope](product-scope.md): product promise and MVP boundary.
- [Runner Compatibility](runner-compatibility.md): package-runner behavior and compatibility gaps.
- [Architecture](architecture.md): component boundaries and security model.
- [Archived MVP Timeline](mvp-timeline.md): the original six-week proposal.
- [Deferred Idea Bank](idea-bank.md): unscheduled options and open product questions.

## Verified Baseline

This is a report of code in the worktree, not a claim that GitX has been published or stabilized. The [README](../README.md) owns the command-by-command inventory; these are only the facts needed to explain the roadmap's starting point.

- The original Rust foundation is present: typed local, Git, and npm sources; manifest inspection with JSON output; local and remote execution; and commit-pinned snapshots whose cached files remain writable.
- The prototype extends beyond that foundation with npm search and execution, compatibility aliases, delegated package-manager backends, metadata scanning, and mutable-tag/exact-version caching.
- Git and npm sources can create persistent shims, while cache commands manage Git snapshots, npm records, backend data, and imported integrity-verified tarballs.
- Tarball download, integrity verification, and import primitives exist, but GitX does not yet materialize and execute npm dependency graphs itself.
- `cargo test --all-targets` passes for the current worktree.

## Active Milestones

### 1. Stabilize Contracts And Tests

- Version or explicitly freeze the agent-facing inspect JSON contract.
- Add hermetic remote-Git integration tests using `file://` fixtures: named-ref resolution, exact commits, cache hits, moved refs, and failed fetches.
- Cover confirmation, tool exit codes, cache removal/import, and installed-shim execution at the CLI boundary.
- Remove stale Phase 1 terminology from tests and command documentation.
- Make offline and degraded-network behavior explicit instead of relying on incidental backend errors.

Exit condition: the core local, Git, cache, and install flows have deterministic integration coverage, and compatibility-breaking output changes require an intentional schema decision.

### 2. Finish The GitX-Owned Npm Artifact Path

- Connect integrity-verified tarball download and import code to package execution.
- Materialize package bins and dependency graphs in GitX's content-addressed cache.
- Feed archive file summaries into the scanner, not metadata alone.
- Add npm semver-range resolution while preserving exact-version and dist-tag semantics.
- Use GitX artifacts directly on cache hits; keep package-manager subprocesses as compatibility fallbacks.

Exit condition: a pinned package can be resolved, verified, materialized, scanned, and executed through a tested GitX-owned path without starting another package manager on a warm cache hit.

### 3. Complete Managed Installs

- Define and implement safe local-path install semantics; local installs are currently rejected.
- Add an explicit update command that re-resolves mutable sources and records the new immutable target.
- Make shim and record writes transactional and handle name collisions clearly.
- Test Git, npm, update, uninstall, and failed-install rollback flows.

Exit condition: every managed command points to an immutable recorded target, updates are explicit, and interrupted operations do not leave partial installs.

### 4. Make The Security Contract Honest And Enforceable

- Distinguish declared permissions, detected scan signals, backend-enforced controls, and uncontained access in all user- and agent-facing output.
- Keep `allow`, `ask`, and `deny` decisions consistent across Git and package flows.
- Test lifecycle-script blocking and every override for each delegated backend.
- Enforce and test canonical entrypoint containment, including symlink targets, before describing repository entrypoints as confined to the source tree.
- Define the smallest realistic permission-enforcement boundary before claiming isolation.
- Smoke-test private Git sources through existing user credentials without storing credentials in GitX.

Exit condition: no risk label implies containment that GitX does not provide, and every security override is explicit in the command and output.

### 5. Prepare A Reproducible Release

- Run formatting, linting, unit, integration, and smoke checks in CI across the supported OS/runtime matrix.
- Harden temporary-file cleanup, concurrent cache writes, corrupt-cache recovery, and user-facing errors.
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
