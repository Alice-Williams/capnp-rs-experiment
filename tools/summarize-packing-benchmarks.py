#!/usr/bin/env python3
"""Summarize matched native and C++ packed-codec samples."""

from __future__ import annotations

import csv
import statistics
import sys
from collections import defaultdict
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 5:
        raise SystemExit("usage: summarize-packing-benchmarks.py RAW SUMMARY COMPARISON INCREMENTAL")
    raw_path, summary_path, comparison_path, incremental_path = map(Path, sys.argv[1:])
    samples: dict[tuple[str, str, str, str, str], list[float]] = defaultdict(list)
    with raw_path.open(newline="", encoding="utf-8") as source:
        for row in csv.DictReader(source, delimiter="\t"):
            if row["run"].startswith("warmup-"):
                continue
            operations = int(row["words"]) * int(row["passes"])
            key = (
                row["implementation"], row["case"], row["shape"],
                row["words"], row["passes"],
            )
            samples[key].append(int(row["elapsed_ns"]) / operations)

    medians: dict[tuple[str, str, str], float] = {}
    with summary_path.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.writer(destination, delimiter="\t", lineterminator="\n")
        writer.writerow([
            "implementation", "case", "shape", "words", "passes", "runs",
            "mean_ns_per_word", "median_ns_per_word", "min_ns_per_word", "max_ns_per_word",
        ])
        for key in sorted(samples):
            values = samples[key]
            median = statistics.median(values)
            medians[(key[0], key[1], key[2])] = median
            writer.writerow([
                *key, len(values), f"{statistics.fmean(values):.4f}", f"{median:.4f}",
                f"{min(values):.4f}", f"{max(values):.4f}",
            ])

    with comparison_path.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.writer(destination, delimiter="\t", lineterminator="\n")
        writer.writerow([
            "case", "shape", "cpp_median_ns_per_word", "native_median_ns_per_word",
            "native_over_cpp",
        ])
        cases = sorted({(key[1], key[2]) for key in medians})
        for case, shape in cases:
            cpp = medians[("cpp", case, shape)]
            native = medians[("native", case, shape)]
            writer.writerow([case, shape, f"{cpp:.4f}", f"{native:.4f}", f"{native / cpp:.3f}"])

    with incremental_path.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.writer(destination, delimiter="\t", lineterminator="\n")
        writer.writerow([
            "case", "shape", "lower_case", "lower_native_over_cpp",
            "cumulative_native_over_cpp", "cpp_increment_ns_per_word",
            "native_increment_ns_per_word", "incremental_native_over_cpp",
        ])
        transforms = (
            ("pack", "copy-unpacked"),
            ("pack-stream", "copy-unpacked"),
            ("unpack", "copy-packed"),
            ("unpack-stream", "copy-packed"),
        )
        for case, lower_case in transforms:
            for shape in sorted({key[2] for key in medians if key[1] == case}):
                cpp = medians[("cpp", case, shape)]
                native = medians[("native", case, shape)]
                lower_cpp = medians[("cpp", lower_case, shape)]
                lower_native = medians[("native", lower_case, shape)]
                cpp_increment = cpp - lower_cpp
                native_increment = native - lower_native
                incremental = native_increment / cpp_increment if cpp_increment > 0 else float("nan")
                writer.writerow([
                    case, shape, lower_case, f"{lower_native / lower_cpp:.3f}",
                    f"{native / cpp:.3f}", f"{cpp_increment:.4f}",
                    f"{native_increment:.4f}", f"{incremental:.3f}",
                ])


if __name__ == "__main__":
    main()
