#!/usr/bin/env python3
"""Summarize paired prepared-storage and fresh message-build samples."""

from __future__ import annotations

import csv
import statistics
import sys
from collections import defaultdict
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 6:
        raise SystemExit(
            "usage: summarize-message-build-benchmarks.py RAW SUMMARY COMPARISON INCREMENTAL COPY_INCREMENTAL"
        )
    raw, summary_path, comparison_path, incremental_path, copy_incremental_path = map(
        Path, sys.argv[1:]
    )
    samples: dict[tuple[str, ...], list[float]] = defaultdict(list)
    by_run: dict[tuple[str, ...], float] = {}
    with raw.open(newline="", encoding="utf-8") as source:
        for row in csv.DictReader(source, delimiter="\t"):
            if row["run"].startswith("warmup-"):
                continue
            key = (row["implementation"], row["case"], row["shape"], row["passes"])
            elapsed = int(row["elapsed_ns"]) / int(row["passes"])
            samples[key].append(elapsed)
            by_run[(*key, row["run"])] = elapsed

    medians: dict[tuple[str, str, str], float] = {}
    with summary_path.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.writer(destination, delimiter="\t", lineterminator="\n")
        writer.writerow(
            [
                "implementation", "case", "shape", "passes", "runs",
                "mean_ns_per_message", "median_ns_per_message",
                "min_ns_per_message", "max_ns_per_message",
            ]
        )
        for key in sorted(samples):
            values = samples[key]
            median = statistics.median(values)
            medians[(key[0], key[1], key[2])] = median
            writer.writerow(
                [*key, len(values), f"{statistics.fmean(values):.4f}",
                 f"{median:.4f}", f"{min(values):.4f}", f"{max(values):.4f}"]
            )

    with comparison_path.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.writer(destination, delimiter="\t", lineterminator="\n")
        writer.writerow(
            ["case", "shape", "cpp_median_ns_per_message",
             "native_median_ns_per_message", "native_over_cpp"]
        )
        for case, shape in sorted({(key[1], key[2]) for key in medians}):
            cpp = medians[("cpp", case, shape)]
            native = medians[("native", case, shape)]
            writer.writerow([case, shape, f"{cpp:.4f}", f"{native:.4f}",
                             f"{native / cpp:.3f}"])

    with incremental_path.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.writer(destination, delimiter="\t", lineterminator="\n")
        writer.writerow(
            ["shape", "cpp_fresh_minus_prepared_ns",
             "native_fresh_minus_prepared_ns", "incremental_native_over_cpp",
             "prepared_native_over_cpp", "fresh_native_over_cpp"]
        )
        for shape in sorted({key[2] for key in medians if key[1] == "fresh"}):
            cpp_incremental = paired_median(by_run, "cpp", "fresh", "prepared", shape)
            native_incremental = paired_median(by_run, "native", "fresh", "prepared", shape)
            if cpp_incremental <= 0 or native_incremental <= 0:
                raise SystemExit(f"non-positive incremental median for {shape}")
            writer.writerow(
                [shape, f"{cpp_incremental:.4f}", f"{native_incremental:.4f}",
                 f"{native_incremental / cpp_incremental:.3f}",
                 f"{medians[('native', 'prepared', shape)] / medians[('cpp', 'prepared', shape)]:.3f}",
                 f"{medians[('native', 'fresh', shape)] / medians[('cpp', 'fresh', shape)]:.3f}"]
            )

    with copy_incremental_path.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.writer(destination, delimiter="\t", lineterminator="\n")
        writer.writerow(
            ["shape", "cpp_copy_minus_prepared_ns", "native_copy_minus_prepared_ns",
             "incremental_native_over_cpp", "prepared_native_over_cpp",
             "copy_native_over_cpp"]
        )
        cpp_incremental = paired_median(
            by_run, "cpp", "copy", "copy-prepared", "graph"
        )
        native_incremental = paired_median(
            by_run, "native", "copy", "copy-prepared", "graph"
        )
        if cpp_incremental <= 0 or native_incremental <= 0:
            raise SystemExit("non-positive incremental median for graph copy")
        writer.writerow(
            ["graph", f"{cpp_incremental:.4f}", f"{native_incremental:.4f}",
             f"{native_incremental / cpp_incremental:.3f}",
             f"{medians[('native', 'copy-prepared', 'graph')] / medians[('cpp', 'copy-prepared', 'graph')]:.3f}",
             f"{medians[('native', 'copy', 'graph')] / medians[('cpp', 'copy', 'graph')]:.3f}"]
        )


def paired_median(
    samples: dict[tuple[str, ...], float], implementation: str,
    upper_case: str, lower_case: str, shape: str
) -> float:
    upper = {key[4]: value for key, value in samples.items()
             if key[0] == implementation and key[1] == upper_case and key[2] == shape}
    lower = {key[4]: value for key, value in samples.items()
             if key[0] == implementation and key[1] == lower_case and key[2] == shape}
    runs = sorted(upper.keys() & lower.keys(), key=int)
    if not runs:
        raise SystemExit(
            f"no paired {lower_case}/{upper_case} samples for {implementation} {shape}"
        )
    return statistics.median(upper[run] - lower[run] for run in runs)


if __name__ == "__main__":
    main()
