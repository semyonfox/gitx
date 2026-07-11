# Archived MVP Timeline

> Historical planning record. This was the repository's original six-week proposal, not a current schedule or status page. Use the [current roadmap](phase-1-roadmap.md) for active work.

## Original Assumption

The project was scoped as a part-time, six-week build aimed at a credible CLI demo rather than a complete ecosystem. The implementation language was still undecided, and package-runner compatibility was not yet part of the weekly plan.

## Proposed Sequence

| Week | Theme | Intended outcome |
| --- | --- | --- |
| 1 | Shape the CLI | Choose the stack; define commands and `gitx.toml`; parse local paths and GitHub shorthand; provide help, inspect scaffolding, and a fixture. |
| 2 | Resolve and fetch repositories | Resolve Git refs to commits, create temporary workspaces, cache by commit, and expose cache listing. |
| 3 | Execute manifests | Run one supported runtime, forward arguments, prompt on first use, and reuse cached content. |
| 4 | Summarize risk | Warn about mutable refs, missing manifests, suspicious entrypoints, permissions, and requested environment variables; add `--yes` and agent JSON. |
| 5 | Support private repos and agents | Reuse existing Git authentication, stabilize machine-readable output, smoke-test a private source, and document agent safety. |
| 6 | Polish the demo | Improve docs, provide a small example tool and repeatable demo, then decide the next product direction. |

## Why It Was Archived

The worktree now uses Rust and includes functionality that the proposal did not schedule, including npm-style execution, compatibility aliases, artifact/cache work, scanning, and persistent shims. The original week-by-week ordering therefore no longer describes the implementation or predicts delivery.

The proposal is kept only to preserve the project's planning history. It must not be used to infer which features are complete.

## Current Sources Of Truth

- [README](../README.md) for commands that exist in the worktree.
- [Current Roadmap](phase-1-roadmap.md) for ordered implementation work and exit criteria.
- [Product Scope](product-scope.md) for the product boundary.
- [Deferred Idea Bank](idea-bank.md) for unscheduled directions that once appeared under “post-MVP.”
