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
        ownerships = ["generated"]
        if any(case.startswith("borrowed-") for _, case in medians):
            ownerships.append("borrowed")
        for ownership in ownerships:
            for shape in ("scalars", "blobs"):
                write_incremental(writer, medians, by_run, ownership, shape)
        if ("cpp", "direct-builder-scalars") in medians:
            for shape in ("scalars", "blobs", "struct", "list"):
                direct_case = f"direct-builder-{shape}"
                generated_case = f"generated-builder-{shape}"
                if ("cpp", direct_case) in medians:
                    write_incremental_cases(
                        writer,
                        medians,
                        by_run,
                        f"builder-{shape}",
                        direct_case,
                        generated_case,
                    )


def write_incremental_cases(
    writer: object,
    medians: dict[tuple[str, str], float],
    by_run: dict[tuple[str, str, str], float],
    label: str,
    direct_case: str,
    generated_case: str,
) -> None:
    cpp_incremental = paired_case_median(by_run, "cpp", direct_case, generated_case)
    native_incremental = paired_case_median(by_run, "native", direct_case, generated_case)
    cpp_direct = medians[("cpp", direct_case)]
    native_direct = medians[("native", direct_case)]
    cpp_generated = medians[("cpp", generated_case)]
    native_generated = medians[("native", generated_case)]
    ratio = (
        f"{native_incremental / cpp_incremental:.3f}"
        if cpp_incremental > 0 and native_incremental >= 0
        else "below-resolution"
    )
    writer.writerow(  # type: ignore[attr-defined]
        [
            label,
            f"{cpp_incremental:.4f}",
            f"{native_incremental:.4f}",
            ratio,
            f"{native_direct / cpp_direct:.3f}",
            f"{native_generated / cpp_generated:.3f}",
        ]
    )


def paired_case_median(
    samples: dict[tuple[str, str, str], float],
    implementation: str,
    direct_case: str,
    generated_case: str,
) -> float:
    generated = {
        run: value
        for (impl, case, run), value in samples.items()
        if impl == implementation and case == generated_case
    }
    direct = {
        run: value
        for (impl, case, run), value in samples.items()
        if impl == implementation and case == direct_case
    }
    runs = sorted(generated.keys() & direct.keys(), key=int)
    if not runs:
        raise SystemExit(f"no paired {generated_case} samples for {implementation}")
    return statistics.median(generated[run] - direct[run] for run in runs)


def write_incremental(
    writer: object,
    medians: dict[tuple[str, str], float],
    by_run: dict[tuple[str, str, str], float],
    ownership: str,
    shape: str,
) -> None:
    direct_case = "direct"
    if ownership == "borrowed" and ("cpp", f"borrowed-direct-{shape}") in medians:
        direct_case = "borrowed-direct"
    cpp_incremental = paired_median(by_run, "cpp", ownership, direct_case, shape)
    native_incremental = paired_median(by_run, "native", ownership, direct_case, shape)
    cpp_direct = medians[("cpp", f"{direct_case}-{shape}")]
    native_direct = medians[("native", f"{direct_case}-{shape}")]
    cpp_generated = medians[("cpp", f"{ownership}-{shape}")]
    native_generated = medians[("native", f"{ownership}-{shape}")]
    ratio = (
        f"{native_incremental / cpp_incremental:.3f}"
        if cpp_incremental > 0 and native_incremental >= 0
        else "below-resolution"
    )
    writer.writerow(  # type: ignore[attr-defined]
        [
            f"{ownership}-{shape}",
            f"{cpp_incremental:.4f}",
            f"{native_incremental:.4f}",
            ratio,
            f"{native_direct / cpp_direct:.3f}",
            f"{native_generated / cpp_generated:.3f}",
        ]
    )


def paired_median(
    samples: dict[tuple[str, str, str], float],
    implementation: str,
    ownership: str,
    direct_case: str,
    shape: str,
) -> float:
    generated = {
        run: value
        for (impl, case, run), value in samples.items()
        if impl == implementation and case == f"{ownership}-{shape}"
    }
    direct = {
        run: value
        for (impl, case, run), value in samples.items()
        if impl == implementation and case == f"{direct_case}-{shape}"
    }
    runs = sorted(generated.keys() & direct.keys(), key=int)
    if not runs:
        raise SystemExit(f"no paired {shape} samples for {implementation}")
    return statistics.median(generated[run] - direct[run] for run in runs)


if __name__ == "__main__":
    main()
