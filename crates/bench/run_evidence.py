#!/usr/bin/env python3
"""Produce the Markdown benchmark report's raw data without parallel workers."""

import argparse
import json
from pathlib import Path
import subprocess
import sys

from run_matrix import CASES, SIZES, fingerprint, run


def matrix(binary, allocation, revision, output, options):
    command = [
        sys.executable, str(Path(__file__).with_name("run_matrix.py")),
        "--timing-binary", str(binary), "--memory-binary", str(allocation),
        "--revision", revision, *options,
    ]
    with output.open("w") as file:
        return subprocess.run(command, stdout=file, check=False).returncode == 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timing-binary", type=Path, required=True)
    parser.add_argument("--memory-binary", type=Path, required=True)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.timing_binary = args.timing_binary.resolve()
    args.memory_binary = args.memory_binary.resolve()
    args.output.mkdir(parents=True, exist_ok=True)

    scale = [
        (("small", "medium", "large"), "11"),
        (("huge",), "5"),
        (("very-huge",), "3"),
    ]
    suites = [(f"scale-{sizes[-1]}", [
        "--case", "mixed", "--case", "real-chat", "--rounds", rounds,
        *[item for size in sizes for item in ("--size", size)],
    ]) for sizes, rounds in scale]
    suites.extend([
        ("shapes-medium", ["--size", "medium", "--rounds", "11",
                           *[item for case in CASES if case not in ("mixed", "real-chat")
                             for item in ("--case", case)]]),
        ("borrowed-tiny-chunks", ["--size", "medium", "--mode", "stream", "--case", "real-chat",
                                  "--borrowed-source", "--chunk-bytes", "32", "--rounds", "11"]),
    ])
    success = True
    for name, options in suites:
        passed = matrix(args.timing_binary, args.memory_binary, args.revision,
                        args.output / f"{name}.json", options)
        success = passed and success

    args.timeout = 180
    previews = []
    args.rounds, args.chunk_bytes = 11, 32
    for case, size in (("real-chat", "large"), ("preview-tail", "huge")):
        verification = run(args.timing_binary, "verify-preview", "stream", case, size, args)
        timing = run(args.timing_binary, "candidate-preview", "stream", case, size, args)
        memory = run(args.memory_binary, "candidate-preview", "stream", case, size, args)
        previews.append({"verification": verification, "timing": timing, "memory": memory})
        success = bool(verification["verified"]) and success
        print(f"preview {case}/{size}: verified={verification['verified']}", file=sys.stderr, flush=True)

    history = []
    args.chunk_bytes = None
    for size in SIZES:
        args.rounds = 11 if size in ("small", "medium", "large") else 5
        verification = run(args.timing_binary, "verify-history", "full", "preview-tail", size, args)
        engines = ["history-eager", "history-batched"]
        if len(history) % 2:
            engines.reverse()
        timing = {engine: run(args.timing_binary, engine, "full", "preview-tail", size, args)
                  for engine in engines}
        memory = {engine: run(args.memory_binary, engine, "full", "preview-tail", size, args)
                  for engine in engines}
        history.append({"verification": verification, "timing": timing, "memory": memory})
        success = bool(verification["verified"]) and success
        print(f"history {size}: verified={verification['verified']}", file=sys.stderr, flush=True)

    soaks = []
    args.rounds, args.chunk_bytes = 1000, 32
    for mode, engine in (("full", "soak"), ("stream", "soak"), ("stream", "soak-preview")):
        result = run(args.memory_binary, engine, mode, "real-chat", "medium", args)
        soaks.append(result)
        success = all(result[key] == 0 for key in ("min_after_drop_bytes_delta",
                                                  "max_after_drop_bytes_delta",
                                                  "live_after_drop_bytes_delta")) and success
        print(f"soak {engine}/{mode}: {result['live_after_drop_bytes_delta']} B after drop",
              file=sys.stderr, flush=True)

    report = {"revision": args.revision, "timing_binary": fingerprint(args.timing_binary),
              "memory_binary": fingerprint(args.memory_binary), "previews": previews,
              "history": history, "soaks": soaks}
    (args.output / "preview-history-soak.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if success else 1


if __name__ == "__main__":
    sys.exit(main())
