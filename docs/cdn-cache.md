# Cache and CDN Design

GitX has a working local cache and verified npm tarball primitives. It does
**not** have a CDN client, CDN server, remote cache configuration, or native npm
artifact runner. This document describes the implemented local behavior first,
then labels the remote design explicitly.

Runner and package-manager behavior lives in
[Runner compatibility](runner-compatibility.md). Git snapshot resolution is
explained in [Architecture](architecture.md#git-resolution-and-snapshots).

## Implemented local cache

### Root and layout

The cache root is selected in this order:

1. `GITX_CACHE_DIR`, when set;
2. `$XDG_CACHE_HOME/gitx`, when `XDG_CACHE_HOME` is set; or
3. `$HOME/.cache/gitx`.

GitX creates this layout:

```text
<cache>/
  .gitx-cache-v1
  git/snapshots/<sha256-remote>/<full-commit>/
  git/snapshots/<sha256-remote>/<full-commit>.complete
  npm/metadata/<package>.json
  npm/tags/<package>.json
  npm/versions/<package>/<version>.json
  npm/tarballs/sha512-<hex>.tgz
  npm/backend-cache/
```

| Area | Key | Current consumer |
| --- | --- | --- |
| Git snapshot | SHA-256 remote key plus full commit hash | `inspect`, `run`, and Git installs |
| npm raw metadata | Package name | Dist-tag and non-exact resolution |
| npm tag record | Package name, containing tag-to-version pointers | Detecting and reporting tag movement |
| npm version record | Package name plus exact version | Exact package inspection and execution preflight |
| npm tarball | SHA-512 integrity digest | `cache import` and artifact library functions only |
| Backend cache | Backend-defined | Delegated npm, bunx, and pnpm execution |

The tarball and backend caches are separate. `gitx exec` does not extract or run
from `npm/tarballs`; the selected package manager still materializes the
dependency graph in its own format.

### Metadata semantics

An explicit exact npm version can be served from its version record without a
registry request. Omitted versions and dist-tags are mutable, so GitX refreshes
registry metadata and updates their tag pointers. A tag pointer never becomes
the identity of an artifact: execution is delegated with the exact version it
resolved to.

The raw metadata and version JSON files are resolution caches, not verified
artifact content. They are keyed by package/version rather than by a digest.

### Verified tarball import

```bash
gitx cache import cowsay@1.6.0
gitx cache import cowsay@1.6.0 --from npm
gitx cache import cowsay@1.6.0 --from all
```

The command resolves an exact package version and requires npm SHA-512 SRI
metadata. It derives the target filename from that digest, verifies bytes
before copying, stages the copy under a temporary name, and renames it into the
GitX tarball store. Existing target files are re-verified. External caches are
read only; GitX does not move or delete their contents.

Import-source behavior is intentionally narrow:

| `--from` | Behavior |
| --- | --- |
| `auto` | Tries npm only and stops after the first verified import |
| `npm` | Checks npm `_cacache` locations for the original tarball bytes |
| `bun` | Reports discovered unpacked cache directories; does not import |
| `pnpm` | Reports discovered content-addressed stores; does not import |
| `all` | Tries npm, then reports bun and pnpm diagnostics |

The npm lookup checks GitX's npm backend cache, `npm_config_cache`,
`$HOME/.npm`, and the result of `npm config get cache`, without duplicating
identical paths. It maps the expected SHA-512 digest to npm's
`_cacache/content-v2/sha512/...` path and imports only a matching file.

Bun and pnpm are not tarball import sources today because their discovered
stores contain unpacked/content-addressed package files rather than the
original npm `.tgz`. Supporting them requires a separate materialized-package
identity and verification design.

### Artifact download primitive

`artifact::ensure_tarball` is implemented and tested. Given a URL and expected
SHA-512 SRI value, it:

1. reuses and verifies an existing digest-keyed tarball;
2. otherwise streams a download to a uniquely named temporary file;
3. verifies the complete partial file; and
4. persists it without overwriting a concurrent winner, which is independently
   verified before reuse.

No CLI command calls this function yet. There is no `gitx cache fetch`, origin
read-through, or CDN read-through path.

### Git snapshots

Git snapshots are grouped by SHA-256 remote identity and full commit hash. A
snapshot counts as complete only when its directory and adjacent `.complete`
record both exist. Fetch work happens in a temporary directory and is renamed
into place after checkout and commit verification.

This gives stable lookup identity, not filesystem immutability. Snapshot files
remain writable, and repo tools execute directly from the cached directory.
Protecting the cache with read-only snapshots or disposable run workspaces is
future work.

### Cache commands

```bash
gitx cache list
gitx cache remove <key>
gitx cache clear
```

`list` reports counts for npm metadata, tags, exact versions, tarballs, backend
entries, and completed Git snapshots. `remove` can remove package metadata and
version records, a remote's Git snapshots, or snapshots matching a commit
prefix. It does not map a package name back to and remove digest-keyed tarballs.
`clear` deletes the entire selected cache root, but refuses filesystem roots,
the user's home directory, the current directory, ancestors of the current
directory, symlink roots, and directories without the GitX cache marker.
Regular cache initialization also refuses to claim a non-empty directory that
does not already look like a GitX cache.

## Current guarantees and limits

Implemented guarantees:

- npm tarball import accepts SHA-512 SRI only and verifies the complete file;
- verified tarballs use digest-derived names;
- imports leave package-manager caches untouched;
- incomplete or malformed Git snapshots are not treated as cache hits and are
  replaced after a verified refetch;
- named Git ref fallback detects movement between resolution and fetch; and
- Git commit directories are namespaced by a collision-resistant SHA-256 remote key.

Not guaranteed:

- cache files are not made read-only;
- `gitx exec` does not consume or scan verified tarballs;
- registry metadata is not signed or pinned locally;
- there is no cache quota, garbage collection, access-time policy, or
  cross-process locking protocol;
- scan reports and bin mappings are not cached as independent objects; and
- private registry and private-repository cache policy is not implemented.

## Proposed CDN

Everything below this heading is a design, not current CLI behavior.

The CDN should remain an optional acceleration layer for public immutable
artifacts. GitX must still work from local caches and origins, and private
sources should bypass the service unless a user deliberately configures a
trusted private deployment.

### Target lookup order

```text
GitX local verified artifact
  -> importable local package-manager cache
  -> optional GitX CDN
  -> origin registry or Git host
```

Every path must end in local verification. A cache hit changes where bytes
came from, never the identity or validation required before use.

### Artifact and index keys

| Object | Proposed identity | Mutability |
| --- | --- | --- |
| npm tarball | npm SHA-512 integrity digest | Immutable |
| Git archive | Source identity plus resolved commit and a verified archive digest | Immutable |
| Bin mapping | Artifact digest plus parser/schema version | Immutable |
| Scan report | Artifact digest plus scanner/ruleset version | Immutable |
| npm dist-tag/range result | Package spec to exact version | Short-lived index |
| Git branch/tag result | Remote and ref to exact commit | Short-lived index |

Mutable names are resolver indexes, never artifact keys. CDN objects for a
given immutable key must be write-once. Rule or parser changes create new
versioned derived objects instead of rewriting prior results.

Git commit identity alone does not verify arbitrary archive bytes. A Git CDN
design must bind the archive digest to the resolved repository/commit through
origin verification or trusted signed metadata, then verify the archive digest
again in the CLI.

### API sketch

```text
GET /v1/resolve/npm/{package}?spec=latest
GET /v1/artifacts/npm/sha512/{hex}.tgz
GET /v1/resolve/git/github/{owner}/{repo}?ref=main
GET /v1/artifacts/git/github/{owner}/{repo}/{commit}.tar.zst
GET /v1/bins/{artifact-digest}?schema=1
GET /v1/scans/{artifact-digest}?ruleset=1
```

Artifact responses should carry source identity, resolved version or commit,
artifact digest, object age, and any derived-object digests. Resolution and
scan metadata need signatures before the CLI can elevate them above advisory
data. Unsigned remote scan results may inform the display but must not weaken a
local deny decision.

### Target npm flow

```text
resolve package spec to exact version and expected integrity
  -> verify/reuse GitX tarball
  -> attempt verified local-cache import
  -> request CDN object by expected digest
  -> fall back to registry tarball
  -> verify SHA-512 locally
  -> extract into a digest-keyed package store
  -> scan files and metadata
  -> materialize bin shims
  -> execute without package-manager startup on cache hits
```

The expected integrity should normally come from origin metadata or trusted
signed resolution metadata. Receiving both the bytes and an unsigned expected
digest from the same CDN does not protect against cache compromise.

### Safety requirements

- Never represent a branch, tag, dist-tag, or range as immutable.
- Never execute an artifact before local digest verification.
- Never overwrite a different object under an existing digest key.
- Never let a remote scan result reduce local policy without a configured trust
  relationship.
- Never upload source, credentials, package-manager cache contents, or private
  repository identity implicitly.
- Preserve an origin-only and offline/local path.
- Version derived metadata so scanner and parser changes are auditable.

### Delivery order

1. Wire a `cache fetch` command to the existing verified download primitive,
   with local npm import before origin fallback.
2. Extract verified tarballs, scan their files, and define a materialized-store
   key and bin mapping.
3. Execute npm bins from that local store and prove cache-hit behavior without
   a package-manager subprocess.
4. Add an opt-in CDN base URL and digest-addressed read-through client.
5. Deploy a minimal read-only npm artifact endpoint backed by immutable object
   storage.
6. Add signed, versioned resolution and scan metadata only after artifact
   delivery and local verification are stable.

This order makes the local artifact contract useful and testable before a
remote service depends on it.
