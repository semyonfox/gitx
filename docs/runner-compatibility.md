# Runner Compatibility

GitX supports repo-native tools and npm package binaries, but it does not try
to be a byte-for-byte replacement for every package runner. This document
separates the compatibility that exists today from the intended direction.

For the end-to-end component flow, see [Architecture](architecture.md). Cache
storage and the proposed CDN are documented in
[CDN cache design](cdn-cache.md).

## Status at a glance

| Capability | Status | Current behavior |
| --- | --- | --- |
| Repo-native `gitx run` | Implemented | Runs a schema-1 `gitx.toml` entrypoint from a local directory or commit snapshot |
| npm `gitx x` | Implemented, delegated | Resolves and scans metadata, then calls npm, bunx, or pnpm with an exact version |
| `gitx run npm:...` | Implemented | Routes to the same package path as `gitx x` |
| `gitx search` | Partial | Queries npm only and renders candidates; it does not prompt for a selection |
| `gitx npx`, `gitx bunx`, `gitx dlx` | Partial | Thin fixed-backend aliases for a simple target; not full syntax emulation |
| Persistent install/uninstall | Implemented, basic | Writes POSIX shell shims pinned to an npm version or Git commit |
| Native npm materialization | Not implemented | Package managers still fetch dependencies and execute bins |
| Yarn execution backend | Not implemented | Yarn is detected by `toolchains` only |

## Repo-native runner

```bash
gitx run github:owner/repo@v1.0.0 -- --tool-flag
gitx run github:owner/repo#0123456789abcdef --yes -- --tool-flag
gitx run ./local-tool --dry-run
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

`shell` is warned as high risk. Execution supports `deno`, although the
inspection warning allow-list currently omits it. Manifest permissions remain
declarations rather than enforced runtime restrictions.

## Package runner

The primary package interface is:

```bash
gitx x cowsay@1.6.0 -- hello
gitx x prettier@latest --yes -- README.md --check
gitx x ng -p @angular/cli@20.0.0 -- new app
gitx x ts-node -p typescript@5.8.3 -p ts-node@10.9.2 -- script.ts
```

`gitx x` accepts:

- one target, interpreted as the package when no `--package` is supplied;
- repeatable `-p, --package` options when the command/bin name differs or
  several packages must be present;
- `--backend auto|npm|bun|pnpm`;
- `--yes` to pass the GitX confirmation gate;
- `--allow-scripts` to opt into install lifecycle scripts and override
  deny-level script findings; and
- `--dry-run` to stop after resolution, scanning, and confirmation.

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
run with an explicit command plus `--package`.

### Backend selection

`auto` makes a concrete choice before showing the preflight:

- for a normal single-package request, or one explicit `--package`, it tries
  bunx, npm, then pnpm when lifecycle policy permits bunx;
- with multiple explicit packages it tries npm, then pnpm; and
- when lifecycle scripts are declared but not allowed, it skips bunx because
  the adapter has no script-blocking flag.

An explicitly selected backend must exist on `PATH`; GitX does not silently
fall back from an explicit choice.

| Backend | Command emitted | Script handling | Current limits |
| --- | --- | --- | --- |
| npm | `npm exec --yes --package=<name>@<exact> -- <bin> ...` | Adds `--strict-allow-scripts` by default or `--dangerously-allow-all-scripts` with `--allow-scripts` | Uses the installed npm CLI's support for those flags |
| bun | `bunx <name>@<exact> ...` | Refuses known lifecycle scripts unless `--allow-scripts` | At most one explicit `--package` |
| pnpm | `pnpm dlx <name>@<exact> ...` | Adds `--allow-build <name>` only when scripts are allowed; otherwise relies on pnpm's own default/configured policy | Behavior therefore depends on the installed pnpm version and configuration |

npm receives `npm_config_cache=<gitx-cache>/npm/backend-cache`. bunx receives
the same directory through `BUN_INSTALL_CACHE_DIR`; pnpm currently receives
`npm_config_cache`. These are backend-managed downloads, separate from GitX's
verified tarball store.

### Preflight and confirmation

The current npm scan uses registry metadata to check:

- missing SHA-512 integrity;
- `preinstall`, `install`, and `postinstall` scripts;
- suspicious fetch-to-shell or decode/eval script patterns;
- a combined dependency count of 75 or more; and
- native/platform-looking package names.

Although file-level scan primitives exist, `gitx x` does not download and
inspect tarball contents before delegating. It does not yet query advisory
databases or scan the complete dependency graph.

Warnings require confirmation. In a non-interactive session, pass `--yes`.
The current package preflight also records the selected lifecycle-script policy
as a warning even when no lifecycle scripts were declared, so package runs
normally encounter the confirmation gate. `--dry-run` is checked after that
gate; automated dry runs should use `--yes --dry-run`.

A suspicious install command is a deny-level finding. Without
`--allow-scripts`, GitX stops before prompting or executing. The override both
acknowledges that finding and enables lifecycle scripts, so it should be used
sparingly.

## Compatibility aliases

These commands are implemented as thin adapters:

```bash
gitx npx cowsay hello
gitx bunx vite dev
gitx dlx eslint .
```

They select npm, bunx, and pnpm respectively, treat the first token as the
package target, and forward every remaining token as tool arguments. They do
not parse native runner options such as `npx -p`, `bunx --bun`, or the runners'
own confirmation/cache flags. They also do not expose GitX's `--yes`,
`--allow-scripts`, or `--dry-run` options.

Because the aliases leave `yes` false, a warning in a non-interactive session
causes them to stop. Use `gitx x --backend ... --yes` for automation and any
non-trivial invocation.

GitX does not install binaries named `npx`, `bunx`, or `pnpm`, so it does not
shadow the real runners.

## Search and selection

```bash
gitx search eslint
gitx search eslint --best
```

Search currently calls npm's search endpoint and renders up to 20 candidates
with name, version, registry score, and description. `--best` labels the first
registry result as recommended and prints a runnable command; it does not yet
apply a confidence threshold.

The reusable selector can render and parse forms such as `1`, `1 3`, and
`1-4`, but the CLI does not wire that parser to an interactive prompt. Search
does not yet combine local project bins, installed GitX shims, cache entries,
Git repositories, or other catalogs.

## Persistent installs

```bash
gitx install prettier
gitx install npm:prettier@3.6.2
gitx install github:owner/repo@v1.0.0
gitx installed list
gitx uninstall prettier
```

`gitx install` resolves mutable input immediately. npm installs require an
inferable bin and store an exact version plus its integrity metadata. Git
installs require a manifest and store an exact commit. Records live under
`$GITX_HOME/installed` (default `~/.gitx/installed`) and shims under
`$GITX_HOME/bin`.

The generated POSIX shim calls `gitx x <exact> --yes` or
`gitx run <commit> --yes`; execution still uses the normal cache and backend
path. Installing local directories, selecting among ambiguous npm bins,
updating an install, Windows-native shims, and storing scan reports in install
records are not implemented.

## Cache boundary

Package execution currently uses three different cache concepts:

- exact-version metadata and mutable dist-tag records owned by GitX;
- backend caches populated by npm, bunx, or pnpm; and
- a GitX-owned SHA-512 tarball store populated only by explicit cache import or
  library-level download primitives.

The tarball store is not an execution source yet. Git commit snapshots are
already reused for repo runs. The precise layout, integrity rules, and CDN
proposal are in [CDN cache design](cdn-cache.md).

## Known gaps

The next compatibility work should be treated as future work until code and
tests land:

- semver-range and configured/private-registry resolution;
- local project-bin preference and project policy integration;
- confidence-gated interactive search and multi-source catalogs;
- fuller native option parsing for compatibility aliases;
- tarball extraction, file scanning, dependency materialization, and direct
  execution from GitX's artifact store;
- an update command for persistent installs;
- a cleaner split between deny overrides and lifecycle-script permission;
- non-prompting dry runs; and
- alignment between the `deno` executor and inspection allow-list.

The optional public artifact service is a separate design; see
[Proposed CDN](cdn-cache.md#proposed-cdn).
