# Package Execution

GitX supports repo-native tools and npm package binaries through one
purpose-built command surface. It is not a clone of another package runner:
GitX resolves an exact target, shows its preflight, then invokes the selected
backend only after policy permits execution.

For the end-to-end component flow, see [Architecture](architecture.md). Cache
storage and the proposed CDN are documented in
[CDN cache design](cdn-cache.md).

## Status at a glance

| Capability | Status | Current behavior |
| --- | --- | --- |
| Repo-native `gitx run` | Implemented | Runs a schema-1 `gitx.toml` entrypoint from a local directory or commit snapshot |
| npm `gitx exec` | Implemented, delegated | Resolves and scans metadata, then calls npm, bunx, or pnpm with an exact version |
| `gitx run npm:...` | Implemented | Routes to the same package path as `gitx exec` |
| `gitx search` | Implemented | Combines installed tools, project bins, exact-version cache records, and npm; selection prints commands but does not execute |
| Persistent install, update, and remove | Implemented, basic | Writes POSIX shell shims pinned to an npm version or Git commit; updates are explicit |
| Native npm materialization | Not implemented | Package managers still fetch dependencies and execute bins |
| Yarn execution backend | Not implemented | Yarn is detected by `gitx doctor` only |

## Repo-native runner

```bash
gitx run github:owner/repo@v1.0.0 -- --tool-flag
gitx run github:owner/repo#0123456789abcdef --yes -- --tool-flag
gitx run ./local-tool --preview
```

Remote refs resolve to exact commits before execution. Local directories are
used in place. Both paths require `gitx.toml`; source and manifest semantics are
covered in [Architecture](architecture.md#source-model).

Manifest runtime dispatch is:

| Runtime | Executable selected |
| --- | --- |
| `node` | `node` |
| `bun` | `bun` |
| `deno` | `deno` |
| `python` | `python3`, then `python` |
| `shell` | `sh` |

`shell` is warned as high risk. Inspection and execution use the same runtime
allow-list. Manifest permissions remain declarations rather than enforced
runtime restrictions.

## Package runner

The package interface is `gitx exec`:

```bash
gitx exec cowsay@1.6.0 -- hello
gitx exec prettier@latest --yes -- README.md --check
gitx exec --package @angular/cli@20.0.0 --bin ng -- new app
gitx exec ts-node -p typescript@5.8.3 -p ts-node@10.9.2 -- script.ts
```

`gitx exec` accepts:

- one target, interpreted as the package when no `--package` is supplied;
- repeatable `-p, --package` options when the command/bin name differs or
  several packages must be present;
- `--backend auto|npm|bun|pnpm`;
- `--yes` to pass the GitX confirmation gate;
- `--allow-scripts` to opt into backend install lifecycle scripts;
- `--allow-dangerous-scripts`, together with `--allow-scripts`, to override a
  deny-level finding in an install lifecycle script; and
- `--preview` or `-n` to stop after resolution, scanning, and preflight.

Forwarded arguments belong after `--`. Package names may be bare or prefixed
with `npm:` and may be scoped.

### Resolution behavior

GitX talks directly to `https://registry.npmjs.org` for package metadata and
search. It does not currently honor project npm registry configuration or
implement authenticated/private registry access.

Resolution follows these rules:

1. An explicitly requested exact version uses its cached version record when
   present.
2. Otherwise, a syntactically exact version is requested from the registry and
   cached.
3. An omitted version means `latest`. Omitted versions and named dist-tags
   refresh full package metadata on every resolution.
4. A dist-tag is stored as a mutable pointer to the exact version selected. If
   it moves, GitX reports the old and new targets and updates the pointer.
5. Semver ranges such as `^3.0.0` are rejected.

The resolver reads `bin` metadata and chooses the sole bin, or a bin matching
the unscoped package name. Packages with unresolved multi-bin ambiguity must be
run with an explicit `--bin <name>` plus `--package <package>@<version>`.

### Backend selection

`auto` makes a concrete choice before showing the preflight:

- without `--allow-scripts`, it tries npm and then pnpm;
- with `--allow-scripts` and at most one explicit `--package`, it tries npm,
  bunx, then pnpm; and
- with multiple explicit packages it tries npm and then pnpm.

An explicitly selected backend must exist on `PATH`; GitX does not silently
fall back from an explicit choice.

| Backend | Command emitted | Script handling | Current limits |
| --- | --- | --- | --- |
| npm | `npm exec --yes --package=<name>@<exact> -- <bin> ...` | Adds `--strict-allow-scripts` by default or `--dangerously-allow-all-scripts` with `--allow-scripts` | Strict mode can refuse packages whose required scripts are not approved |
| bun | `bunx <name>@<exact> ...` | GitX refuses execution without `--allow-scripts`, because bunx has no reliable per-run blocking flag | At most one explicit `--package` |
| pnpm | `pnpm --config.ignore-scripts=true dlx ...` by default | Explicitly enables `ignoreScripts`; with `--allow-scripts`, adds `--allow-build <name>` for selected packages | Transitive build behavior still follows pnpm policy when scripts are allowed |

npm receives `npm_config_cache=<gitx-cache>/npm/backend-cache`. bunx receives
the same directory through `BUN_INSTALL_CACHE_DIR`. pnpm receives
`pnpm_config_cache_dir=<gitx-cache>/npm/backend-cache` and stores its package
store below that directory too. These are backend-managed downloads, separate
from GitX's verified tarball store.

### Preflight and confirmation

The current npm scan uses registry metadata to check:

- missing SHA-512 integrity;
- `preinstall`, `install`, and `postinstall` scripts;
- suspicious fetch-to-shell or decode/eval script patterns;
- a combined dependency count of 75 or more; and
- native/platform-looking package names.

Although file-level scan primitives exist, `gitx exec` does not download and
inspect tarball contents before delegating. It does not yet query advisory
databases or scan the complete dependency graph.

Warnings require confirmation before execution. In a non-interactive session,
pass `--yes`. A preview stops after resolution and preflight, before deny or
confirmation gates, so `--preview` never needs `--yes`.

A suspicious install lifecycle command is a deny-level finding. Execution
requires both `--allow-scripts` and the separate
`--allow-dangerous-scripts` acknowledgement. Non-lifecycle package scripts are
not treated as execution denials because the delegated one-shot install does
not invoke them. Package preflight renders the effective `allow`, `ask`, or
`deny` policy separately from its risk level; an unoverridden deny finding is
reported as high risk rather than as a confirmation-only warning.

## Search and selection

```bash
gitx search eslint
gitx search eslint --recommend --limit 5
gitx search eslint --source installed --select 1
gitx search eslint --source project --project . --interactive
gitx search eslint --source bun,github --details 1
gitx search eslint --source device --details 1
gitx search eslint --device
gitx search eslint --format json
```

The default source set starts with managed GitX installs, the supplied project's
`node_modules/.bin`, and GitX's local exact-version package records. It then
uses available on-device package state: `system`, `apt`, `pacman`, `flatpak`,
`snap`, `cargo`, `python`, and `npm-global`. Remote sources follow: npm, the
registry selected by Bun configuration, GitHub, GitLab, and Codeberg. On an
Arch-based Linux host it also searches the AUR. Use repeatable or
comma-separated `--source` values to select a specific provider. `device`
expands to strict local sources, while `--device` is the strict local-only
shortcut. Local matches remain usable when a remote provider is unavailable;
default-provider failures are reported as notes rather than hiding those
matches. `--offline` skips remote providers, while an explicit remote source
with `--offline` fails instead of quietly changing the request.

`--catalog <id>` selects an explicitly configured user catalog. A JSON or JSON
Lines file snapshot works in `--offline`, `--device`, and `--source device`; a
configured command does not, because GitX cannot prove that it stays off the
network. See
[Catalog Search](catalogs.md#user-catalogs) for its schema.

Search renders at most 20 candidates (10 by default), with source, concrete
origin, version when known, a short description, and a compatibility state.
`--details 1` adds the full card for one result: description, homepage, update
time, platform and runtime requirements, provider facts, and the suggested next
action. `--format json` returns the same data with `schemaVersion: 2`; search
schema version 2 requires a concrete candidate origin.
`--recommend` marks an exact-name match when one exists, otherwise the first
displayed candidate. `--select` accepts forms such as `1`, `1 3`, or `1-4`;
`--interactive` prompts for the same input only on a terminal. Both paths print
copyable next commands and never execute a selected result.

## Persistent installs

```bash
gitx install prettier
gitx install npm:prettier@3.6.2
gitx install github:owner/repo@v1.0.0
gitx install github:owner/repo@v1.0.0 --as repo-tool
gitx list
gitx update repo-tool --check
gitx update repo-tool
gitx remove prettier
```

`gitx install` resolves mutable input immediately. npm installs require an
inferable bin and store an exact version plus its integrity metadata. Git
installs require a manifest and store an exact commit. Records live under
`$GITX_HOME/installed` (default `~/.gitx/installed`) and shims under
`$GITX_HOME/bin`.

The generated POSIX shim calls `gitx exec <exact> --yes` or
`gitx run <commit> --yes`; execution still uses the normal cache and backend
path. `gitx update <tool>` re-resolves the original source, preserves the
installed command name, and shows the old and resolved pins. It also reports
changes to runtime, integrity, description, homepage, license, entry point,
binary names, platform constraints, runtime requirements, and manifest
metadata. `--check` shows that decision without writing either file. A changed
pin replaces the record and shim atomically. A metadata-only change replaces
the record and leaves the shim alone. If replacing the shim fails, GitX restores
the previous record.

Installing local directories, selecting among ambiguous npm bins,
Windows-native shims, and storing scan reports in install records are not
implemented.

## Cache boundary

Package execution currently uses three different cache concepts:

- exact-version metadata and mutable dist-tag records owned by GitX;
- backend caches populated by npm, bunx, or pnpm; and
- a GitX-owned SHA-512 tarball store populated only by explicit cache import or
  library-level download primitives.

The tarball store is not an execution source yet. Git commit snapshots are
already reused for repo runs. The precise layout, integrity rules, and CDN
proposal are in [CDN cache design](cdn-cache.md).

## Remaining work

The next package-execution work should be treated as future work until code and
tests land:

- semver-range and configured/private-registry resolution;
- project-bin provenance and project policy integration;
- Git repository catalogs with trustworthy search provenance;
- tarball extraction, file scanning, dependency materialization, and direct
  execution from GitX's artifact store;
- Windows-native managed shims and local-path install semantics; and
- verified backend-version tests for lifecycle-script controls.

The optional public artifact service is a separate design; see
[Proposed CDN](cdn-cache.md#proposed-cdn).
