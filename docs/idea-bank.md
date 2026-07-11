# Deferred Idea Bank

> Parking lot only. Nothing here is scheduled, promised, or implemented merely because it is listed. See the [current roadmap](phase-1-roadmap.md) for active work.

The core repo runner, agent-readable inspection, package compatibility, and cache direction are documented elsewhere; this file holds options that still need product evidence or design decisions.

## Promotion Rule

Move an idea into the roadmap only when it has:

- a concrete user problem that the current CLI does not solve;
- a security and compatibility boundary;
- testable acceptance criteria and a place in the milestone order.

## Candidate Capabilities

### Private Team Catalog

Let a team publish an approved set of private repo tools under short names, for example `engineering/deploy-helper@v2`. This needs an ownership, authentication, and policy model before command syntax is worth fixing.

### User And Team Trust Policies

Allow local or managed rules such as permitted hosts, required commit pins, denied permissions, and trusted signers. Keep this separate from the basic `allow`/`ask`/`deny` result until policy precedence and overrides are designed.

### Signed Release Policy

Verify signed tags or commits against configured trust roots. The open design work is key discovery, trust storage, rotation, and useful behavior when signatures are absent or unverifiable.

### Team Lockfile Or Approval Record

Record immutable repo commits or package integrities that a team has reviewed. Decide whether this is a portable lockfile, a local approval database, or part of a future policy service.

### Optional Registry Index

Add discoverability without making a central registry necessary for execution. Promotion depends on evidence that direct Git and npm discovery are insufficient.

### Lazy Workspace Materialization

Fetch or materialize only the paths needed to run a tool. This should wait until the simpler identity-keyed cache is measured and its bottlenecks are understood.

### Hosted Integrations

Possible integrations include a GitHub App that reports install risk, a shared catalog service, and precomputed signed scan reports. Each creates a service and trust boundary that the local CLI does not currently need.

### Broader Workspace Or Source-Control Experiments

Workspace sync, lazy repository mounts, or deeper VCS behavior may eventually grow from the cache layer. They are separate products unless the runner produces a specific need for them.

## Open Product Questions

- What useful access controls can GitX enforce consistently across Linux, macOS, and Windows?
- Should manifests ever permit install or build steps, and how would those steps be scanned and confirmed?
- How should GitX expose the gap between declared permissions and actual OS-level access?
- What private-repository UX works with existing credentials without GitX collecting secrets?
- Which facts make an approval reusable, and which source changes must invalidate it?

## Brand Fallbacks

The working name is GitX. Check naming and package/binary conflicts before public distribution. If a change is required, earlier alternatives were RepoX, RPX, RunRepo, GitRun, FetchRun, and RepoExec.

## Topics Documented Elsewhere

- The safer repo runner and agent inspection are the current product, not deferred ideas: see [Product Scope](product-scope.md).
- Package-runner behavior and local cache design live in [Runner Compatibility](runner-compatibility.md).
- A hosted immutable cache is explored in [Cache and CDN Design](cdn-cache.md).
- Demo and release work belongs in the [current roadmap](phase-1-roadmap.md), where it can have acceptance criteria.
