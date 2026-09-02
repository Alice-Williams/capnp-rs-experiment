#!/usr/bin/env python3
"""Summarize native-versus-C++ sequential RPC samples."""

from __future__ import annotations

import csv
import statistics
import sys
from collections import defaultdict
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 4:
        raise SystemExit("usage: summarize-native-cpp-rpc.py RAW SUMMARY COMPARISON")
    raw, summary_path, comparison_path = map(Path, sys.argv[1:])
    samples: dict[tuple[str, str, str], list[float]] = defaultdict(list)
    with raw.open(newline="", encoding="utf-8") as source:
        for row in csv.DictReader(source, delimiter="\t"):
            if row["run"].startswith("warmup-"):
                continue
            key = (row["implementation"], row["transport"], row["iterations"])
            samples[key].append(int(row["elapsed_ns"]) / int(row["iterations"]))

    headings = [
        "implementation",
        "transport",
        "iterations",
        "runs",
        "mean_ns_per_call",
        "median_ns_per_call",
        "min_ns_per_call",
        "max_ns_per_call",
    ]
    medians: dict[str, float] = {}
    with summary_path.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.writer(destination, delimiter="\t", lineterminator="\n")
        writer.writerow(headings)
        for key in sorted(samples):
            values = samples[key]
            median = statistics.median(values)
            medians[key[0]] = median
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
        writer.writerow(["cpp_median_ns_per_call", "native_median_ns_per_call", "native_over_cpp"])
        writer.writerow(
            [f"{medians['cpp']:.2f}", f"{medians['native']:.2f}", f"{medians['native'] / medians['cpp']:.3f}"]
        )


if __name__ == "__main__":
    main()
