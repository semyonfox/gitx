# GitX timing report, 2026-08-20

## Bottom line

GitX is not faster than the provider it calls. It should not be. It starts a
process, resolves source metadata, normalizes the result, and sometimes shows a
preflight card. A direct `npm`, `pnpm`, or host API call has less work to do.

On this machine, local catalog search stays close to the original tool. Remote
search usually adds a small wrapper cost, except where the remote service owns
the wait. Package execution adds about 0.16 seconds on a cold npm or pnpm run.
That is a fair price for a version pin, provenance, compatibility information,
and a preflight. It is not a reason to use GitX for a hot loop where a native
tool already has the exact answer.

One bug did show up during this test. The first Pnpm run left a `dlx` cache
entry pointing at the temporary benchmark store. GitX now sets both
`pnpm_config_cache_dir` and `pnpm_config_store_dir` below its own cache. A CLI
test asserts both paths. Fresh isolated runs leave no Pnpm reference outside
the test cache.

## Test setup

| Item | Value |
| --- | --- |
| Host | Ubuntu 24.04.4, x86_64 |
| GitX | 0.2.0 release build |
| Git | 2.43.0 |
| Node | 24.16.0 |
| npm | 12.0.2 |
| Bun | 1.3.14 |
| pnpm | 11.21.0 |
| Python | 3.12.3 |
| Runs per row | 3 |
| Clock | `/usr/bin/time` elapsed time |
| Package case | `cowsay@1.6.0`, run as `cowsay gitx benchmark` |

The local and remote test script is [scripts/bench-smoke.sh](../scripts/bench-smoke.sh).
It makes a new temporary root for every run and moves that root to the desktop
trash on exit. Set `KEEP_BENCH_DIR=1` to retain the TSV. It never wipes the
normal npm, Bun, pnpm, APT, or GitX caches.

These are three-run arithmetic means from one host and one network connection.
The first request often pays DNS, TLS, CDN, or service-cache costs. Do not
generalize a few hundredths of a second into a universal winner.

## Local catalog search

The raw commands include equivalent filtering and package-detail work where
needed. Lower is better.

| Provider | GitX mean | Raw mean | Difference | Read |
| --- | ---: | ---: | ---: | --- |
| Installed system packages, `dpkg-query` | 0.020 s | 0.040 s | -0.020 s | Noise-sized difference. |
| APT cache | 0.377 s | 0.343 s | +0.034 s | GitX adds a little formatting and normalization work. |
| Snap | 0.080 s | 0.100 s | -0.020 s | Noise-sized difference. |
| Cargo installs | 0.030 s | 0.020 s | +0.010 s | Both are effectively instant here. |
| Python packages | 0.453 s | 0.443 s | +0.010 s | Provider work dominates. |
| Global npm packages | 0.517 s | 0.540 s | -0.023 s | Noise-sized difference. |

GitX is not doing anything magical in the rows where it wins by a few
milliseconds. Treat differences below roughly 50 ms in this run as noise.

## Remote search

Each row queries the same provider and the same text. The raw row calls the
provider endpoint or native CLI directly.

| Provider | GitX mean | Raw mean | Difference | Read |
| --- | ---: | ---: | ---: | --- |
| npm registry search | 0.297 s | 0.277 s | +0.020 s | Very close after the cold first request. |
| GitHub repositories | 0.213 s | 0.077 s | +0.136 s | GitX parses and normalizes cards after the API call. |
| GitLab projects | 0.543 s | 0.447 s | +0.096 s | Same shape as GitHub. |
| Codeberg repositories | 3.757 s | 3.690 s | +0.067 s | The server response dominates. |

The raw Bun registry endpoint returned HTTP 429 on all three attempts. GitX's
Bun-selected registry path averaged 0.130 s, but there is no valid raw
comparison for it. It is excluded from the table.

GitX starts no more than four remote provider workers at once and keeps the
final source order stable. An all-source search can therefore finish sooner
than running those sources serially, but that is a workflow benefit, not proof
that any individual provider call is faster.

## Package execution and cache state

For npm and pnpm, each cold sample used a new cache directory and each warm
sample reused the preceding cold cache. The Pnpm table uses the repaired cache
configuration, measured again after the fix. The native commands used the same
script-blocking policy as GitX.

| Backend | GitX cold | Native cold | Cold difference | GitX warm | Native warm | Warm difference |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| npm | 1.533 s | 1.370 s | +0.163 s | 0.500 s | 0.543 s | -0.043 s |
| pnpm | 1.497 s | 1.330 s | +0.167 s | 0.607 s | 0.563 s | +0.044 s |

The npm warm difference is too small to call a win. It changed across the
three samples. The useful conclusion is simpler: GitX adds roughly a tenth or
two tenths of a second to a cold one-shot run, and little enough to disappear
into normal warm-run variance.

Bun needs a separate note. GitX's metadata-cold path measured 0.183 s and its
warm path measured 0.053 s. A raw Bun shared-cache warm path measured 0.057 s.
The package was already available to Bun's shared global cache, so the GitX
"cold" result was not a true download-from-zero result. Bun documents its
global cache and `bunx` behavior at [Bun's cache guide](https://bun.com/docs/pm/global-cache) and [the bunx guide](https://bun.com/docs/pm/bunx). A clean user or container is needed before making a cold-cache claim for Bun.

## What was fixed during the run

Pnpm's `dlx` path has its own cache directory. The old GitX command set an npm
cache variable, which did not isolate that directory. The repaired command now
sets:

```text
pnpm_config_cache_dir=<gitx-cache>/npm/backend-cache
pnpm_config_store_dir=<gitx-cache>/npm/backend-cache/pnpm-store
```

The focused integration test is
`pnpm_execution_keeps_dlx_cache_and_store_inside_gitx_cache`. It captures the
environment of a fake Pnpm executable and verifies both paths. The full test
suite also passes after the change.

The earlier faulty test created one 188 KiB Pnpm `dlx` entry outside the test
root. I moved only that known broken test entry to the desktop trash. I did not
clear the ordinary Bun or Pnpm caches. No sudo command was used.

## What this benchmark does not claim

- Flatpak, Pacman, and AUR were unavailable on this Ubuntu host, so they were
  skipped rather than simulated.
- A raw Git clone is not compared with `gitx run`. A clone only fetches code;
  GitX also resolves a manifest, a runtime, and execution policy. That would
  be a false comparison.
- Public API timings change with network route, rate limits, and service
  caching. Re-run the script on the target machine before choosing a default
  backend based on a few milliseconds.
- This does not prove that Bun cache isolation is complete. It proves that the
  normal shared-cache path stayed fast on this machine.

## Practical recommendation

Use the original tool when you already know the source and need the lowest
single-command latency. Use GitX when discovery, provenance, update diffs,
local-only search, compatibility information, or preflight checks matter. That
is the honest boundary between the two.
