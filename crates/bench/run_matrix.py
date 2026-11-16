#!/usr/bin/env python3
"""Verify and compare separately built timing and allocation benchmark binaries."""

import argparse
import hashlib
import json
import math
import platform
import subprocess
import sys
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
    child = subprocess.run(command, capture_output=True, text=True, timeout=args.timeout)
    if child.returncode:
        raise ValueError(f"{binary.name}/{action}: {child.stderr.strip() or child.returncode}")
    result = json.loads(child.stdout)
    expected = {"engine": action, "mode": mode, "case": corpus, "size": size}
    if any(result.get(key) != value for key, value in expected.items()):
        raise ValueError(f"{binary.name}/{action} emitted a result for a different workload")
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


def collect(mode, corpus, size, args):
    row = {"mode": mode, "case": corpus, "size": size, "ok": False}
    try:
        verification = run(args.timing_binary, "verify", mode, corpus, size, args)
        row["verification"] = verification
        if not verification.get("verified"):
            return row

        engines = ["pulldown", "candidate"]
        primary = "pulldown"
        if mode == "stream":
            primary = "pulldown-owned"
            engines.insert(1, primary)
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
            timing["pulldown"], timing["candidate"], timing_fields, None,
        )
        borrowed_memory, _ = compare(
            memory["pulldown"], memory["candidate"], MEMORY_FIELDS, None,
        )
        primary_timing, slow = compare(
            timing[primary], timing["candidate"], timing_fields, args.min_speedup,
        )
        primary_memory, memory_failures = compare(
            memory[primary], memory["candidate"], MEMORY_FIELDS, args.min_memory_ratio,
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
        if mode == "stream":
            row["pulldown_owned_over_candidate_timing"] = primary_timing
            row["pulldown_owned_over_candidate_memory"] = primary_memory
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        row["error"] = str(error)
    return row


def fingerprint(path):
    with path.open("rb") as binary:
        return {"path": str(path), "sha256": hashlib.file_digest(binary, "sha256").hexdigest()}


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
    args = parser.parse_args()
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
            for mode in args.mode or MODES:
                row = collect(mode, corpus, size, args)
                rows.append(row)
                status = "MEASURED" if row["ok"] else "FAIL"
                if row["ok"] and (args.min_speedup is not None or args.min_memory_ratio is not None):
                    status = "PASS"
                print(f"{status} {corpus}/{size}/{mode}", file=sys.stderr, flush=True)
    report = {
        "revision": args.revision,
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
