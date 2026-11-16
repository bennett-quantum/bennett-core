"""Compare every program's outcome, reason and steps between two run-all.sh tables.

    python3 compare.py old/results.tsv new/results.tsv > changes.tsv

Diagnostic source locations are omitted from reasons: moving an exported source
root is not a change in a refusal. Both tables must contain the same program set.
The output includes all programs, including unchanged ones.
"""
import argparse
import collections
import csv
import sys


def read(path):
    with open(path, newline="") as source:
        rows = list(csv.reader(source, delimiter="\t"))
    if any(len(row) != 4 for row in rows):
        raise ValueError(f"{path}: expected four columns per program")
    result = {row[0]: row[1:] for row in rows}
    if len(result) != len(rows):
        raise ValueError(f"{path}: duplicate program names")
    return result


def reason(detail):
    return detail.split(", at ", 1)[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("old")
    parser.add_argument("new")
    args = parser.parse_args()
    old, new = read(args.old), read(args.new)
    if old.keys() != new.keys():
        parser.error(f"program sets differ: removed {sorted(old.keys() - new.keys())}; "
                     f"added {sorted(new.keys() - old.keys())}")
    writer = csv.writer(sys.stdout, delimiter="\t", lineterminator="\n")
    writer.writerow(["program", "change", "old_outcome", "new_outcome", "old_steps", "new_steps",
                     "old_reason", "new_reason"])
    changes = collections.Counter()
    for name in sorted(old):
        a, b = old[name], new[name]
        kinds = []
        if a[0] != b[0]:
            kinds.append("outcome")
        elif reason(a[1]) != reason(b[1]):
            kinds.append("reason")
        if a[2] != b[2] and a[0] == b[0]:
            kinds.append("steps")
        kind = "+".join(kinds) or "unchanged"
        changes[kind] += 1
        writer.writerow([name, kind, a[0], b[0], a[2], b[2], reason(a[1]), reason(b[1])])
    print(f"{len(new)} programs: {dict(sorted(changes.items()))}", file=sys.stderr)
    print(f"old outcomes: {dict(sorted(collections.Counter(r[0] for r in old.values()).items()))}", file=sys.stderr)
    print(f"new outcomes: {dict(sorted(collections.Counter(r[0] for r in new.values()).items()))}", file=sys.stderr)


if __name__ == "__main__":
    main()
