"""Size and cost of probe runs: for each program directory a probe run left (<work dir>/out/<name>), the device
program's functions, instructions, bytes (all, and in `data` lines), the data and memory sizes it declares, device
steps, bennett-front wall seconds and peak RSS (from result.tsv), and the executor's wall seconds and peak RSS
(bennett run, timed again here).

    python3 -I cost.py <bennett binary> <work dir> [name ...]   # markdown table on stdout
"""

import os
import re
import subprocess
import sys


def run_time(bennett, bn):
    t = subprocess.run(["/usr/bin/time", "-f", "%e %M", bennett, "run", "--entry", "__bennett_entry", "--budget",
                        "1000000000", bn], capture_output=True, text=True)
    sec, kib = t.stderr.strip().splitlines()[-1].split()
    return float(sec), int(kib)


def main():
    bennett, work = sys.argv[1], sys.argv[2]
    names = sys.argv[3:] or sorted(os.listdir(os.path.join(work, "out")))
    print("| program | outcome | functions | instructions | .bn bytes | of which data lines | data bytes declared |"
          " memory | steps | front s | front MiB | run s | run MiB |")
    print("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    for name in names:
        d = os.path.join(work, "out", name)
        row = open(os.path.join(d, "result.tsv")).read().rstrip("\n").split("\t")
        bn = os.path.join(d, "prog-A.bn")
        if not os.path.exists(bn) or row[1] in ("refused", "crash", "timeout"):
            print(f"| {name} | {row[1]} | | | | | | | | {row[4]} | {int(row[5] or 0) // 1024} | | |")
            continue
        text = open(bn).read()
        lines = text.splitlines()
        data_text = sum(len(l) + 1 for l in lines if l.startswith("data "))
        declared = 0
        for l in lines:
            if l.startswith("data "):
                body = l.split("=", 1)[1].strip()
                if body.startswith('"'):
                    declared += len(re.sub(r"\\x[0-9a-fA-F]{2}|\\.", "x", body[1:-1]))
        memory = next(int(l.split()[1]) for l in lines if l.startswith("memory "))
        funcs = sum(1 for l in lines if l.startswith("func "))
        insts = sum(1 for l in lines if l.startswith("    "))
        sec, kib = run_time(bennett, bn)
        print(f"| {name} | {row[1]} | {funcs} | {insts} | {len(text.encode())} | {data_text} | {declared} | {memory} |"
              f" {row[3]} | {row[4]} | {int(row[5]) // 1024} | {sec} | {kib // 1024} |")


main()
