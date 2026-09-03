#!/usr/bin/env python3
"""Summarize paired framing and message-read samples."""

from __future__ import annotations

import csv
import statistics
import sys
from collections import defaultdict
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 5:
        raise SystemExit(
            "usage: summarize-message-read-benchmarks.py RAW SUMMARY COMPARISON INCREMENTAL"
        )
    raw, summary_path, comparison_path, incremental_path = map(Path, sys.argv[1:])
    samples: dict[tuple[str, ...], list[float]] = defaultdict(list)
    with raw.open(newline="", encoding="utf-8") as source:
        for row in csv.DictReader(source, delimiter="\t"):
            if row["run"].startswith("warmup-"):
                continue
            key = (row["implementation"], row["case"], row["segments"], row["passes"])
            samples[key].append(int(row["elapsed_ns"]) / int(row["passes"]))

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
            cpp_incremental = cpp_root - cpp_framing
            native_incremental = native_root - native_framing
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


if __name__ == "__main__":
    main()
