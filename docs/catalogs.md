# Catalog search and compatibility

`gitx search` is a discovery command. It finds candidates and prints the next
action. It never installs, builds, opens a browser, or runs a selected result.

## Search order and latency

The order is deliberate. It is not a claim that every provider is faster than
the next one.

1. GitX reads its own local state first: managed installs, the selected
   project's `node_modules/.bin`, and exact-version package records.
2. It reads available device catalogs next. Up to four fixed package-manager
   commands run at once, each has a five-second limit, and none asks its
   package manager to refresh data. Results keep source order, not completion
   order.
3. An explicitly selected user catalog runs after the built-in device sources.
   It is never part of the default search.
4. Remote providers run in batches of four. Each request has a 15-second total
   timeout and a 10-second connect timeout.

The default sources begin with the available device sources, then use `npm`,
`bun`, `github`, `gitlab`, and `codeberg`. On an Arch-based Linux host, `aur`
joins the remote list. When Bun resolves to the public npm registry, GitX skips
the duplicate default Bun lookup and says why in a note.

Use `--source` to make the request exact. It is repeatable and accepts a
comma-separated list.

```bash
gitx search formatter
gitx search formatter --source installed,cache
gitx search formatter --source bun,github --details 1
gitx search formatter --source device --details 1
gitx search formatter --device
gitx search formatter --source gitlab --source codeberg --format json
gitx search formatter --offline
```

`--offline` keeps local sources and skips all remote providers. It refuses an
explicit remote provider, because silently changing a request would be worse
than failing it. `--device` is stricter: it expands only on-device sources and
cannot be combined with `--source`. `--source device` is the equivalent strict
local source group when it needs to appear beside another source. For example,
`--source device,github` keeps the device adapters local and makes GitHub the
only network source.

## Providers

| Source | What GitX searches | Next action |
| --- | --- | --- |
| `installed` | GitX-managed shims | `gitx exec` or `gitx run` at the recorded pin |
| `project` | `node_modules/.bin` under `--project` | The project binary path |
| `cache` | GitX's exact npm version records | `gitx inspect npm:…` |
| `device` | All available on-device sources below | Source-specific next action |
| `system` | One host-appropriate installed-package database: dpkg, pacman, RPM, Homebrew, or Chocolatey | Inspect with that manager |
| `apt` | Local APT package lists, without `apt update` | `sudo apt install package` |
| `pacman` | Local pacman sync databases, without `pacman -Sy` | `sudo pacman -S package` |
| `flatpak` | Installed Flatpak apps, plus cached remote summaries outside `--offline`, `--device`, and `--source device` | `flatpak run` or `flatpak install` |
| `snap` | Installed Snap packages | `snap info package` |
| `cargo` | `cargo install --list` records | The recorded binary, when known |
| `python` | Packages in the active `python3` or `python` environment | `python -m pip show package` |
| `npm-global` | npm's global package tree | `npm ls --global` |
| `npm` | The public npm registry | `gitx inspect npm:…` |
| `bun` | The registry selected for the query by Bun configuration | `bunx package@version -- <args>` |
| `github` | GitHub repository search | `gitx inspect github:owner/repo@branch` |
| `gitlab` | GitLab project search | `gitx inspect gitlab:group/project@branch` |
| `codeberg` | Codeberg repository search | `gitx inspect codeberg:owner/repo@branch` |
| `aur` | Arch User Repository search | The AUR package page |

Git itself has no global package catalog. GitX therefore searches forge APIs
for GitHub, GitLab, and Codeberg. A generic Git URL remains a direct source:
`gitx inspect https://host.example/team/tool.git@main`.

### Bun registries

For a `bun` search GitX selects the registry for the specific package query.
It reads global and project `.npmrc`, global and project `bunfig.toml`, then
`BUN_CONFIG_REGISTRY` or `NPM_CONFIG_REGISTRY`. A matching scoped registry such
as `@company:registry=…` wins for `@company/tool`.

GitX validates the endpoint but does not load, send, or display Bun registry
tokens. That keeps secrets out of the catalog path. It also means discovery may
not work for an authenticated private registry. The printed `bunx` command
uses the user's normal Bun configuration, where Bun owns authentication.

### AUR and platform catalogs

The AUR is automatically searched only when `/etc/os-release` identifies an
Arch-based distribution. An explicit `--source aur` elsewhere still shows
matches, marked incompatible, so a user can inspect them without pretending
they will install locally. GitX does not build `PKGBUILD`s or invoke an AUR
helper. The next action opens the package page for review.

`apt` and `pacman` read metadata already present on disk. They do not refresh a
repository index. Outside strict local modes, `flatpak` also asks for its cached
remote summary with `flatpak remote-ls --cached`. Flatpak describes that flag as
a cache preference, not an absolute no-network guarantee, so `--offline`,
`--device`, and `--source device` show installed Flatpak apps only. These are
catalog records, not GitX-managed installs.

The system source is selected from the host and available executable. Linux
uses dpkg, pacman, or RPM when found. macOS and Linux can use Homebrew, and
Windows can use Chocolatey when it is installed. Snap and Flatpak are separate
because their origin remote matters to users.

GitX can import a verified npm tarball from npm's content cache with `gitx
cache import`. Bun and pnpm caches normally store unpacked content rather than
a stable, searchable package index, so GitX does not pretend that a filename
scan gives trustworthy package name and version metadata. Use a file-backed
user catalog when another local cache has a reliable index.

### User catalogs

`--catalog <id>` connects a user-owned catalog. It is explicit by design. A
catalog definition lives at `$XDG_CONFIG_HOME/gitx/catalogs.toml`, falling back
to `~/.config/gitx/catalogs.toml`. Set `GITX_CATALOG_CONFIG` to use a different
file. `--catalog` is repeatable, accepts commas, and `--catalog all` selects
every definition.

There are two kinds. A `file` catalog reads a local JSON or JSON Lines
snapshot. A `command` catalog runs a configured argument array and reads the
same JSON contract from standard output. GitX never invokes a shell for either
the configured command or its action. The command form is useful for a private
catalog client, a Nix index, an internal forge, or a package manager that GitX
does not yet know about. It has the same five-second local command limit as a
built-in device adapter. File snapshots and command output are capped at 16 MiB.

```toml
version = 1

[[catalog]]
id = "company-tools"
label = "Company tool snapshot"
kind = "file"
path = "company-tools.json"
format = "json"
origin = "company mirror snapshot"
action = ["toolctl", "install", "{name}"]

[[catalog]]
id = "private-index"
kind = "command"
command = ["company-catalog", "search", "--json", "{query}"]
format = "json"
origin = "company catalog client"
action = ["toolctl", "install", "{name}@{version}"]
```

Paths are relative to `catalogs.toml`. The data is either a JSON array or one
JSON object per line for `jsonl`:

```json
[
  {
    "name": "company-lint",
    "version": "4.2.0",
    "description": "Internal lint wrapper",
    "homepage": "https://tools.example.test/company-lint",
    "origin": "mirror revision 0123456",
    "information": { "Owner": "Developer Experience" }
  }
]
```

`name` is required. `version`, `description`, `homepage`, `origin`,
`updatedAt`, and `information` are optional. The `action` array is required on
the catalog definition and supports `{query}`, `{name}`, and `{version}`.
An action that uses `{version}` requires a version on the matching record.

`--offline`, `--device`, and `--source device` accept file catalogs only. GitX
blocks command catalogs in those modes because a child command could contact a
network even when its output looks local. A file snapshot is the way to get a
strict, reproducible device-only view of a private catalog.

## Candidate information

Every result has a source, origin, name, optional version, description,
compatibility state, provider facts, and a next action. `Source` names the
adapter. `Origin` identifies the registry, remote, database, cache, or user
snapshot that supplied that record. `--details <number>` shows the whole record.
`--format json` returns the same fields with `schemaVersion: 2`. Version 2
adds required `origin` data to every candidate.

Compatibility has three states:

- `compatible` means GitX has a concrete reason that the candidate fits this
  host, such as an available project binary or an AUR package on Arch Linux.
- `incompatible` means the provider or exact package metadata rules it out.
- `unknown` means GitX does not have enough evidence yet. It does not guess.

Package registry search results are usually `unknown` because search results do
not contain complete package metadata. `gitx inspect npm:package@version` and
`gitx exec package@version` resolve the exact package. They read npm `os` and
`cpu` constraints, show platform and runtime requirements, and block an
incompatible package before execution. Runtime engine ranges are shown, not yet
checked against the installed runtime's exact version.

Git-host results remain `unknown` until GitX resolves the repository and reads
its `gitx.toml`. Repository descriptions, stars, forks, language, archive
status, homepage, default branch, and update timestamp are discovery facts,
not trust signals.

## Update checks

`gitx update <tool>` handles one installed tool at a time. That is intentional.
It must preserve the record and shim as one recoverable change, so parallel
updates would add contention without making one update safer or clearer.

The sequence is:

1. Read and validate the existing record and shim.
2. Re-resolve the original Git or npm source.
3. Compare pin, runtime, integrity, description, homepage, license, entry
   point, binary names, platform constraints, runtime requirements, and manifest
   metadata.
4. Print the exact diff.
5. With `--check`, stop before writes. Without it, atomically replace the shim
   only if the pin changed. Metadata-only changes replace only the record.

```bash
gitx update prettier --check
gitx update repo-tool
```

## Next providers

The provider type is built for more adapters, but GitX does not claim that it
already searches every ecosystem. The next additions should be grouped by
action and host support, then tested with fixed provider responses:

1. Finish native local catalog coverage: dnf/zypper/apk on Linux, winget and
   Scoop on Windows, and richer Homebrew formula and cask metadata.
2. Add language registries: crates.io, PyPI, RubyGems, Go modules, and npm
   package-manager global stores where their local formats are stable.
3. Add direct private-forge and registry adapters with an authentication
   handoff that never copies credentials into GitX records or JSON output.

Each adapter needs a clear next action, a version and description contract,
bounded requests, offline behavior, and a real compatibility rule. A catalog
entry that merely looks installable is not useful here.
