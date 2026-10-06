# cargo-impact

Find out whether a Rust API change actually breaks downstream code. An embedded
`cargo-semver-checks` gate decides when to spend time discovering and compiling
consumers. A semver warning alone never fails the job.

This is a standalone Cargo workspace. It has no GPUI dependencies and can be
copied out of this repository. The initial implementation deliberately uses a
single bounded worker; the library API also works independently of the CLI.

## Try it

Install from this checkout with Rust 1.96 or newer:

```sh
cargo install --locked --path tooling/cargo-impact
```

Build workers require a Unix host (Linux or macOS). Windows workers return an
unsupported-host error rather than running without process-tree termination.

Compare two source directories, using Docker for all Cargo operations:

```sh
cargo impact check --library your-crate \
  --baseline ../base --candidate ../candidate \
  --downstream app=../app --no-discovery
```

For a trusted local experiment without Docker, add `--local --allow-local`.
`--force` bypasses the API gate when investigating behavior, macros, feature
changes, or other changes outside the checker's API model. Downstreams still
receive the same recipe and revision on both sides of the comparison.

Reports are written to `impact-report/report.json` and `report.md`, including
fatal configuration, preparation, or gate errors. The JSON includes fingerprints, selected
package IDs, recipes, immutable downstream revisions, full rustc diagnostic
trees, suggestions, macro expansions, byte/line/column ranges, and source
snippets. Markdown shows the useful errors and links to tested source commits.
Each diagnostic retains its Cargo target and package origin (downstream,
injected library, or external dependency). Source links use verified original
checkout files, including nested workspace paths; generated and external files
remain unlinked. A library compilation failure is inconclusive. Errors in a
transitive consumer can still establish impact, with their origin shown explicitly.

| Result | Evidence |
| --- | --- |
| `compatible` | Both builds succeeded and compiled the injected library. |
| `regression` | Baseline succeeded; candidate produced compiler errors. |
| `pre_existing_failure` | Baseline already had compiler errors; inconclusive. |
| `harness_failure` | Preparation, resolution, environment, timeout, ICE, or evidence limits prevented a reliable comparison. |
| `not_exercised` | The consuming package or feature did not compile the requested library. |

Exit status is **1** for proven regressions, **2** for harness/discovery failures,
and **0** otherwise. Coverage notes and `not_exercised` results matter: a green
job establishes compatibility only for the successful experiments it records.

## Set up CI and the comment bot

Run this in the library repository:

```sh
cargo impact init --library your-crate
```

This writes `impact.toml`, two GitHub Actions workflows, and a versioned copy of
the scanner in `.github/cargo-impact`. It checks every destination first and
refuses existing files. The workflows install that copy with `--locked`; no
published crate, hosted service, or GitHub App registration is required. Add
`.cargo-impact/` and `impact-report/` to the library repository's `.gitignore`.
The vendored composite action pins the scanner's Rust toolchain and saves its
binary cache immediately, so a later regression failure still preserves the
installation for the next attempt. Its key includes source, embedded templates,
lockfile, toolchain, host OS, and architecture.

The generated workflows pin official Actions to reviewed release commits and
use their Node.js 24 runtimes. They target GitHub.com and current hosted runners;
self-hosted runners need Actions Runner 2.327.1 or newer. GitHub Enterprise
Server requires an alternative artifact transport before using the comment bot.

The PR workflow produces an artifact and a job summary. The comment workflow
accepts exactly `@cargo-impact check` or `/cargo-impact check` on an open PR.
The slash command avoids mentioning a GitHub account. Before reacting with 👀,
it queries the commenter's **current write, maintain, or admin permission**;
author-association fields and account badges do not grant access. Enable the
repository's Actions setting that permits the workflow's requested comment
permissions. The comment workflow becomes available on the default branch.

Authorization, builds, and reporting run in separate jobs. The control jobs use
trusted default-branch code; the worker installs the scanner and reads recipes
from the PR's pinned baseline in the PR workflow. Comment jobs pass the trusted
default-branch scanner as an immutable same-run artifact to the worker and
reporter; their recipes still come from the pinned baseline. Candidate code runs in Docker without API tokens
or checkout credentials. The reporter reads JSON as data and checks the current
PR head again before commenting. If the PR moved, it keeps the artifact and
does not post stale results. Comments neutralize source-controlled mentions,
HTML, and Markdown delimiters. Cache keys are scoped to untrusted PR work.

GitHub gives `issue_comment` jobs read-only cache tokens. The default-branch
push job warms the scanner cache, and the bot's artifact handoff avoids repeated
installation even on a cold run. PR/dispatch jobs save incremental work caches;
comment jobs can restore available caches but do not write them. For persistent
incremental retries, use the PR workflow or the CLI's retained work directory.

This implements an Actions bot. There is no webhook server or GitHub App
installation flow yet. A service integrating `bot::plan` must verify webhook
signatures before supplying events; the CLI `bot` command requires an Actions
event environment. No issue is created automatically.

## Discovery and other forges

Default discovery uses crates.io reverse dependencies. It follows repositories
on GitHub, GitLab, Codeberg/Gitea, and arbitrary HTTPS Git servers. GitHub code
search is an additional opt-in provider:

```toml
library = "your-crate"

[discovery]
crates_io = true
github = true
max_pages = 5
max_repositories = 20
```

Supply `GITHUB_TOKEN` or `GH_TOKEN` for GitHub search. `GITHUB_API_URL` supports
enterprise API roots. Tokens stay in the HTTP control plane. Use
`cargo impact discover --library your-crate --github` to inspect candidates
without building them.

Discovery is explicitly incomplete. crates.io covers published direct
dependents with repository metadata. GitHub search covers indexed default
branches, has a 1,000-hit cap, and can return incomplete results and false
positives. Page failures retain earlier candidates and appear in coverage
notes. Neither provider enumerates every unpublished or transitive user.
Cargo resolution and compiler artifacts verify that a candidate really
exercises the requested source. Add important consumers explicitly:

```toml
[[downstreams]]
name = "important-app"
manifest = "app/Cargo.toml"
[downstreams.source]
kind = "git"
url = "https://gitlab.com/team/group/app"
revision = "main"
forge = "git_lab"
```

Git branches are resolved to immutable commits per experiment. Private Git
repositories, submodules, and Git LFS materialization are not supported by the
anonymous checkout adapter. Local sources use `kind = "local"` and `path`.
Use full commit IDs when pinning revisions; abbreviated IDs are rejected with
an actionable error. Checkouts use shallow, filtered clones and atomic staging
so interrupted downloads can be retried. Safe internal dangling symlinks are
preserved; unsafe or unprovable links report their repository path.
Paths resolve relative to `impact.toml`. Use the containing workspace as the
source when a package has sibling path dependencies; `manifest` chooses the
consumer within it.

`Discover` is the provider boundary; implementations return normalized
`Repository` identities and evidence. Forge-specific URL/link behavior is
independent of discovery and execution. New providers can be composed with
`Discovery::merge`; embeddings supply them lazily through
`analyze_with_discovery`. GitHub is currently the only comment-bot adapter.

## Fix harness errors with recipes

The default recipe uses `rust:1.99.0-bookworm`. Its Cargo commands run with a read-only
container root, dropped capabilities, process/memory/CPU limits, and no build
network. Dependency fetching is a separate network-enabled step. Pin an image
digest for reproducible production runs and use a custom image for system
libraries or additional Rust targets.

An override is a **complete recipe**, selected by the reported downstream name
on the next run. Its omitted fields take defaults rather than inheriting the
global recipe. Features, package selection, and target are preserved equally
on both sides:

```toml
[overrides."important-app"]
features = ["use-your-crate"]
packages = ["app"]
no_default_features = false
[overrides."important-app".runner]
kind = "docker"
image = "your-build-image:fixed-version"
```

For local Nix environments:

```toml
[overrides."important-app"]
packages = ["app"]
[overrides."important-app".runner]
kind = "nix"
file = "recipes/app.nix"
attribute = ""
```

```nix
# recipes/app.nix; pin the nixpkgs supplied through NIX_PATH for repeatability.
{ pkgs ? import <nixpkgs> {} }:
pkgs.mkShell {
  packages = [ pkgs.cargo pkgs.rustc pkgs.pkg-config ];
  buildInputs = [ pkgs.openssl ];
}
```

This executes `nix develop --file ... --command cargo ...`. Nix supplies an
environment, **not process isolation**; it and `kind = "local"` require
`--allow-local` for trusted inputs. The supplied CI workflows retain Docker
isolation and do not enable host recipes automatically.

## Reuse builds and reduce output

Keep the same `--work-dir` (default `.cargo-impact`) between attempts. It stores
anonymous checkouts, Cargo downloads, separate rustdoc outputs, per-consumer
targets, and Boxington storage. Baseline and candidate share a consumer's target
directory. Identical copied files preserve timestamps; changed/deleted files
refresh the source snapshot. Cargo incrementality and fingerprints decide which
artifacts remain valid. A work-directory lock prevents overlapping scans.

Set `driver = "boxington"` in a recipe to run downstream checks through `mbx`.
Install mbx in the chosen local/Nix environment or Docker image. Rustdoc and
metadata continue through Cargo. Persistent `MBX_CACHE_DIR` and `MBX_SHIMS_DIR`
are provided; quiet output and target-view settings favor repeated harness
runs. Different consumers keep separate Cargo targets to prevent stale results
when repositories share package names. Boxington can share matching artifacts
across those targets.

Both output streams are drained continuously. Cargo progress and artifact JSON
are removed from the human log while structured diagnostics are retained after
the log fills. Each stream keeps a 256 KiB tail; diagnostic evidence is bounded
to 8 MiB / 512 messages per stream. Exceeding evidence limits is inconclusive,
not a successful scan. Unix timeouts terminate process groups and bounded pipe
draining prevents detached descendants from holding a scan open. Docker
containers are explicitly removed after completion or timeout.

## Design and verification

`engine` coordinates experiments; `gate` embeds the checker; `cargo` performs
graph/source injection; `source` owns snapshots and checkout revisions;
`runner` owns execution environments; `process` owns bounded I/O; `discovery`,
`forge`, and `http` own indexes and identities; `report` presents evidence;
`bot` owns permission checks and stale-result handling. Public results reuse
Cargo's diagnostic types rather than maintaining a lossy parallel schema.

Injection preserves aliases, inherited workspace dependencies, target tables,
features, optionality, and version constraints. Root patches reach transitive
registry/git sources; selected package IDs prove that Cargo used the copied
library. A changed candidate version is normalized to the baseline version in
the disposable copy **after** the gate, so a major bump can test source
compatibility against consumers requiring the old version. This is recorded in
the report. Original source trees and lockfiles are never rewritten.

```sh
cd tooling/cargo-impact
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
# With actionlint installed:
actionlint examples/impact.yml examples/impact-comment.yml
```

Tests compile actual temporary Rust workspaces and use scripted local HTTP
servers for provider/bot behavior. They exercise the embedded gate, exact
compiler spans, unused API removal, optional features, inherited/renamed/target
dependencies, transitive packages, baseline failures, build-script failures,
missing std, timeouts, retry recipes, cache reuse, source immutability, provider
pagination/rate limits, authorization, stale heads, output neutralization, and
self-contained setup. A local Git fixture verifies patching a transitive Git
dependency after graph resolution. Local Cargo and a Nix shell were exercised during
development on macOS. Fork-contained Actions runs exercised Docker, the embedded
gate, a real GPUI Refineable API change, exact E0624 spans, E0463 environment
failures, and the comment bot's permission check and eyes reaction. An anonymous
Git checkout of that fork verified a nested consumer's commit-pinned source link.
Boxington live execution and alternate-registry patching need broader coverage.

The gate checks the configured feature/target selection, not every possible
configuration or runtime behavior. Builds use `cargo check --all-targets`, not
execution of downstream tests. Additional runner modes, workers, forge indexes,
and a hosted App can extend these boundaries without changing the comparison
model.

Inspired by [Rust Crater](https://github.com/rust-lang/crater), using the
[cargo-semver-checks library](https://docs.rs/cargo-semver-checks/0.51.0/cargo_semver_checks/)
and optional [mr boxington](https://mr-boxington.jdx.dev/configuration).
Apache-2.0; see [LICENSE.md](LICENSE.md).
