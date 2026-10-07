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
| Replay looked more exact than the available data justified | Starter recipes identify recorded provenance, warn about floating images and missing resolved lockfiles, and label legacy reports. A per-consumer recipe resets its coverage minimum to one. |
| Bot reruns created noise or could publish against a moved head | Update the bot's own marked comment, ignore forged human markers, bounded history lookup and a second head check immediately before writing. Only reads retry; ambiguous writes do not. |
| A failed worker could leave the command unanswered | Comment workflows can export an explicit inconclusive report when no worker artifact is available. Authorization still checks current repository permission before the eyes reaction. |
| Setup had hidden prerequisites | Self-contained `init`, preserved `.gitignore` additions, bounded `doctor`, standalone scanner assets and offline rendering. Setup rejects symlinks inside the selected root and refuses existing installation files. |
| One obsolete registry package poisoned another valid package in the same repository | Separate package selections, normalized selection deduplication, experiment budget and explicit overlap notes. |

JSON is the authoritative evidence. Individual artifact files are replaced
atomically, but a bundle is not a transactional multi-file filesystem snapshot.
A crash between view writes can leave views from different checkpoints; render
the saved JSON to regenerate a consistent bundle. An immutable generation layout
is a useful next improvement.

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
they do not prove that one dependency caused every failure. A dependency-cause
graph, infrastructure retry policy, reasoned suppressions with expiry, and clean
confirmation builds are still missing.

[cargo-audit's presenter](https://github.com/rustsec/rustsec/blob/main/cargo-audit/src/presenter.rs)
uses inverse dependency explanations and actionable solutions, while
[its SARIF output](https://github.com/rustsec/rustsec/blob/main/cargo-audit/src/sarif.rs)
illustrates structured results and stable fingerprints. This scanner now provides
actionable issue/replay bundles and SARIF, validated against the
[official OASIS schema](https://github.com/oasis-tcs/sarif-spec/blob/main/sarif-2.1/schema/sarif-schema-2.1.0.json).
Index freshness/commit metadata and a richer dependency explanation should be
first-class evidence rather than inferred confidence.

## Work still needed

| Area | Current boundary | Next production step |
| --- | --- | --- |
| Discovery | crates.io published direct dependents and optional indexed GitHub code search; repository HEAD is built | Published archive/version adapter, index freshness, sampled/curated watchlists and transitive dependency coverage |
| Replay/storage | Phase-specific incremental compiler caches; fingerprints identify sources and locks | Immutable experiment manifest, retained lockfiles and source refs, append-only pair events, verified shared dependency seeds and quota-backed cache volumes |
| Execution | `cargo check --all-targets` for one recipe | Feature/target/MSRV matrices, build/test modes, clean confirmation and retryable infrastructure errors |
| Resource bounds | Managed-work watchdog, Docker worker limits, supervised CLI semver helper | Hard disk quotas, Docker image/tmpfs accounting, cgroup/process peak telemetry and explicit OOM classification |
| Embedding | Optional semver helper; arbitrary discovery callback | Require an interruption-aware executor for untrusted production embeddings; inline analysis and arbitrary blocking callbacks are not forcibly supervised |
| Source contract | Compilation reads immutable sources; generated files should use Cargo `OUT_DIR` | Trusted preparation adapters for exceptional valid packages that generate into their source tree |
| Forges | HTTPS Git cloning; GitHub, GitLab and Gitea web link/form implementations; GitHub Actions bot | GitLab/Gitea search and permission/comment adapters, per-repository URL templates and custom/disabled tracker support |
| Authentication | API tokens stay in control jobs; Git checkout is anonymous | Scoped private Git credentials, authenticated registry adapters, submodule/LFS handling and service-side webhook verification |
| Bot deployment | Versioned Actions workflows; no hosted service | App installation flow, verified webhooks, queue/backpressure, durable request states and alternative GHES artifact transport |
| Triage | Compiler diagnostics, origin, symptom grouping and local drafts | Dependency-cause graph, policy/suppressions with reason and expiry, immutable issue attachments and report generations |

The current GitLab form route follows
[current GitLab documentation](https://docs.gitlab.com/user/project/issues/create_issues/).
Older self-managed releases or projects using a different tracker can need a
different route; copyable drafts remain available. Capability flags describe
implemented URL families, rather than probing whether a project enables issues.

The disk watcher can overshoot between observations, particularly with parallel
writers. Docker image layers and tmpfs are outside managed-work accounting.
Arbitrary blocking filesystem operations cannot always be interrupted. These
limits should remain visible when assessing deployment readiness.
