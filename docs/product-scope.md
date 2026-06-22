# Product Scope

## One-Sentence Pitch

GitX is NPX for Git repos: run a tool from a repository temporarily, with explicit commit resolution, visible permissions, and a clear risk summary before execution.

## User Problems

### NPX Has Weak Trust Signals

Current NPX prompts mostly say that a package needs to be installed. They do not give enough context about who published it, what changed recently, what it can access, or whether it looks suspicious.

### Git Repos Are Too Heavy For Disposable Tools

If someone shares a useful script in a repo, running it usually means cloning the repo, reading setup docs, installing dependencies, and remembering to remove it later. That is too much friction for small tools.

### Agents Need Safer Execution Metadata

Coding agents increasingly run commands from docs, scripts, and instructions. If those commands call NPX or curl random install scripts, the agent often has no structured way to know whether it should stop and ask.

## MVP Users

- Developers who run one-off tools from GitHub.
- Builders who ship small CLI utilities but do not want NPM publishing friction.
- AI coding-agent users who want safer command execution.
- Teams with private utility repos.

## MVP Capabilities

### `gitx inspect`

Inspect a repo ref without running it.

Expected output:

- Resolved repo URL.
- Resolved commit hash.
- Manifest found or missing.
- Entrypoint.
- Runtime.
- Declared permissions.
- Requested environment variables.
- Basic risk warnings.

### `gitx run`

Run a repo-defined command in a temporary workspace.

Required behavior:

- Resolve `github:owner/repo@ref` or a full Git URL.
- Fetch only the resolved ref where practical.
- Read `gitx.toml`.
- Show a first-run confirmation prompt.
- Cache by commit hash.
- Execute the manifest entrypoint.
- Forward arguments after `--`.

### Cache Management

Commands:

```bash
gitx cache list
gitx cache clear
gitx cache remove <commit-or-tool>
```

Cache rules:

- Cache resolved repos by commit hash.
- Never treat a mutable branch as trusted just because it was seen before.
- If `main` moves, inspect again.

## Permission Model

MVP permissions should be honest and simple.

Useful permissions:

- `network`: tool expects network access.
- `read`: paths it needs to read.
- `write`: paths it needs to write.
- `env`: environment variables it requests.
- `subprocess`: whether it shells out to other commands.

The MVP does not need perfect OS sandboxing. It should start with visibility, warnings, and optional deny-by-default wrappers where realistic.

## Out Of Scope For MVP

- Replacing Git internals.
- Distributed registry hosting.
- Perfect sandbox isolation.
- GUI app.
- Team policy engine.
- Package name marketplace.
- Mobile support.
- Windows-first implementation, unless the chosen stack makes it cheap.

## Success Criteria

The MVP is useful if:

- A developer can run a public repo tool in under 30 seconds.
- The same command resolves to the same commit when pinned.
- The risk summary is more useful than NPX's current prompt.
- A private GitHub repo can work through existing Git auth.
- A coding agent can inspect the manifest and decide whether to ask the user.
