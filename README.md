# GitX

GitX is an idea for **NPX backed by Git repos instead of NPM packages**.

The first version should not replace Git. It should sit on top of Git and prove one narrow workflow:

```bash
gitx run github:owner/repo@v1.0.0
gitx run github:owner/repo#commit-sha -- --tool-flags
gitx inspect github:owner/repo@main
```

Git already gives us authorship, commit history, tags, private repo auth, signatures, and review culture. NPX gives us the convenient "run this tool now" experience. GitX is the missing layer between them: temporary, content-addressed, permission-aware execution from a repo.

## Core Thesis

Modern developer tools need a safer way to run small tools, scripts, and agent utilities without pretending every executable has to become a traditional package.

NPM/NPX is convenient, but it hides too much risk. Git is trustworthy and reviewable, but not convenient enough for ephemeral executable tools. GitX should combine Git's source-of-truth model with NPX's instant-use UX.

## What GitX Should Be

- A CLI for running tools directly from Git repositories.
- A temporary execution layer: fetch, run, cache, discard.
- Content-addressed by commit hash, not mutable package names alone.
- Manifest-driven, so tools declare entrypoints, runtime, permissions, and environment needs.
- Agent-readable, so AI coding agents can inspect the risk profile before running code.
- Git-compatible from day one.

## What GitX Should Not Be Yet

- A full Git replacement.
- A full NPM registry replacement.
- A package ecosystem before the runner is useful.
- A sandbox fantasy that claims perfect security in v0.
- A cloud platform before the local developer loop works.

## First Product Wedge

Build **a safer NPX for Git repos**.

The first MVP should answer this question:

> Can a developer safely run a tool from a Git repo with less setup than cloning it and more visibility than NPX gives today?

## Useful Docs

- [Product Scope](docs/product-scope.md)
- [MVP Timeline](docs/mvp-timeline.md)
- [Architecture Notes](docs/architecture.md)
- [Idea Bank](docs/idea-bank.md)
- [Example Manifest](examples/gitx.toml)
