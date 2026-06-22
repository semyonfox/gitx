# Idea Bank

## Names

Working name: GitX.

Other possible names:

- RepoX
- RPX
- RunRepo
- GitRun
- FetchRun
- RepoExec

GitX is clear, but check naming conflicts before publishing.

## Product Ideas

### Safer Repo Runner

The main wedge. Run tools from Git repos with commit pinning, temporary execution, and visible risk.

### Private Team Tool Catalog

A team can maintain a list of approved private repo tools:

```bash
gitx catalog add engineering git@github.com:org/tools.git
gitx run engineering/deploy-helper@v2
```

### Agent-Safe Command Gate

Agents can call:

```bash
gitx inspect --json github:owner/repo@main
```

Then decide whether to continue, refuse, or ask the user.

### Repo-Native Publishing

Instead of publishing to NPM, a maintainer tags a release and ships a manifest. The repo is the package.

### Trust Policies

Local policy file:

```toml
[trust]
allow_hosts = ["github.com"]
require_pinned_commit = true
allow_network = false
```

### Signed Releases

Warn unless the selected tag or commit is signed by a trusted key.

### Lazy Workspace Mode

A longer-term bridge into the source-control idea: mount or materialize only the needed paths from a repo when a tool runs.

### Registry Index

Optional later layer for discoverability. Keep it separate from execution so GitX does not need a central registry to be useful.

## Hard Questions

- How much sandboxing is realistic cross-platform?
- Should GitX use system Git or libgit2?
- What is the first runtime?
- Should manifests allow install/build steps?
- How does the tool prevent a manifest from lying about permissions?
- What UX is strict enough for safety but not so noisy that users ignore it?
- How should private repo auth work without collecting secrets?
- Should there be a lockfile for teams?

## Good Demo Scenarios

### Public Tool

Run a tiny formatter or repo auditor directly from GitHub.

### Private Tool

Run a private team script using existing SSH auth.

### Agent Safety

Show an agent inspecting a command, noticing mutable `main` plus network plus env access, and asking before running.

### Cache Immutability

Run a tag, resolve it to a commit, run again from cache, then show that a moved branch triggers a fresh warning.
