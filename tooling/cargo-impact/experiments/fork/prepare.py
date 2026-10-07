"""Prepare a bounded experiment from exact checkouts of this fork.

Only the actual Refineable crate and its sibling derive crate are copied. The
rest of GPUI's platform/build graph is irrelevant to the selected API change.
Consumers are synthetic packages committed in this fork; there is no discovery.
"""

import argparse
import json
import shutil
from pathlib import Path


def copy_library(checkout: Path, destination: Path) -> None:
    for name in ("gpui_refineable", "gpui_derive_refineable"):
        shutil.copytree(checkout / "crates" / name, destination / "crates" / name)
    (destination / "Cargo.toml").write_text(
        '[workspace]\nresolver = "3"\n'
        'members = ["crates/gpui_refineable", "crates/gpui_derive_refineable"]\n'
    )


def prepare(baseline: Path, candidate: Path, output: Path, missing_target: bool, local: bool, consumer_revision: str | None) -> None:
    output.mkdir(parents=True, exist_ok=False)
    for name, checkout in (("baseline", baseline), ("candidate", candidate)):
        copy_library(checkout, output / name)
    config = ['library = "gpui_ce_refineable"', "[discovery]", "crates_io = false", "github = false"]
    if local:
        config.extend(["[recipe.runner]", 'kind = "local"'])
    for name in ("reserve-user", "base-user"):
        config.extend(["[[downstreams]]", f"name = {json.dumps(name)}"])
        if consumer_revision:
            config.extend([
                f'manifest = "tooling/cargo-impact/experiments/fork/consumers/{name}/Cargo.toml"',
                "[downstreams.source]", 'kind = "git"',
                'url = "https://github.com/philocalyst/gpui-ce"',
                f"revision = {json.dumps(consumer_revision)}",
            ])
        else:
            source = baseline / "tooling/cargo-impact/experiments/fork/consumers" / name
            destination = output / "consumers" / name
            shutil.copytree(source, destination)
            config.extend(["[downstreams.source]", 'kind = "local"', f"path = {json.dumps(str(destination.resolve()))}"])
    if missing_target:
        config.extend([
            '[overrides."reserve-user"]',
            'target = "thumbv7em-none-eabi"',
        ])
        if local:
            config.extend(['[overrides."reserve-user".runner]', 'kind = "local"'])
    (output / "impact.toml").write_text("\n".join(config) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--missing-target", action="store_true")
    parser.add_argument("--local", action="store_true")
    parser.add_argument("--consumer-revision", help="Clone only the personal fork at this exact commit, retaining source permalinks")
    args = parser.parse_args()
    prepare(args.baseline.resolve(), args.candidate.resolve(), args.output.resolve(), args.missing_target, args.local, args.consumer_revision)
