#!/usr/bin/env python3
"""Summarize paired generated-data API samples."""

from __future__ import annotations

import csv
import statistics
import sys
from collections import defaultdict
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 5:
        raise SystemExit(
            "usage: summarize-generated-api-benchmarks.py RAW SUMMARY COMPARISON INCREMENTAL"
        )
    raw, summary_path, comparison_path, incremental_path = map(Path, sys.argv[1:])
    samples: dict[tuple[str, str, str], list[float]] = defaultdict(list)
    by_run: dict[tuple[str, str, str], float] = {}
    with raw.open(newline="", encoding="utf-8") as source:
        for row in csv.DictReader(source, delimiter="\t"):
            if row["run"].startswith("warmup-"):
                continue
            key = (row["implementation"], row["case"], row["passes"])
            elapsed = int(row["elapsed_ns"]) / int(row["passes"])
            samples[key].append(elapsed)
            by_run[(row["implementation"], row["case"], row["run"])] = elapsed

    medians: dict[tuple[str, str], float] = {}
    with summary_path.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.writer(destination, delimiter="\t", lineterminator="\n")
        writer.writerow(
            [
                "implementation",
                "case",
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
            medians[(key[0], key[1])] = median
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
                "cpp_median_ns_per_operation",
                "native_median_ns_per_operation",
                "native_over_cpp",
            ]
        )
        for case in sorted({key[1] for key in medians}):
            cpp = medians[("cpp", case)]
            native = medians[("native", case)]
            writer.writerow([case, f"{cpp:.4f}", f"{native:.4f}", f"{native / cpp:.3f}"])

    with incremental_path.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.writer(destination, delimiter="\t", lineterminator="\n")
        writer.writerow(
            [
                "shape",
                "cpp_generated_minus_direct_ns",
                "native_generated_minus_direct_ns",
                "incremental_native_over_cpp",
                "direct_native_over_cpp",
                "generated_native_over_cpp",
            ]
        )
        for shape in ("scalars", "blobs"):
            cpp_incremental = paired_median(by_run, "cpp", shape)
            native_incremental = paired_median(by_run, "native", shape)
            cpp_direct = medians[("cpp", f"direct-{shape}")]
            native_direct = medians[("native", f"direct-{shape}")]
            cpp_generated = medians[("cpp", f"generated-{shape}")]
            native_generated = medians[("native", f"generated-{shape}")]
            ratio = (
                f"{native_incremental / cpp_incremental:.3f}"
                if cpp_incremental > 0 and native_incremental >= 0
                else "below-resolution"
            )
            writer.writerow(
                [
                    shape,
                    f"{cpp_incremental:.4f}",
                    f"{native_incremental:.4f}",
                    ratio,
                    f"{native_direct / cpp_direct:.3f}",
                    f"{native_generated / cpp_generated:.3f}",
                ]
            )


def paired_median(
    samples: dict[tuple[str, str, str], float], implementation: str, shape: str
) -> float:
    generated = {
        run: value
        for (impl, case, run), value in samples.items()
        if impl == implementation and case == f"generated-{shape}"
    }
    direct = {
        run: value
        for (impl, case, run), value in samples.items()
        if impl == implementation and case == f"direct-{shape}"
    }
    runs = sorted(generated.keys() & direct.keys(), key=int)
    if not runs:
        raise SystemExit(f"no paired {shape} samples for {implementation}")
    return statistics.median(generated[run] - direct[run] for run in runs)


if __name__ == "__main__":
    main()
