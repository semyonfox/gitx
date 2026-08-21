# GitX CLI guide

GitX finds tools from the machine, package registries, and Git hosts. It can
inspect a source, show where it came from, pin a version, and run it after a
preflight check. It does not take over another package manager. npm, Bun, and
pnpm still do their own installs and execution.

The fastest way to get oriented is:

```bash
gitx doctor
gitx search ripgrep --device
gitx search formatter --source npm,github --details 1
gitx inspect npm:prettier@3.6.2
gitx exec prettier@3.6.2 --preview
gitx exec prettier@3.6.2 --yes -- --check README.md
```

`search`, `inspect`, and `--preview` do not execute a tool. `run` and `exec`
ask before execution unless `--yes` is present.

## Command tree

```text
gitx [--color auto|always|never] [-v...] [--progress auto|always|never]
├── inspect SOURCE [--format text|json]
├── run SOURCE [--yes] [--preview] [-- ARGS...]
├── exec [TARGET] [-p PACKAGE]... [--bin BIN]
│   [--backend auto|bun|npm|pnpm] [--yes]
│   [--allow-scripts] [--allow-dangerous-scripts] [--preview] [-- ARGS...]
├── search QUERY [--recommend] [-n 1..20]
│   [--source SOURCE[,SOURCE]...] [--project DIR]
│   [--select SELECTION | --interactive | --details RESULT]
│   [--offline | --device] [--catalog ID[,ID]...] [--format text|json]
├── install SOURCE [--as NAME]
├── update TOOL [--check]
├── list
├── remove TOOL
├── cache
│   ├── list
│   ├── clear
│   ├── remove KEY
│   └── import PACKAGE [--from auto|all|npm|pnpm|bun]
├── doctor
└── completion bash|elvish|fish|powershell|zsh
```

Every command also accepts `--help`. The root command accepts `--version`.

## Colour, progress, and terminal output

GitX uses colour for human-readable terminal output when standard output is a
terminal. It keeps pipes and redirected output plain by default.

| Global option | Meaning |
| --- | --- |
| `--color auto` | The default. Colour a real terminal unless `NO_COLOR` is set or `TERM=dumb`. |
| `--color always` | Emit ANSI colour codes even when output is piped. Useful for a pager that understands colour. |
| `--color never` | Emit plain text. Use this for scripts and logs. |

| Global option | Meaning |
| --- | --- |
| `-v`, `--verbose` | Write high-level resolution, policy, and backend hand-off steps to standard error. Repeat as `-vv` for source-level search detail. |
| `--progress auto` | The default. Draw GitX-owned transfer progress and Git activity on an interactive standard-error terminal. |
| `--progress always` | Force the animated terminal renderer. Useful when a terminal-aware wrapper hides TTY detection. It emits carriage-return control codes. |
| `--progress never` | Do not draw progress. Best for logs and strict stderr parsers. |

The text search view puts the package name, version, source, and compatibility
on the first line. Its wrapped description and concrete origin follow below.
`--format json` never includes ANSI colour codes. Verbose lines and progress
always use standard error, so JSON on standard output stays machine-readable.

GitX shows a real byte counter and bar only for a transfer it owns, currently
cache imports and the internal tarball downloader. Git fetches use the
animated `C---` marker because GitX cannot know their byte total. `exec` hands
download and execution to npm, Bun, or pnpm, keeps that backend's own output
visible, and does not invent a misleading byte count.

```text
$ gitx -v cache import cowsay@1.6.0 --from npm
:: Reading GitX cache
:: Importing a verified package tarball from the local cache
[##########--------] -C-- Importing cached package tarball  12 KiB / 22 KiB  54%
:: Importing cached package tarball complete: 22 KiB / 22 KiB
```

## Sources and the safe path to running one

Use a local directory, a GitX-style Git source, a generic Git URL, or an npm
package. Repo execution requires a `gitx.toml` manifest in the repository.

```bash
gitx inspect ./tool
gitx inspect github:owner/repo@v1.2.0
gitx inspect gitlab:group/project@main
gitx inspect codeberg:owner/repo@main
gitx inspect npm:prettier@3.6.2

gitx run ./tool -- --fix
gitx run github:owner/repo@v1.2.0 --preview
gitx run github:owner/repo@v1.2.0 --yes -- --check

gitx exec prettier@3.6.2 --preview
gitx exec prettier@3.6.2 --yes -- --check README.md
```

For a remote repo, GitX resolves the ref to a commit before it runs. A local
directory runs in place. `run` is for a repo-defined command. `exec` is for an
npm package binary.

## `inspect`

```text
gitx inspect SOURCE [--format text|json]
```

`SOURCE` is required. `--format text` is the default. `--format json` emits
the same inspection data for another program.

Use it before running an unfamiliar repository or package:

```bash
gitx inspect github:owner/repo@main
gitx inspect npm:eslint@9.30.0 --format json
```

## `run`

```text
gitx run SOURCE [--yes] [--preview] [-- ARGS...]
```

| Option | Meaning |
| --- | --- |
| `-y`, `--yes` | Skip the interactive confirmation. |
| `-n`, `--preview` | Resolve and show the preflight card. It never runs the tool. |
| `-- ARGS...` | Forward the rest of the command line to the manifest entrypoint. |

`run` requires a schema-1 `gitx.toml`. It selects the manifest runtime from
`node`, `bun`, `deno`, `python`, or `shell`. `shell` gets a high-risk warning.
The manifest permissions are declarations for the preflight. They are not an
OS sandbox.

## `exec`

```text
gitx exec [TARGET] [-p PACKAGE]... [--bin BIN]
  [--backend auto|bun|npm|pnpm] [--yes]
  [--allow-scripts] [--allow-dangerous-scripts] [--preview] [-- ARGS...]
```

Use `TARGET` when the package name is also the binary name:

```bash
gitx exec prettier@3.6.2 -- --check README.md
```

Use `--package` and `--bin` when they differ, or when a command needs more
than one package:

```bash
gitx exec --package @angular/cli@20.0.0 --bin ng -- new app
gitx exec ts-node -p typescript@5.8.3 -p ts-node@10.9.2 -- script.ts
```

| Option | Meaning |
| --- | --- |
| `TARGET` | Package spec, or the binary name when `--package` is present. |
| `-p`, `--package PACKAGE` | Repeatable package made available to the command. |
| `--bin BIN` | Explicit binary name. Requires at least one `--package` and cannot be used with `TARGET`. |
| `--backend auto|bun|npm|pnpm` | Select the backend after GitX resolves and scans the package. `auto` is the default. |
| `-y`, `--yes` | Skip the confirmation prompt. |
| `--allow-scripts` | Allow delegated install lifecycle scripts. The default blocks them where the backend supports that. |
| `--allow-dangerous-scripts` | Acknowledge a deny-level lifecycle finding. Requires `--allow-scripts`. |
| `-n`, `--preview` | Resolve and show the preflight card without execution. |
| `-- ARGS...` | Forward remaining arguments to the package binary. |

GitX resolves an exact package version and scans registry metadata. It checks
integrity metadata, install lifecycle scripts, suspicious script patterns,
large dependency counts, and native-looking package names. It does not yet
scan extracted files, the full dependency tree, or advisory databases.

### Backend choice

`auto` chooses a concrete backend before showing the preflight:

| Condition | Backend order |
| --- | --- |
| Default script policy | npm, then pnpm |
| `--allow-scripts` with one package | npm, Bun, then pnpm |
| More than one `--package` | npm, then pnpm |

An explicit backend never falls back to another one. Bun only supports one
explicit package. Bun execution requires `--allow-scripts` because `bunx` has
no equivalent per-run script-blocking switch.

## `search`

```text
gitx search QUERY [--recommend] [-n 1..20]
  [--source SOURCE[,SOURCE]...] [--project DIR]
  [--select SELECTION | --interactive | --details RESULT]
  [--offline | --device] [--catalog ID[,ID]...] [--format text|json]
```

Search is read-only. Selecting a result prints the next command; it does not
run it.

| Option | Meaning |
| --- | --- |
| `QUERY` | Required search text. |
| `--recommend` | Mark the strongest displayed result as a suggestion. |
| `-n`, `--limit 1..20` | Result limit. The default is 10. |
| `--source SOURCE` | Restrict the provider list. Repeat it or use commas. |
| `--project DIR` | Directory used when searching `node_modules/.bin`. The default is `.`. |
| `--select SELECTION` | Print next commands for result numbers such as `1`, `1 3`, or `1-4`. It cannot be combined with `--interactive` or `--details`. |
| `--interactive` | Prompt for result numbers on a terminal. It cannot be combined with `--select` or `--details`. |
| `--details RESULT` | Show the full card for one result. JSON already includes all fields. |
| `--offline` | Skip registry, Git-host, and AUR requests. An explicitly requested remote source fails rather than quietly changing the search. |
| `--device`, `--device-only` | Use only strict on-device sources. It cannot be combined with `--source`. |
| `--catalog ID` | Run a named user catalog from the catalog configuration. Repeat it or use commas. |
| `--format text|json` | Human-readable output or schema-version-2 JSON. |

The default search begins with device state, then queries npm, the Bun-selected
registry, GitHub, GitLab, Codeberg, and the AUR on Arch Linux. GitX runs at
most four device providers and four remote providers at once. It keeps the
display order stable by source priority, so parallel work does not scramble
the results. With `-v`, GitX names each remote source as it starts. With
`-vv`, it also reports source-level completion counts.

### Search sources

| Source | What it reads |
| --- | --- |
| `all` | The default source set. It cannot be combined with another source. |
| `device` | `installed`, `project`, `cache`, and every usable local platform source. It is the composable local-only source. |
| `npm` | The public npm registry. |
| `bun` | The registry selected from Bun and npm configuration. |
| `github`, `gitlab`, `codeberg` | Public repository searches on those hosts. |
| `aur` | Arch User Repository search, on Arch Linux. |
| `installed` | GitX-managed install records. |
| `cache` | GitX exact-version cache records. |
| `project` | `node_modules/.bin` in `--project`. |
| `system` | Installed system packages. |
| `apt`, `pacman` | Available local package-manager caches. |
| `flatpak`, `snap` | Installed Flatpak or Snap applications. |
| `cargo`, `python`, `npm-global` | Locally installed tools from those ecosystems. |

Examples:

```bash
gitx search formatter --device
gitx search formatter --source npm,github --details 1
gitx search formatter --offline --source installed,cache
gitx search formatter --source cache --select 1
gitx search formatter --catalog company-tools --format json
```

Platform sources only appear when their local tool or database is available.
On this Ubuntu host that means APT, Snap, Cargo, Python, and npm-global can
participate. Pacman, AUR, and Flatpak do not.

## `install`, `update`, `list`, and `remove`

```text
gitx install SOURCE [--as NAME]
gitx update TOOL [--check]
gitx list
gitx remove TOOL
```

`install` resolves mutable input now and writes a persistent GitX shim. npm
installs pin an exact version and integrity value. Git installs pin a commit.

| Command or option | Meaning |
| --- | --- |
| `gitx install SOURCE` | Create a managed shim. |
| `install --as NAME` | Use `NAME` for the shim instead of the inferred tool name. |
| `gitx update TOOL` | Resolve the original source again. It shows old and new pins and then writes only if something changed. |
| `update --check` | Show the same diff without changing the record or shim. |
| `gitx list` | Show managed shims. |
| `gitx remove TOOL` | Remove one GitX-managed shim and its record. |

Updates compare the pin, integrity, description, homepage, license, entry
point, binary names, platform rules, runtime requirements, and manifest
metadata. A metadata-only change updates the record without rewriting the
shim. A changed pin replaces the record and shim atomically.

Records live under `$GITX_HOME/installed` and shims under `$GITX_HOME/bin`.
The default `GITX_HOME` is `~/.gitx`.

## Cache commands

```text
gitx cache list
gitx cache clear
gitx cache remove KEY
gitx cache import PACKAGE [--from auto|all|npm|pnpm|bun]
```

| Command or option | Meaning |
| --- | --- |
| `cache list` | List GitX-owned artifacts. |
| `cache clear` | Remove the GitX local cache only. |
| `cache remove KEY` | Remove one GitX cache record, commit, package, or tool key. |
| `cache import PACKAGE` | Copy a verified package artifact from an existing manager cache into GitX's tarball store. |
| `--from auto` | Try npm first and stop after the first import. |
| `--from all` | Look in npm, Bun, and pnpm caches. |
| `--from npm|pnpm|bun` | Read only that manager cache. |

`GITX_CACHE_DIR` overrides the GitX cache path. Without it, GitX uses
`$XDG_CACHE_HOME/gitx` or `~/.cache/gitx`. `cache clear` does not wipe normal
npm, Bun, or pnpm caches. That boundary is deliberate.

Delegated npm and pnpm runs receive cache settings below the GitX cache. Bun
also receives a GitX cache path, but Bun may satisfy a package from its shared
global cache. Treat a Bun package benchmark as warm unless it runs in a clean
user or container.

## `doctor` and `completion`

```bash
gitx doctor
gitx completion bash > ~/.local/share/bash-completion/completions/gitx
gitx completion zsh > ~/.zfunc/_gitx
```

`doctor` reports which supported runtimes are on `PATH`. `completion` writes a
shell completion script to standard output. Supported shells are `bash`,
`elvish`, `fish`, `powershell`, and `zsh`.

## Current boundaries

GitX is usable for local testing and internal development. It is not a
sandbox. A manifest can describe permissions, but it cannot limit filesystem,
network, environment, or child-process access. Package execution remains a
delegation to the selected package manager. Windows-native shims, configured
private registries for package execution, range resolution, tarball file
scanning, and direct execution from GitX-owned artifacts are still unfinished.

For the implementation detail and provider notes, read [Package execution](runner-compatibility.md), [Catalog search](catalogs.md), and the [repository audit](repository-audit-2026-08-20.md).
