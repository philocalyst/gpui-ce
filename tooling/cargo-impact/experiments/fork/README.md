# Fork-contained Actions experiment

This experiment is restricted to `philocalyst/gpui-ce`. Both workflows check the
repository identity; the comment workflow accepts only the repository owner's
`/cargo-impact check` command. All checkouts use that repository explicitly.
No reviewers, assignees, user mentions, tags, releases, or registry publication
are part of the experiment. Discovery is disabled in the generated configuration
and on the command line. Consumers are committed fixtures in this fork.

The selected library is the actual `gpui_ce_refineable` crate. Preparation copies
that crate and its sibling derive crate from the pinned base/candidate checkouts
into a minimal workspace. This keeps GPUI's unrelated platform dependency graph
out of the experiment while testing its real Rust code, renamed library target,
procedural macro, semver gate, and downstream diagnostics.

| Planned draft PR | Change to the real crate | Expected result |
| --- | --- | --- |
| Additive API | Add a public experiment marker function | Gate closes; no consumer builds; exit 0 |
| Used API becomes private | Make `Cascade::reserve` crate-private | Gate opens; reserve-user gets E0624; base-user succeeds; exit 1 |
| Unused API becomes private | Make `Cascade::set` crate-private | Gate opens; both consumers succeed; exit 0 |

The PR workflow also supports manual dispatch. Dispatch forces consumer builds
against the selected branch to exercise Docker without requiring an API change.
`missing_target=true` assigns only reserve-user an unavailable embedded target;
its failure should be classified as an environment error, not a regression.
Dispatch again with `false` to verify recovery with the corrected recipe.

The scanner binary cache includes its toolchain, source, lockfile, embedded
templates, and license. Build-result caches use separate PR/dispatch namespaces. The API token
is supplied only to the authorization and reporting jobs. The build job uses
Docker and supplies no GitHub token to the scanner or its Cargo commands.

For comment-event testing, the workflow must be present on the fork's default
branch. The planned test branch is `codex/impact-fork-base`; changing the fork's
default branch temporarily makes this test possible without changing `main`'s
Git history. The default must be restored to `main` after the runs finish. The
inherited prerelease workflow is main-specific and must never be dispatched or
allowed to publish as part of this experiment. The test base excludes itself
from the broad inherited PR CI to avoid unrelated platform builds.

The fork is public: draft PRs, workflow logs, artifacts, and bot reports would be
publicly viewable. Remote execution is pending clarification of whether
"private" means fork-contained or confidential. No remote state has been
modified so far.

## Local results

The same fixtures were exercised locally with the explicit trusted-host opt-in:
the unchanged library closes the gate; private `reserve` produces E0624 at
reserve-user's source span while base-user succeeds; private `set` opens the
gate but both consumers compile. The missing target produces E0463 and a
harness-failure classification, and removing that recipe override recovers.
Reports are retained in the ignored repository-local `.cargo-impact/` directory.
Hosted Docker, reactions, permission checks, and report comments still require
the GitHub runs.

```sh
python3 tooling/cargo-impact/experiments/fork/prepare.py \
  --baseline . --candidate . --output .cargo-impact/fork-local-inputs --local
cargo run --manifest-path tooling/cargo-impact/Cargo.toml -- check \
  --baseline .cargo-impact/fork-local-inputs/baseline \
  --candidate .cargo-impact/fork-local-inputs/candidate \
  --config .cargo-impact/fork-local-inputs/impact.toml \
  --work-dir .cargo-impact/fork-local-work \
  --report-dir .cargo-impact/fork-local-report --no-discovery --allow-local
```
