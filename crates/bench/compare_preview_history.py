#!/usr/bin/env python3
"""Compare active preview caching and chat history restore across five sizes."""

import argparse
import json
import subprocess
import sys
from pathlib import Path
from statistics import median

from run_matrix import SIZES, fingerprint


def run(binary, action, mode, case, size, rounds):
    command = [str(binary), action, mode, case, size, str(rounds), "32"]
    return json.loads(subprocess.check_output(command, text=True))


def paired_timing(groups, commands, trials):
    for group in groups.values():
        group["timing_trials"] = []
    order = tuple(commands)
    for trial in range(trials):
        for name in order if trial % 2 == 0 else reversed(order):
            result = run(*commands[name])
            assert result["measurement"] == "timing"
            groups[name]["timing_trials"].append(result)
    for group in groups.values():
        samples = group["timing_trials"]
        group["timing"] = dict(samples[0])
        for key in samples[0]:
            if key == "median_ns" or key.endswith("_median_ns"):
                group["timing"][key] = median(sample[key] for sample in samples)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for version in ("before", "after"):
        for metric in ("timing", "allocations"):
            parser.add_argument(f"--{version}-{metric}", required=True, type=Path)
    parser.add_argument("--rounds", default=11, type=int)
    parser.add_argument("--trials", default=5, type=int)
    args = parser.parse_args()
    if args.rounds < 1 or args.trials < 1:
        parser.error("--rounds and --trials must be positive")
    binaries = {
        name: getattr(args, name).resolve()
        for name in ("before_timing", "before_allocations", "after_timing", "after_allocations")
    }
    rows = []
    for case in ("preview-tail", "real-chat"):
        for size in SIZES:
            groups = {}
            for version in ("before", "after"):
                timing = binaries[f"{version}_timing"]
                groups[version] = {
                    "verification": run(timing, "verify-preview", "stream", case, size, args.rounds),
                    "allocations": run(binaries[f"{version}_allocations"], "candidate-preview",
                                       "stream", case, size, args.rounds),
                }
            before, after = (groups[version]["verification"] for version in ("before", "after"))
            assert before["verified"] and after["verified"], (case, size, before, after)
            for key in ("source_fingerprint", "packet_fingerprint"):
                assert before[key] == after[key], (case, size, key, before[key], after[key])
            paired_timing(groups, {
                version: (binaries[f"{version}_timing"], "candidate-preview", "stream",
                          case, size, args.rounds)
                for version in groups
            }, args.trials)
            assert all(sample["source_fingerprint"] == before["source_fingerprint"]
                       for group in groups.values()
                       for sample in [group["allocations"], *group["timing_trials"]])
            rows.append({"workload": "preview", "case": case, "size": size, **groups})
            print(f"VERIFIED preview {case}/{size}", file=sys.stderr)
    for size in SIZES:
        timing = binaries["after_timing"]
        verified = run(timing, "verify-history", "full", "preview-tail", size, args.rounds)
        assert verified["verified"], verified
        groups = {}
        for policy in ("eager", "batched"):
            action = f"history-{policy}"
            groups[policy] = {
                "allocations": run(binaries["after_allocations"], action, "full", "preview-tail",
                                   size, args.rounds),
            }
        paired_timing(groups, {
            policy: (timing, f"history-{policy}", "full", "preview-tail", size, args.rounds)
            for policy in groups
        }, args.trials)
        assert all(sample["source_fingerprint"] == verified["source_fingerprint"]
                   for group in groups.values()
                   for sample in [group["allocations"], *group["timing_trials"]])
        rows.append({"workload": "history", "size": size, "verification": verified, **groups})
        print(f"VERIFIED history {size}", file=sys.stderr)
    print(json.dumps({"binaries": {key: fingerprint(path) for key, path in binaries.items()},
                      "cases": rows}, indent=2))


if __name__ == "__main__":
    main()
