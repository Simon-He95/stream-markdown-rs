#!/usr/bin/env python3
"""Verify and compare separately built timing and allocation benchmark binaries."""

import argparse
import hashlib
import json
import math
import os
import platform
import subprocess
import sys
import time

if os.name == "posix":
    import resource
from pathlib import Path


CASES = (
    "plain", "mixed", "long-paragraph", "open-fence", "table", "list",
    "late-reference", "real-chat",
)
SIZES = ("small", "medium", "large", "huge", "very-huge")
MODES = ("full", "stream")
TIMING_FIELDS = (
    "median_ns", "first_append_median_ns", "first_output_median_ns",
    "append_p95_median_ns", "max_append_median_ns",
)
MEMORY_FIELDS = ("peak_live_bytes_delta", "live_at_finish_bytes_delta", "total_requested_bytes")


def positive_integer(text):
    value = int(text)
    if value <= 0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return value


def positive_ratio(text):
    value = float(text)
    if not math.isfinite(value) or value <= 0:
        raise argparse.ArgumentTypeError("must be finite and greater than zero")
    return value


def run(binary, action, mode, corpus, size, args):
    command = [str(binary), action, mode, corpus, size, str(args.rounds)]
    if args.chunk_bytes is not None:
        command.append(str(args.chunk_bytes))
    cpu_before = resource.getrusage(resource.RUSAGE_CHILDREN) if os.name == "posix" else None
    began = time.perf_counter_ns()
    child = subprocess.run(command, capture_output=True, text=True, timeout=args.timeout)
    worker_elapsed = time.perf_counter_ns() - began
    cpu_after = resource.getrusage(resource.RUSAGE_CHILDREN) if cpu_before is not None else None
    if child.returncode:
        raise ValueError(f"{binary.name}/{action}: {child.stderr.strip() or child.returncode}")
    result = json.loads(child.stdout)
    expected = {"engine": action, "mode": mode, "case": corpus, "size": size}
    if any(result.get(key) != value for key, value in expected.items()):
        raise ValueError(f"{binary.name}/{action} emitted a result for a different workload")
    result["worker_wall_ns"] = worker_elapsed
    if cpu_after is not None:
        cpu = ((cpu_after.ru_utime - cpu_before.ru_utime)
               + (cpu_after.ru_stime - cpu_before.ru_stime))
        result["worker_cpu_ns"] = round(cpu * 1_000_000_000)
    return result


def compare(reference, candidate, fields, minimum, allow_zero=False):
    ratios = {}
    failures = []
    for field in fields:
        if field not in reference or field not in candidate:
            raise ValueError(f"sample is missing required metric {field}")
        left, right = reference[field], candidate[field]
        ratios[field] = left / right if left >= 0 and right > 0 else None
        inconclusive = left <= 0 or right < 0 or (right == 0 and not allow_zero)
        if minimum is not None and (inconclusive or left < minimum * right):
            failures.append(field)
    return ratios, failures


def check_sample(sample, verification, measurement):
    if sample.get("measurement") != measurement:
        raise ValueError(f"expected a {measurement} binary")
    fields = (
        "source_bytes", "target_bytes", "source_fingerprint", "commits",
        "chunk_bytes", "prefix_replay_bytes", "pulldown_version",
    )
    if any(key not in sample or key not in verification or sample[key] != verification[key] for key in fields):
        raise ValueError("sample and verification measured different inputs")


def collect(mode, corpus, size, args, reverse):
    row = {"mode": mode, "case": corpus, "size": size, "ok": False}
    candidate = "candidate-borrowed" if args.borrowed_source else "candidate"
    verifier = "verify-borrowed" if args.borrowed_source else "verify"
    try:
        verification = run(args.timing_binary, verifier, mode, corpus, size, args)
        row["verification"] = verification
        if not verification.get("verified"):
            return row

        engines = ["pulldown", candidate]
        primary = "pulldown"
        if mode == "stream" and not args.borrowed_source:
            primary = "pulldown-owned"
            engines.insert(1, primary)
        if reverse:
            engines.reverse()
        timing = {
            engine: run(args.timing_binary, engine, mode, corpus, size, args)
            for engine in engines
        }
        memory = {
            engine: run(args.memory_binary, engine, mode, corpus, size, args)
            for engine in engines
        }
        for engine in engines:
            check_sample(timing[engine], verification, "timing")
            check_sample(memory[engine], verification, "allocations")
        timing_fields = TIMING_FIELDS if mode == "stream" else ("median_ns",)
        borrowed_timing, _ = compare(
            timing["pulldown"], timing[candidate], timing_fields, None,
        )
        borrowed_memory, _ = compare(
            memory["pulldown"], memory[candidate], MEMORY_FIELDS, None,
        )
        primary_timing, slow = compare(
            timing[primary], timing[candidate], timing_fields, args.min_speedup,
        )
        primary_memory, memory_failures = compare(
            memory[primary], memory[candidate], MEMORY_FIELDS, args.min_memory_ratio,
            allow_zero=True,
        )
        row.update(
            ok=not slow and not memory_failures,
            gate_reference=primary,
            timing=timing,
            memory=memory,
            pulldown_over_candidate_timing=borrowed_timing,
            pulldown_over_candidate_memory=borrowed_memory,
            timing_failures=slow,
            memory_failures=memory_failures,
        )
        if primary == "pulldown-owned":
            row["pulldown_owned_over_candidate_timing"] = primary_timing
            row["pulldown_owned_over_candidate_memory"] = primary_memory
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        row["error"] = str(error)
    return row


def fingerprint(path):
    relative = path.relative_to(Path.cwd()) if path.is_relative_to(Path.cwd()) else path.name
    with path.open("rb") as binary:
        return {"path": str(relative), "sha256": hashlib.file_digest(binary, "sha256").hexdigest()}


def progress_details(row):
    if "error" in row:
        return row["error"]
    if "gate_reference" not in row:
        return "semantic verification failed"
    reference = row["gate_reference"].replace("-", "_")
    timing = row[f"{reference}_over_candidate_timing"]
    memory = row[f"{reference}_over_candidate_memory"]
    fields = [
        ("total", timing["median_ns"]),
        ("first-append", timing.get("first_append_median_ns")),
        ("first-output", timing.get("first_output_median_ns")),
        ("append-p95", timing.get("append_p95_median_ns")),
        ("peak", memory["peak_live_bytes_delta"]),
    ]
    details = [f"{name}={ratio:.2f}x" for name, ratio in fields if ratio is not None]
    failures = row["timing_failures"] + row["memory_failures"]
    if failures:
        details.append(f"below threshold: {', '.join(failures)}")
    return " ".join(details)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timing-binary", type=Path, required=True)
    parser.add_argument("--memory-binary", type=Path, required=True)
    parser.add_argument("--case", action="append", choices=CASES)
    parser.add_argument("--size", action="append", choices=SIZES)
    parser.add_argument("--mode", action="append", choices=MODES)
    parser.add_argument("--rounds", type=positive_integer, default=5)
    parser.add_argument("--chunk-bytes", type=positive_integer)
    parser.add_argument("--timeout", type=positive_integer, default=180)
    parser.add_argument("--revision", help="revision or source snapshot label for these binaries")
    parser.add_argument("--min-speedup", type=positive_ratio)
    parser.add_argument("--min-memory-ratio", type=positive_ratio)
    parser.add_argument("--borrowed-source", action="store_true")
    args = parser.parse_args()
    if args.borrowed_source and args.mode != ["stream"]:
        parser.error("--borrowed-source requires --mode stream")
    args.timing_binary = args.timing_binary.resolve()
    args.memory_binary = args.memory_binary.resolve()
    environment = {
        "os": platform.system(),
        "os_release": platform.release(),
        "arch": platform.machine(),
        "processor": platform.processor(),
        "timing_binary": fingerprint(args.timing_binary),
        "memory_binary": fingerprint(args.memory_binary),
    }

    rows = []
    for corpus in args.case or CASES:
        for size in args.size or SIZES:
            modes = args.mode or MODES
            for mode_index, mode in enumerate(modes):
                reverse = bool((len(rows) // len(modes) + mode_index) % 2)
                row = collect(mode, corpus, size, args, reverse)
                rows.append(row)
                status = "MEASURED" if row["ok"] else "FAIL"
                if row["ok"] and (args.min_speedup is not None or args.min_memory_ratio is not None):
                    status = "PASS"
                print(f"{status} {corpus}/{size}/{mode} {progress_details(row)}",
                      file=sys.stderr, flush=True)
    report = {
        "revision": args.revision,
        "borrowed_candidate": args.borrowed_source,
        "environment": environment,
        "thresholds": {
            "min_speedup": args.min_speedup,
            "min_memory_ratio": args.min_memory_ratio,
        },
        "cases": rows,
    }
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if all(row["ok"] for row in rows) else 1


if __name__ == "__main__":
    sys.exit(main())
