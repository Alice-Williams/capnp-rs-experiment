#!/usr/bin/env python3
"""Summarize matched native and C++ wire-value samples."""

from __future__ import annotations

import csv
import statistics
import sys
from collections import defaultdict
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 4:
        raise SystemExit("usage: summarize-wire-value-benchmarks.py RAW SUMMARY COMPARISON")
    raw, summary_path, comparison_path = map(Path, sys.argv[1:])
    samples: dict[tuple[str, ...], list[float]] = defaultdict(list)
    with raw.open(newline="", encoding="utf-8") as source:
        for row in csv.DictReader(source, delimiter="\t"):
            if row["run"].startswith("warmup-"):
                continue
            operations = int(row["words"]) * int(row["passes"])
            key = (row["implementation"], row["case"], row["words"], row["passes"])
            samples[key].append(int(row["elapsed_ns"]) / operations)

    medians: dict[tuple[str, str, str], float] = {}
    with summary_path.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.writer(destination, delimiter="\t", lineterminator="\n")
        writer.writerow(
            [
                "implementation",
                "case",
                "words",
                "passes",
                "runs",
                "mean_ns_per_operation",
                "median_ns_per_operation",
                "min_ns_per_operation",
                "max_ns_per_operation",
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
                "words",
                "cpp_median_ns_per_operation",
                "native_median_ns_per_operation",
                "native_over_cpp",
            ]
        )
        cases = sorted({(key[1], key[2]) for key in medians})
        for case, words in cases:
            cpp = medians[("cpp", case, words)]
            native = medians[("native", case, words)]
            writer.writerow([case, words, f"{cpp:.4f}", f"{native:.4f}", f"{native / cpp:.3f}"])


if __name__ == "__main__":
    main()
