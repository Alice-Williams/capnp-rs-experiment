#!/usr/bin/env python3
"""Summarize paired framing and message-read samples."""

from __future__ import annotations

import csv
import statistics
import sys
from collections import defaultdict
from pathlib import Path


def main() -> None:
    if len(sys.argv) not in (5, 6):
        raise SystemExit(
            "usage: summarize-message-read-benchmarks.py RAW SUMMARY COMPARISON INCREMENTAL [SCALAR_INCREMENTAL]"
        )
    raw, summary_path, comparison_path, incremental_path = map(Path, sys.argv[1:5])
    scalar_incremental_path = Path(sys.argv[5]) if len(sys.argv) == 6 else None
    samples: dict[tuple[str, ...], list[float]] = defaultdict(list)
    samples_by_run: dict[tuple[str, ...], float] = {}
    with raw.open(newline="", encoding="utf-8") as source:
        for row in csv.DictReader(source, delimiter="\t"):
            if row["run"].startswith("warmup-"):
                continue
            key = (row["implementation"], row["case"], row["segments"], row["passes"])
            elapsed = int(row["elapsed_ns"]) / int(row["passes"])
            samples[key].append(elapsed)
            samples_by_run[(*key, row["run"])] = elapsed

    medians: dict[tuple[str, str, str], float] = {}
    with summary_path.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.writer(destination, delimiter="\t", lineterminator="\n")
        writer.writerow(
            [
                "implementation",
                "case",
                "segments",
                "passes",
                "runs",
                "mean_ns_per_message",
                "median_ns_per_message",
                "min_ns_per_message",
                "max_ns_per_message",
            ]
        )
        for key in sorted(samples):
            values = samples[key]
            median = statistics.median(values)
            medians[(key[0], key[1], key[2])] = median
            writer.writerow(
                [
                    *key,
                    len(values),
                    f"{statistics.fmean(values):.4f}",
                    f"{median:.4f}",
                    f"{min(values):.4f}",
                    f"{max(values):.4f}",
                ]
            )

    with comparison_path.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.writer(destination, delimiter="\t", lineterminator="\n")
        writer.writerow(
            [
                "case",
                "segments",
                "cpp_median_ns_per_message",
                "native_median_ns_per_message",
                "native_over_cpp",
            ]
        )
        for case, segments in sorted({(key[1], key[2]) for key in medians}):
            cpp = medians[("cpp", case, segments)]
            native = medians[("native", case, segments)]
            writer.writerow(
                [case, segments, f"{cpp:.4f}", f"{native:.4f}", f"{native / cpp:.3f}"]
            )

    with incremental_path.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.writer(destination, delimiter="\t", lineterminator="\n")
        writer.writerow(
            [
                "segments",
                "cpp_root_minus_framing_ns",
                "native_root_minus_framing_ns",
                "incremental_native_over_cpp",
                "framing_native_over_cpp",
                "root_native_over_cpp",
            ]
        )
        for segments in sorted({key[2] for key in medians}, key=int):
            cpp_framing = medians[("cpp", "framing", segments)]
            native_framing = medians[("native", "framing", segments)]
            cpp_root = medians[("cpp", "root", segments)]
            native_root = medians[("native", "root", segments)]
            cpp_incremental = paired_median(
                samples_by_run, "cpp", "root", "framing", segments
            )
            native_incremental = paired_median(
                samples_by_run, "native", "root", "framing", segments
            )
            if cpp_incremental <= 0 or native_incremental <= 0:
                raise SystemExit(f"non-positive incremental median for {segments} segments")
            writer.writerow(
                [
                    segments,
                    f"{cpp_incremental:.4f}",
                    f"{native_incremental:.4f}",
                    f"{native_incremental / cpp_incremental:.3f}",
                    f"{native_framing / cpp_framing:.3f}",
                    f"{native_root / cpp_root:.3f}",
                ]
            )

    if scalar_incremental_path is not None:
        with scalar_incremental_path.open("w", newline="", encoding="utf-8") as destination:
            writer = csv.writer(destination, delimiter="\t", lineterminator="\n")
            writer.writerow(
                [
                    "segments",
                    "cpp_scalar_only_ns",
                    "native_scalar_only_ns",
                    "scalar_only_native_over_cpp",
                    "scalars_native_over_cpp",
                    "paired_cpp_scalars_minus_root_ns",
                    "paired_native_scalars_minus_root_ns",
                ]
            )
            for segments in sorted({key[2] for key in medians}, key=int):
                cpp_incremental = paired_median(
                    samples_by_run, "cpp", "scalars", "root", segments
                )
                native_incremental = paired_median(
                    samples_by_run, "native", "scalars", "root", segments
                )
                cpp_scalar_only = medians[("cpp", "scalar-only", segments)]
                native_scalar_only = medians[("native", "scalar-only", segments)]
                writer.writerow(
                    [
                        segments,
                        f"{cpp_scalar_only:.4f}",
                        f"{native_scalar_only:.4f}",
                        f"{native_scalar_only / cpp_scalar_only:.3f}",
                        f"{medians[('native', 'scalars', segments)] / medians[('cpp', 'scalars', segments)]:.3f}",
                        f"{cpp_incremental:.4f}",
                        f"{native_incremental:.4f}",
                    ]
                )


def paired_median(
    samples: dict[tuple[str, ...], float],
    implementation: str,
    upper_case: str,
    lower_case: str,
    segments: str,
) -> float:
    upper = {
        key[4]: value
        for key, value in samples.items()
        if key[0] == implementation and key[1] == upper_case and key[2] == segments
    }
    lower = {
        key[4]: value
        for key, value in samples.items()
        if key[0] == implementation and key[1] == lower_case and key[2] == segments
    }
    runs = sorted(upper.keys() & lower.keys(), key=int)
    if not runs:
        raise SystemExit(
            f"no paired {lower_case}/{upper_case} samples for {implementation} {segments}"
        )
    return statistics.median(upper[run] - lower[run] for run in runs)


if __name__ == "__main__":
    main()
