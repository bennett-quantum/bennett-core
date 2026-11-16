#!/usr/bin/env python3
"""Count retained executed-site TSVs; optionally compare two or list LLVM target guards.

TSV columns: function, block, site (zero-based, terminator last), count, operation
(the Debug instruction from the scratch corpus profiler). No timing is inferred.
"""
import argparse
import collections
import csv
import re
from pathlib import Path


def read_profile(path):
    totals = collections.defaultdict(collections.Counter)
    for row in csv.DictReader(path.open(), delimiter="\t"):
        n = int(row["count"])
        op = row["operation"]
        counts = totals[row["function"]]
        counts["steps"] += n
        kind = op.split(" {", 1)[0].split("(", 1)[0]
        counts[kind] += n
        if op.startswith("Compare { pred: Eq"):
            counts["eq"] += n
        if op.startswith("CallIndirect"):
            params, result = op.split("params: [", 1)[1].split("], result:", 1)
            widths = [int(w) for w in re.findall(r"Width\((\d+)\)", params)]
            counts["argument_words"] += n * sum((w + 63) // 64 for w in widths)
            m = re.search(r"Some\(Int\(Width\((\d+)\)", result)
            counts["result_words"] += n * ((int(m[1]) + 63) // 64 if m else 0)
    return totals


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("profile", type=Path)
    parser.add_argument("--before", type=Path)
    parser.add_argument("--llvm", type=Path, help="--emit-llvm output: static candidate groups, including unexecuted ones")
    args = parser.parse_args()
    after = read_profile(args.profile)
    before = read_profile(args.before) if args.before else {}
    keys = ["steps", "eq", "Compare", "Branch", "Jump", "Select", "Binary", "CallIndirect",
            "argument_words", "result_words"]
    print("function\t" + "\t".join(keys))
    total = collections.Counter()
    for name in sorted(after.keys() | before.keys()):
        counts = collections.Counter({k: after.get(name, {}).get(k, 0) - before.get(name, {}).get(k, 0)
                                      for k in keys})
        total.update(counts)
        print(name + "\t" + "\t".join(str(counts[k]) for k in keys))
    print("TOTAL\t" + "\t".join(str(total[k]) for k in keys))
    if args.llvm:
        print("\nfunction\ttarget\tstatic_candidates")
        for part in args.llvm.read_text().split("\ndefine "):
            name = re.search(r"@([^(]+)\(", part.splitlines()[0])
            if not name:
                continue
            groups = collections.defaultdict(set)
            patterns = [r"icmp eq ptr ([%@][\w.]+), @([\w.]+)",
                        r"icmp eq \(ptr (@[\w.]+), ptr @([\w.]+)\)"]
            for pattern in patterns:
                for target, candidate in re.findall(pattern, part):
                    groups[target].add(candidate)
            for target, candidates in groups.items():
                print(f"{name[1]}\t{target}\t{len(candidates)}")


if __name__ == "__main__":
    main()
