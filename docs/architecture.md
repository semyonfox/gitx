# Architecture

This document describes the code in `src/` as it exists today. Proposed work is
kept in [Future boundaries](#future-boundaries) so it is not mistaken for a
runtime guarantee.

For command and backend compatibility, see
[Runner compatibility](runner-compatibility.md). For the local artifact store
and proposed remote cache, see [CDN cache design](cdn-cache.md).

## System flow

GitX has two execution paths behind one CLI:

```text
CLI (`cli.rs`)
  -> parse source (`source.rs`)
  -> Git or local source
       -> resolve/fetch Git snapshot when remote (`git.rs`)
       -> parse gitx.toml (`manifest.rs`)
       -> show manifest-based preflight (`inspect.rs`, `run.rs`)
       -> confirmation gate (`prompt.rs`)
       -> invoke the declared runtime
  -> npm source
       -> resolve registry metadata (`npm.rs`)
       -> cache exact-version and dist-tag records (`cache.rs`)
       -> scan package metadata (`scanner.rs`)
       -> confirmation gate
       -> delegate execution to npm, bunx, or pnpm
```

Persistent installs do not copy a separate executable. `install.rs` writes a
small shell shim that calls `gitx` again with the resolved npm version or Git
commit.

## Source model

`Source::parse` recognizes four source kinds:

| Kind | Accepted form | Resolution |
| --- | --- | --- |
| Local | `./tool`, `../tool`, an absolute path, or an existing path | Classifies the path as local; running requires a directory and makes no cache copy |
| GitHub | `github:owner/repo`, `github:owner/repo@ref`, `github:owner/repo#commit` | Expands to the repository's HTTPS Git URL |
| Git | A recognized Git URL, optionally ending in `.git@ref` or `.git#commit` | Uses the supplied remote |
| npm | `npm:package`, `npm:package@version`, including scoped names | Resolves against the public npm registry |

The separators are implemented, not provisional: `@name` is a named Git ref
and `#hex` is a commit. A missing Git ref resolves `HEAD`. Bare package names
are accepted by `gitx x` and `gitx install`, but not by the general source
parser.

### Git resolution and snapshots

Remote execution requires the system `git` binary. A named ref is resolved
with `git ls-remote`; a peeled tag wins over a tag, branch, `HEAD`, or other
matching ref. GitX then:

1. resolves the request to a commit;
2. reuses a completed snapshot for that remote and commit when present;
3. otherwise shallow-fetches the commit into a temporary cache directory;
4. falls back to fetching the named ref when the server rejects direct commit
   fetches, and rejects the fetch if that ref moved meanwhile;
5. checks out detached `FETCH_HEAD`, verifies the resulting hash, removes
   `.git`, and atomically moves the snapshot into place; and
6. writes a `.complete` JSON marker beside it.

Commit prefixes must contain at least seven hexadecimal characters. The final
cache directory always uses the full 40-character commit hash.

The snapshot is *identity-pinned*, but it is not filesystem-immutable. GitX
runs the entrypoint from the cached tree without making that tree read-only or
creating a disposable work copy. A tool can therefore modify its own cached
snapshot; this is a known boundary, not an isolation guarantee.

### npm resolution

The npm path resolves package metadata before choosing a backend. Exact
versions can be reused from GitX's version-record cache. Omitted versions and
dist-tags (including `latest`) are refreshed from the registry and recorded as
mutable pointers to exact versions. Semver ranges are not implemented.

GitX currently caches metadata and delegates dependency materialization and
execution. Its verified tarball cache is not yet connected to `gitx x`; see
[CDN cache design](cdn-cache.md#implemented-local-cache).

## Manifest contract

Repo-native tools must provide `gitx.toml` at the source root. Schema `1` has
four required fields:

```toml
schema = 1
name = "repo-audit"
runtime = "node"
entry = "src/main.js"
```

`description`, `[permissions]`, and `[metadata]` are optional. Permission
fields default to `false` or an empty list:

```toml
[permissions]
network = true
read = ["."]
write = ["./reports"]
env = ["GITHUB_TOKEN"]
subprocess = false
```

The complete example is in [`examples/gitx.toml`](../examples/gitx.toml).
Metadata values must be strings.

The permission block is currently descriptive. It feeds inspection warnings,
but GitX does not filter environment variables, block network or subprocesses,
or enforce read/write paths. The child process inherits the caller's
environment, standard streams, and working directory.

GitX applies a lexical entrypoint check only when `run` executes a manifest:
`entry` may not be absolute or contain a parent-directory component, and the
joined path must be a file. It does not canonicalize the entrypoint, so a
symlink inside the source tree can still resolve outside it. Manifest parsing,
inspection, and installation do not apply this run-time file check.

## Runtime execution

Repo manifests currently dispatch as follows:

| Manifest runtime | Executable |
| --- | --- |
| `node` | `node` |
| `bun` | `bun` |
| `deno` | `deno` |
| `python` | first available of `python3`, `python` |
| `shell` | `sh` |

Arguments after `--` are appended after the entrypoint. Exit status and
standard I/O pass through. Shell manifests trigger a warning. There is a
current inconsistency worth preserving in tests and fixing deliberately:
execution supports `deno`, while inspection labels it unrecognized.

## Inspection and policy

`gitx inspect` builds one serializable `InspectReport`. Its JSON fields are:

```text
source, sourceType, resolvedRemote, requestedRef, resolvedCommit,
package, packageVersion, manifestPath, manifestFound, toolName,
entrypoint, runtime, permissions, warnings, riskLevel,
policyDecision, requiresConfirmation
```

Local manifest errors fail inspection. Remote resolution failures are instead
returned as warnings with unresolved fields, which lets inspection remain
useful when a host is unavailable.

Current policy is deliberately small:

- named or missing Git refs, shell entrypoints, sensitive environment names,
  broad writes, network, subprocesses, and unknown runtimes produce warnings;
- npm inspection checks registry metadata for integrity, lifecycle scripts,
  suspicious install commands, dependency count, and native-looking names;
- warnings normally require an interactive confirmation, or `--yes` in a
  non-interactive session; and
- npm deny-level script findings require the explicit `--allow-scripts`
  override.

`InspectReport` currently emits `allow` when there are no warnings and `ask`
otherwise; it does not emit `deny`. Package execution applies its deny-level
check separately. The JSON shape is machine-readable but not yet versioned or
frozen as a compatibility contract.

This is deterministic preflight analysis, not malware detection. The npm
scanner has file-level input types, but current package execution supplies only
registry metadata; tarball contents are not scanned.

## Module boundaries

| Module | Responsibility |
| --- | --- |
| `cli` | Clap command surface and request construction |
| `source` | Source syntax and typed source representation |
| `git` | Ref resolution and commit snapshot materialization |
| `manifest` | Schema-1 TOML parsing and validation |
| `inspect` | Human/JSON report rendering and manifest risk warnings |
| `npm` | Registry search/resolution, package preflight, backend delegation |
| `scanner` | Deterministic npm metadata/file findings |
| `artifact` | npm SHA-512 verification and tarball import/download primitives |
| `cache` | Cache paths, listing, removal, and external-cache import |
| `run` / `prompt` | Repo execution and confirmation |
| `install` | Pin records and generated command shims |
| `selector` | Candidate rendering and selection parsing primitives |
| `toolchain` | PATH and version detection |

## Security boundary

Implemented protections are source resolution to an exact Git commit or npm
version, Git ref race detection, lexical entrypoint checks, npm SHA-512
verification during tarball import, visible warnings, and an explicit prompt.

GitX does not currently provide a sandbox, enforce manifest permissions,
verify commit signatures, consult vulnerability advisories, isolate the
runtime, guarantee symlink targets remain inside the source tree, protect
cached snapshots from writes, or prove that code is safe. Installed runtimes
and delegated package managers remain inside the trusted computing base.

## Future boundaries

These are design directions, not implemented behavior:

- execute npm packages from a GitX-owned materialized store;
- run cached Git sources from disposable or read-only workspaces;
- enforce canonical entrypoint containment, including symlink targets;
- enforce manifest permissions with platform-specific isolation;
- add signed policy and advisory inputs;
- add a confidence-gated interactive search selector; and
- optionally fetch verified public artifacts and scan reports from a CDN.

The package compatibility gaps are tracked in
[Runner compatibility](runner-compatibility.md#known-gaps). The CDN protocol
and rollout order live only in [CDN cache design](cdn-cache.md#proposed-cdn).
