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

| Draft PR | Change to the real crate | Expected result |
| --- | --- | --- |
| [Additive API: PR 1](https://github.com/philocalyst/gpui-ce/pull/1) | Add a public experiment marker function | Gate closes; no consumer builds; exit 0 |
| [Used API becomes private: PR 2](https://github.com/philocalyst/gpui-ce/pull/2) | Make `Cascade::reserve` crate-private | Gate opens; reserve-user gets E0624; base-user succeeds; exit 1 |
| [Unused API becomes private: PR 3](https://github.com/philocalyst/gpui-ce/pull/3) | Make `Cascade::set` crate-private | Gate opens; both consumers succeed; exit 0 |

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

The user authorized public draft PRs and Actions runs confined to this personal
fork. Draft PRs, workflow logs, artifacts, and bot reports are publicly viewable.
The first hosted run caught Cargo rejecting an inline-table `--config` value for
rustdoc. Rustdoc's bootstrap flag now uses Docker's environment option instead;
both failures and successful scans preserve their reports as artifacts.

## Local results

The same fixtures were exercised locally with the explicit trusted-host opt-in:
the unchanged library closes the gate; private `reserve` produces E0624 at
reserve-user's source span while base-user succeeds; private `set` opens the
gate but both consumers compile. The missing target produces E0463 and a
harness-failure classification, and removing that recipe override recovers.
Reports are retained in the ignored repository-local `.cargo-impact/` directory.
The hosted runs below exercise Docker, reactions, permission checks, and report
comments with the same fixtures.

## Hosted results

The corrected Docker runs matched all three expected outcomes:
[additive](https://github.com/philocalyst/gpui-ce/actions/runs/37517124744),
[used API](https://github.com/philocalyst/gpui-ce/actions/runs/37517123762), and
[unused API](https://github.com/philocalyst/gpui-ce/actions/runs/37517122444).
The used-API job's failure is intentional: its JSON records a successful
baseline, candidate E0624 at `src/lib.rs:10:25`, full secondary spans, the
consumer's package/target, and a compatible base-user. Markdown deduplicates
the library/test-target errors while retaining both records in JSON.

[Missing-target dispatch](https://github.com/philocalyst/gpui-ce/actions/runs/37517134500)
returned E0463 as `harness_failure` / `environment`; base-user still succeeded.
[Recovery dispatch](https://github.com/philocalyst/gpui-ce/actions/runs/37517954062)
compiled both consumers after removing the target override.

[The comment run](https://github.com/philocalyst/gpui-ce/actions/runs/37517143302)
verified the owner's current permission, reacted with eyes, and posted
[a diagnostic report](https://github.com/philocalyst/gpui-ce/pull/2#issuecomment-6023820205)
with the tested head and artifact link despite the build's intentional exit 1.
The final reusable installation action pins the host toolchain and saves its
binary before downstream work so compiler failures do not discard that cache.
Hosted comment jobs revealed GitHub's read-only cache-token scope for
`issue_comment`. The final bot therefore transfers its trusted scanner through
an immutable same-run artifact; PR and dispatch events retain writable caches.
The generic default-branch push job warms scanner installation, and the fork's
scanner-only dispatch permits exercising that cache without running consumers.

An additional anonymous Git checkout of this same fork pinned the full commit
ID and compiled the nested reserve-user. It exposed an unrelated dangling
`tooling/perf/LICENSE-APACHE` link, now safely preserved by snapshots. The retry
returned the expected E0624 and a verified commit-pinned link to
`tooling/cargo-impact/experiments/fork/consumers/reserve-user/src/lib.rs#L10`.
Generated files, external dependency files, and upstream macro definitions
remain unlinked; verified macro invocation spans can be linked separately.

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
