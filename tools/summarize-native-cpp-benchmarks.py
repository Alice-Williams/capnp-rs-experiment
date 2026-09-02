#!/usr/bin/env python3
"""Summarize checked native-versus-C++ benchmark samples without dependencies."""

from __future__ import annotations

import csv
import statistics
import sys
from collections import defaultdict
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 4:
        raise SystemExit("usage: summarize-native-cpp-benchmarks.py RAW SUMMARY COMPARISON")
    raw, summary_path, comparison_path = map(Path, sys.argv[1:])
    samples: dict[tuple[str, ...], list[float]] = defaultdict(list)
    with raw.open(newline="", encoding="utf-8") as source:
        for row in csv.DictReader(source, delimiter="\t"):
            if row["run"].startswith("warmup-"):
                continue
            key = (
                row["implementation"],
                row["case"],
                row["mode"],
                row["scratch"],
                row["compression"],
                row["iterations"],
            )
            samples[key].append(int(row["elapsed_ns"]) / int(row["iterations"]))

    headings = [
        "implementation",
        "case",
        "mode",
        "scratch",
        "compression",
        "iterations",
        "runs",
        "mean_ns_per_iteration",
        "median_ns_per_iteration",
        "min_ns_per_iteration",
        "max_ns_per_iteration",
    ]
    summaries: dict[tuple[str, ...], float] = {}
    with summary_path.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.writer(destination, delimiter="\t", lineterminator="\n")
        writer.writerow(headings)
        for key in sorted(samples):
            values = samples[key]
            median = statistics.median(values)
            summaries[key] = median
            writer.writerow(
                [
                    *key,
                    len(values),
                    f"{statistics.fmean(values):.2f}",
                    f"{median:.2f}",
                    f"{min(values):.2f}",
                    f"{max(values):.2f}",
                ]
            )

    with comparison_path.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.writer(destination, delimiter="\t", lineterminator="\n")
        writer.writerow(
            [
                "case",
                "mode",
                "scratch",
                "compression",
                "iterations",
                "cpp_median_ns_per_iteration",
                "native_median_ns_per_iteration",
                "native_over_cpp",
            ]
        )
        scenarios = sorted({key[1:] for key in summaries})
        for scenario in scenarios:
            cpp = summaries[("cpp", *scenario)]
            native = summaries[("native", *scenario)]
            writer.writerow([*scenario, f"{cpp:.2f}", f"{native:.2f}", f"{native / cpp:.3f}"])


if __name__ == "__main__":
    main()
