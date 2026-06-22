# MVP Timeline

This is scoped as a 6-week build, assuming part-time but consistent work. The goal is a credible CLI demo, not a complete ecosystem.

## Week 1 - Shape The CLI

Goals:

- Pick implementation stack.
- Define command syntax.
- Define `gitx.toml` manifest v0.
- Build argument parsing.
- Resolve local paths and GitHub shorthand syntax.

Deliverables:

- `gitx --help`.
- `gitx inspect <source>` stub.
- Manifest parser.
- Example repo fixture.

Recommended stack:

- Rust if prioritizing single-binary distribution and systems credibility.
- TypeScript/Bun if prioritizing speed of iteration and dogfooding the JS/NPX pain point.

## Week 2 - Repo Resolution And Fetching

Goals:

- Resolve `github:owner/repo@ref`.
- Resolve full Git URLs.
- Convert branches/tags to commit hashes.
- Create temp workspaces.
- Cache by commit hash.

Deliverables:

- `gitx inspect github:owner/repo@main` prints resolved commit.
- `gitx cache list` shows cached refs.
- Basic sparse or shallow clone support.

## Week 3 - Manifest Execution

Goals:

- Execute manifest entrypoints.
- Forward command arguments.
- Support at least one runtime well.
- Add first-run prompt.

Deliverables:

- `gitx run github:owner/repo@ref -- arg1 arg2` works.
- Clear errors for missing manifests and unsupported runtimes.
- Cache reuse works across repeated runs.

## Week 4 - Risk Summary

Goals:

- Make the inspect output useful.
- Add warnings for mutable refs, missing pins, huge repos, missing manifests, and suspicious entrypoints.
- Show requested permissions and env vars.

Deliverables:

- Human-readable risk card before run.
- `--yes` flag for explicit non-interactive execution.
- `--json` output for agents.

## Week 5 - Private Repos And Agent UX

Goals:

- Work with existing SSH Git auth.
- Support HTTPS remotes where possible.
- Make output stable enough for agents to parse.
- Add docs for agent usage.

Deliverables:

- Private repo smoke test.
- `gitx inspect --json` schema.
- Agent safety notes.

## Week 6 - Demo Polish

Goals:

- Improve docs.
- Add example tools.
- Record a clear demo path.
- Decide whether to continue into sandboxing, registry, or VCS features.

Deliverables:

- README demo.
- Small example GitX-compatible repo.
- Roadmap for v0.2.

## Post-MVP Directions

- Stronger sandboxing.
- Signed manifests and signed refs.
- Trust policies per user/team.
- Registry index for discoverability.
- GitHub App for install/risk summaries.
- Workspace sync integration.
- Private team tool catalogs.
- Deeper source-control experiments.
