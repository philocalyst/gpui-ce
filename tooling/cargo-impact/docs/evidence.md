# Evidence and recovery contract

Each experiment has a full SHA-256 `ExperimentId` over a versioned library name,
source selection, manifest and effective recipe. Features and package selections
are normalized. Different recipes remain independent even when display names match.
Git branches resolve once; baseline and candidate use the same consumer snapshot.

Reports and managed work must use separate directory trees. Report output must also
stay outside library and local consumer sources; these checks run before failure reports
or checkpoints can overwrite files. Existing symlink aliases and future paths are resolved
during preflight. With `--candidate .`, select a sibling report directory or a temporary CI
directory. The engine refuses known source inputs beneath managed work before pruning;
work beneath a source's excluded `.cargo-impact` directory remains supported.

An embedding's lazy discovery callback must supply source inputs outside managed work.
The engine rejects a discovered local path there before dispatch, but cannot protect a
path that was not supplied until after an explicitly requested pre-run prune.

The engine separates preparation, scheduling, experiment execution and storage.
Workers send typed lifecycle events. The coordinator owns reports and acknowledges
a durable completed-baseline checkpoint before the candidate can begin. Queued and
running experiments remain pending. A typed failure records stage and cause while
retaining completed build evidence. Rendered assessments treat integrity failures as
inconclusive and suppress source links for mutated inputs.

## Published bundles

`report.json` at the root is the latest durable compiler evidence. Publication renders
all views into a private stage, records an exact bounded inventory, syncs it and renames
the complete stage to `bundles/<content-sha256>/`. The identity includes every artifact,
so a changed renderer receives a new generation even when its evidence is unchanged.
`manifest.json` identifies that generation; `bundle.json` is the final publication record.

Canonical HTML, Markdown, SARIF, issue drafts, reproduction TOML and retained phase
locks live together inside the generation. Their relative links remain portable in a
ZIP or a local `file://` browser. Root HTML redirects to that generation and root Markdown
links into it. Root aliases are individually replaced convenience views. Existing
generations stay intact when another run publishes or checkpoints.

Verification checks the publication, manifest identity, exact file inventory, each size
and SHA-256, and the root checkpoint's agreement with its publication. It rejects unsafe
paths, internal symlinks, extra files, missing files, changed files and unsupported formats.
The parsed JSON comes from the exact bytes that were hashed. A JSON-only checkpoint has
an explicitly different verification contract. Hashes provide transport integrity;
the download or CI run supplies the trust context.

| Failure | Retained evidence | Recovery |
| --- | --- | --- |
| Cancelled during compilation | Latest acknowledged phase and pending lifecycle in checkpoint JSON | Inspect or render that JSON; rerun the same work directory |
| Secondary renderer/root alias failed | Newest root JSON, previous publication and any complete staged generation | Render `report.json` into a usable destination; verify afterward |
| Bundle differs after transport | Original downloaded files, verification error | Download the original trusted artifact again; the bot reports an inconclusive transport failure |
| Two writers choose one report directory | One writer owns the lock; the other fails before changing JSON | Use independent report directories or retry after the writer completes |
| Old generations consume disk | Complete generations and their bounded inventories | Preview `prune-reports`; apply explicit retention after review |

Retention holds the writer lock and verifies every generation selected for deletion.
It keeps the publication plus the requested previous generations, ordered by recorded
filesystem modification time. Unknown, incomplete, corrupt and user-owned directories
remain for inspection. Checkpoints must be rendered before pruning. Deletion first
quarantines each selected generation so an interrupted removal cannot resemble a
complete publication.

## Clean replay

`ReplayRequest` selects one completed controlled Docker experiment by its exact ID.
The replay identity includes the engine implementation/dependency hash, three frozen
source hashes, effective recipe, phase lockfiles, injection patch URLs, normalized
resolved graphs, Cargo/rustc versions and immutable image identity. Package identities
remove absolute worker paths while retaining source, name, version, origin and manifest.
Raw Cargo IDs remain available for diagnostic lookup. External unfrozen path dependencies
cannot establish complete replay identity.

A replay checks sources before scheduling, resolves the recorded immutable consumer
commit, applies phase source patches before locked dependency resolution, and compares
fresh compiler/image/lock/graph evidence before compiling. It clears only that experiment's
baseline and candidate targets under the managed-work lock and retains downloads.
A mismatch returns a typed `EvidenceMismatch` failure and preserves observed evidence.

Replay records the original expected classification and the new comparison separately.
Identical input identity cannot attest nondeterministic build behavior; a changed outcome
needs inspection. The compiler diagnostics and dependency paths explain the observed
failure. Matching symptoms do not automatically establish a shared dependency cause.
Runtime behavior, arbitrary feature matrices and ecosystem completeness remain outside
`cargo check --all-targets` comparisons.
