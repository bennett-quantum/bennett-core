"""Tables from probe runs (probe-all.sh's results.tsv; experiments/c-corpus's run-all.sh results.tsv for a
baseline).

    python3 -I summarize.py <results.tsv>                       # outcomes, refusal families, refusal reasons
    python3 -I summarize.py <results.tsv> --list <names file>   # the same, for the programs named in the file
    python3 -I summarize.py <results.tsv> --base <baseline.tsv> # also outcome transitions from the baseline
"""

import collections
import re
import sys

FP = re.compile(r"double|float|x86_fp80|constant -?[0-9.]+e[+-]|constant 0x[0-9A-FK]{10,}|`[su]itofp`|`fp|fcmp")


def load(path):
    rows = {}
    for line in open(path):
        cols = line.rstrip("\n").split("\t")
        rows[cols[0]] = cols
    return rows


def reason(detail):
    return re.sub(r", at .*", "", detail)


def family(detail):
    r = reason(detail)
    if "which has no definition" in r:
        return "no definition: @" + r.split("@", 1)[1].split(",")[0]
    if FP.search(r):
        return "floating point"
    return r


def table(title, counter):
    print(f"\n| count | {title} |\n|---:|---|")
    for key, n in counter.most_common():
        print(f"| {n} | {key} |")


def main():
    rows = load(sys.argv[1])
    args = sys.argv[2:]
    if "--list" in args:
        names = {line.strip() for line in open(args[args.index("--list") + 1]) if line.strip()}
        rows = {k: v for k, v in rows.items() if k in names}
    table("outcome", collections.Counter(v[1] for v in rows.values()))
    refused = [v for v in rows.values() if v[1] == "refused"]
    table("refusal family", collections.Counter(family(v[2]) for v in refused))
    table("refusal reason", collections.Counter(reason(v[2]) for v in refused))
    others = sorted((v[0], v[1], v[2]) for v in rows.values() if v[1] not in ("agrees", "refused", "skipped"))
    if others:
        print("\n| program | outcome | detail |\n|---|---|---|")
        for name, outcome, detail in others:
            print(f"| {name} | {outcome} | {reason(detail)} |")
    if "--base" in args:
        base = load(args[args.index("--base") + 1])
        common = [k for k in rows if k in base]
        table("baseline -> this run", collections.Counter(f"{base[k][1]} -> {rows[k][1]}" for k in common))
        table("baseline refusal family of refused -> agrees",
              collections.Counter(family(base[k][2]) for k in common
                                  if base[k][1] == "refused" and rows[k][1] == "agrees"))


main()
