# Adversarial review and production gaps

This iteration was driven by a Sol review, followed by implementation, regression
tests and a second review. The scanner is still a prototype. It does not provide
Crater's ecosystem coverage or operational infrastructure.

## What changed

| Finding | Change and evidence |
| --- | --- |
| A build could mutate candidate, sibling or shared cache files | Narrow phase mounts, read-only library/consumer/registry/git inputs during compilation, separate targets and mutable caches, symlink checks and source drift detection. Fake-Docker mount tests and an actual-Docker integration test exercise this boundary. |
| A source Cargo configuration could substitute its compiler and return false success | Scanner-controlled rustc/rustdoc and empty wrapper settings override project configuration. Real Cargo fixtures exercise forced environment tables and scalar settings; diagnostics from mutated inputs lose their source links. |
| Consumers ran serially; individual commands each received a fresh timeout | Bounded parallel consumers, sequential pairs, pair and scan deadlines, supervised embedded API analysis, deadline-aware HTTP pagination. Barrier tests prove overlap without reversing a pair. |
| A cancellation discarded all useful results | Serialized progress callbacks and atomic authoritative JSON checkpoints. Partial reports retain completed results. A failed secondary view cannot discard saved compiler evidence. |
| Storage grew without a policy | Default 5 GiB managed-work budget, streaming source copies, storage watchdogs, explicit pruning under the scan lock, and narrower Actions cache tiers. |
| A gate-open run could be green with no exercised consumers | Configurable exercised minimum, explicit coverage result, inconclusive exit status and report/SARIF notices. The gate-closed case is separate. |
| Reports omitted highlighting and practical inspection | Rust fences and bold expressions in Markdown; exact Unicode-safe `<mark>` spans in offline HTML; search, classification filters, source navigation, deduplicated errors, macro callsites and full diagnostic trees in JSON. Browser checks exercised filtering, search, navigation and copying an issue draft. |
| Reports gave little help filing an issue | Local Markdown and TOML bundles, tested downstream revision, selected package/manifest, source/toolchain/image/lock fingerprints, GitHub/GitLab/Gitea issue forms and copy/download fallbacks. No issue is submitted automatically. |
| Replay looked more exact than the available data justified | Retained phase locks, patch URLs, normalized graphs, exact source/engine/compiler/image identities and clean selected targets. Verified replay pins the consumer commit, checks identities before compilation, records outcome drift and refuses incomplete/host/Nix evidence. |
| Bot reruns created noise or could publish against a moved head | Update the bot's own marked comment, ignore forged human markers, bounded history lookup and a second head check immediately before writing. Only reads retry; ambiguous writes do not. |
| A failed worker could leave the command unanswered | Comment workflows can export an explicit inconclusive report when no worker artifact is available. Authorization still checks current repository permission before the eyes reaction. |
| Setup had hidden prerequisites | Self-contained `init`, preserved `.gitignore` additions, bounded `doctor`, standalone scanner assets and offline rendering. Setup rejects symlinks inside the selected root and refuses existing installation files. |
| One obsolete registry package poisoned another valid package in the same repository | Separate package selections, normalized selection deduplication, experiment budget and explicit overlap notes. |

Completed views now publish as immutable content-addressed generations with an exact
artifact manifest. Root JSON remains the newest durable evidence; root aliases are
convenience views. `verify` checks complete generations or a separately identified
checkpoint. An interrupted alias update can be recovered by rendering the latest JSON.
Explicit report retention preserves current, corrupt, incomplete and user-owned files.
See [the evidence contract](evidence.md).

An independent second Sol review found seven concrete boundary bugs after the refactor.
The fixes read and parse the same verified bytes, remove replay-command diff markers,
reject replay destinations inside the input bundle, reject blocked setup ancestors before
writing, accept a bare report filename, normalize lifecycle interpretation in every view,
and label failed preparation as an attempt. Tests cover concurrent checkpoint replacement,
shell argument forwarding, retained-input protection, installation preflight and report
classification. `init` automatically embeds every source module and is compiled as a
fresh standalone installation in the fork's verification workflow.

A follow-up review exposed report/source overlap with managed cleanup. Both CLI commands
now resolve directory aliases and reject overlapping report/work/source trees before
writing. The embedding API checks known inputs before pruning and discovered local inputs
before worker dispatch. Sentinel tests cover selected replay targets, source-root output,
future paths, symlinks and configured local consumers. Linux also exposed an executable
test-fixture race; supervision tests now run their input script through the system shell.

## What the reference implementations teach

[Crater workers](https://github.com/rust-lang/crater/blob/master/src/runner/worker.rs)
record progress at each toolchain experiment and isolate preparation/execution.
[Its runner](https://github.com/rust-lang/crater/tree/master/src/runner) uses bounded
workers and coordinates disk cleanup with idle workers. We adopted bounded
pairs, serialized progress and isolation; an idle-barrier LRU cache remains to
be implemented. Our storage watchdog is not a filesystem quota.

[Crater's analyzer](https://github.com/rust-lang/crater/blob/master/src/report/analyzer.rs)
and [triage guide](https://github.com/rust-lang/crater/blob/master/docs/report-triage.md)
distinguish experiment failures, dependency fallout and known exceptional crates.
Matching error fingerprints in this scanner identify repeated compiler symptoms;
they do not prove that one dependency caused every failure. Resolved dependency paths
and clean confirmation are now recorded through Cargo graphs and verified replay.
A proven dependency-cause graph, infrastructure retry policy and reasoned suppressions
with expiry remain missing.

[cargo-audit's presenter](https://github.com/rustsec/rustsec/blob/main/cargo-audit/src/presenter.rs)
uses inverse dependency explanations and actionable solutions, while
[its SARIF output](https://github.com/rustsec/rustsec/blob/main/cargo-audit/src/sarif.rs)
illustrates structured results and stable fingerprints. This scanner now provides
actionable issue/replay bundles and SARIF, validated against the
[official OASIS schema](https://github.com/oasis-tcs/sarif-spec/blob/main/sarif-2.1/schema/sarif-schema-2.1.0.json).
The report shows the recorded path to the injected library and failing package, including
selected features. Index freshness and causal dependency triage still require more evidence.

## Work still needed

| Area | Current boundary | Next production step |
| --- | --- | --- |
| Discovery | crates.io published direct dependents and optional indexed GitHub code search; repository HEAD is built | Published archive/version adapter, index freshness, sampled/curated watchlists and transitive dependency coverage |
| Replay/storage | Typed experiment identity, acknowledged phase checkpoints, immutable report generations, retained locks/graphs and clean verified replay | Append-only pair event archive, verified shared dependency seeds, quota-backed volumes and idle-barrier LRU |
| Execution | `cargo check --all-targets` for one recipe; clean replay confirmation | Feature/target/MSRV matrices, build/test modes and retryable infrastructure errors |
| Resource bounds | Managed-work watchdog, Docker worker limits, supervised CLI semver helper | Hard disk quotas, Docker image/tmpfs accounting, cgroup/process peak telemetry and explicit OOM classification |
| Embedding | Optional semver helper; arbitrary discovery callback | Require an interruption-aware executor for untrusted production embeddings; inline analysis and arbitrary blocking callbacks are not forcibly supervised |
| Source contract | Compilation reads immutable sources; generated files should use Cargo `OUT_DIR` | Trusted preparation adapters for exceptional valid packages that generate into their source tree |
| Forges | HTTPS Git cloning; GitHub, GitLab and Gitea web link/form implementations; GitHub Actions bot | GitLab/Gitea search and permission/comment adapters, per-repository URL templates and custom/disabled tracker support |
| Authentication | API tokens stay in control jobs; Git checkout is anonymous | Scoped private Git credentials, authenticated registry adapters, submodule/LFS handling and service-side webhook verification |
| Bot deployment | Versioned Actions workflows; no hosted service | App installation flow, verified webhooks, queue/backpressure, durable request states and alternative GHES artifact transport |
| Triage | Compiler diagnostics, resolved dependency paths/features, symptom grouping and immutable local issue bundles | Proven dependency-cause graph and policy/suppressions with reason and expiry |

The current GitLab form route follows
[current GitLab documentation](https://docs.gitlab.com/user/project/issues/create_issues/).
Older self-managed releases or projects using a different tracker can need a
different route; copyable drafts remain available. Capability flags describe
implemented URL families, rather than probing whether a project enables issues.

The disk watcher can overshoot between observations, particularly with parallel
writers. Docker image layers and tmpfs are outside managed-work accounting.
Arbitrary blocking filesystem operations cannot always be interrupted. These
limits should remain visible when assessing deployment readiness.
