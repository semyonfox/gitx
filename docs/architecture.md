# Architecture Notes

## Conceptual Flow

```text
source string
  -> resolver
  -> exact commit
  -> fetch/cache layer
  -> manifest parser
  -> risk analyzer
  -> confirmation gate
  -> runtime executor
  -> cleanup/cache update
```

## Source Syntax

Initial syntax ideas:

```bash
gitx run github:owner/repo@v1.0.0
gitx run github:owner/repo#abc123
gitx run git@github.com:owner/repo.git@main
gitx run ./local-tool
```

Open question: whether `@` or `#` should represent commits. A clean rule might be:

- `@ref` for branch, tag, or named ref.
- `#sha` for exact commit.

## Manifest

Default manifest path:

```text
gitx.toml
```

Manifest responsibilities:

- Tool name.
- Version or manifest schema.
- Entrypoint.
- Runtime.
- Permission declarations.
- Environment variables.
- Optional install/build step.

See `examples/gitx.toml`.

## Fetching Strategy

Start simple:

- Use system Git for clone/fetch.
- Shallow clone when possible.
- Resolve exact commit before execution.
- Cache by host, owner, repo, and commit hash.

Later:

- Partial clone.
- Sparse checkout based on manifest paths.
- GitHub API optimization.
- Bare object cache shared across tools.

## Runtime Strategy

MVP should support one runtime first. Avoid supporting everything badly.

Possible first runtimes:

- `node`: natural fit for NPX replacement.
- `bun`: fast setup and direct TypeScript support.
- `python`: common scripting target.
- `shell`: useful but high-risk.

A practical first target could be Bun or Node, with shell support marked high-risk.

## Risk Analyzer v0

Risk analyzer does not need to be magical. It can start with concrete checks:

- Is the ref mutable?
- Is the repo pinned to a commit or signed tag?
- Does the manifest request network access?
- Does it request sensitive env vars?
- Does it write outside the temp workspace?
- Does it use a shell entrypoint?
- Is the repo very new or very large?
- Did the cached ref move since last run?

## Agent Output

`gitx inspect --json` should be stable and boring.

Useful fields:

- `source`
- `resolvedRemote`
- `resolvedCommit`
- `manifestPath`
- `entrypoint`
- `runtime`
- `permissions`
- `env`
- `warnings`
- `riskLevel`
- `requiresConfirmation`

## Security Position

GitX v0 should be transparent about what it does and does not protect.

It can provide:

- Better pre-run information.
- Commit pinning.
- Cache immutability.
- Permission declarations.
- Warnings for suspicious patterns.
- Optional wrappers for obvious file access constraints.

It cannot initially provide:

- Perfect containment.
- Guaranteed malware detection.
- Trust in arbitrary unreviewed code.
- Protection from a malicious runtime already installed locally.
