# Repository audit, 2026-08-20

## Decision

GitX is in good shape for continued internal development. The command contract,
catalog lookup, update behavior, and package preflight have meaningful test
coverage. It is not ready to publish as a security boundary or a finished
cross-platform tool runner.

## What passed

| Check | Result |
| --- | --- |
| Formatting | `cargo fmt --all -- --check` passed |
| Linting | Strict Clippy passed with `-D warnings` |
| Tests | Unit and CLI integration tests passed |
| Release build | `cargo build --release --locked` passed |
| Package verification | `cargo package --allow-dirty --locked` packaged and rebuilt the crate |
| Dependency update check | `cargo update --dry-run` found no newer release compatible with Rust 1.88 |
| RustSec | `cargo-audit` found no advisories in 214 locked crates |

The direct dependencies and lockfile are current for the declared Rust 1.88
minimum. Cargo package verification warns that the non-publishable manifest has
no public documentation, homepage, or repository URL. That is correct for now:
no repository remote or distribution name has been chosen.

## Work completed in this pass

- Replaced stale `search --scope` and `update --preview` documentation with the
  new `--source` and `update --check` grammar. Tests reject the retired flags.
- Added a normalized catalog result type, host detection, and explicit
  `compatible`, `incompatible`, and `unknown` states.
- Added local-first search across managed installs, project bins, cache records,
  host package databases, APT and pacman caches, installed Flatpaks, Snap, Cargo, Python,
  npm global packages, npm, the Bun-configured registry, GitHub, GitLab,
  Codeberg, and the AUR on Arch-based Linux.
- Kept device lookup bounded to four worker threads at a time with a five-second
  command limit, and remote lookup bounded to four worker threads with a
  15-second request limit. Both keep stable source-priority output.
- Added `--device` for a strict on-device view, `--source device` for the same
  sources beside an explicit provider, and file-backed per-user catalogs for
  private snapshots. `--offline` and `--device` reject configured command
  catalogs because GitX cannot constrain their network behavior.
- Made `--details` and JSON include source, concrete origin, versions,
  descriptions, homepages, compatibility, runtime requirements, provider facts,
  and the next action.
- Added exact-package npm OS and CPU checks. Incompatible packages are visible
  in inspect and preview, then blocked before execution or managed install.
- Added update diffs for pin, integrity, description, homepage, license,
  entry point, binaries, platform requirements, and manifest metadata.
- Made metadata-only updates replace the record without needlessly rewriting a
  shim.
- Preserved Bun scope selection without loading or printing registry tokens.
  Search suggestions quote provider-controlled values before presenting a
  copyable shell command.
- Fixed Pnpm delegated execution so its `dlx` cache and package store stay
  under GitX's cache root. A CLI integration test captures the backend
  environment and asserts both paths.
- Added fixed-response tests for every remote catalog adapter and scoped Bun
  configuration. They do not depend on live provider responses.

See [Catalog Search](catalogs.md) for the exact provider and update behavior.

## Remaining release and security work

1. **Cached Git snapshots are writable.** GitX verifies a commit when it
   materializes the cache, but runs execute in that reusable directory. A tool
   can alter files that a later run sees. Run from a disposable work copy or a
   read-only materialization.
2. **There is no OS sandbox.** Manifest permissions influence warnings only.
   They do not restrict filesystem access, networking, environment variables,
   or child processes.
3. **Package execution is delegated.** npm, Bun, or pnpm still materialize the
   dependency graph. GitX validates metadata and lifecycle policy, but does not
   yet execute a GitX-owned verified package tree.
4. **Runtime engine ranges are not enforced.** npm `engines` values are shown,
   but GitX does not compare them with the installed Node or Bun version yet.
5. **Managed shims are Unix-only.** Windows-native shims and their tests remain
   unfinished.
6. **Catalog coverage is intentionally incomplete.** GitX reads Homebrew,
   APT, pacman, RPM, Chocolatey, Flatpak, Snap, Cargo, Python, and npm local
   state where available, but it does not yet cover dnf, zypper, apk, winget,
   Scoop, or language registries such as crates.io and PyPI. The next adapter
   order is in [the catalog plan](catalogs.md#next-providers).
7. **Repository and release provenance are missing.** The worktree still has
   untracked implementation files and no Git remote. The crate is marked
   `publish = false`; a distribution name, repository URL, changelog, and CI
   evidence are needed before a public release.
8. **Bun cold-cache behavior needs a clean environment test.** Bun may satisfy
   a package from its shared global cache even when GitX gives it a separate
   cache path. The current benchmark reports Bun only as a shared-cache warm
   path until a clean user or container proves the cold path.

## Recommendation

Keep building on this branch. The next technical change should isolate Git
execution from the reusable snapshot cache. In parallel, add platform-native
catalog adapters behind the same bounded provider contract, starting with the
package systems that match each host.
