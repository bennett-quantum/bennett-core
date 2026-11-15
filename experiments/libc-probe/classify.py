"""Classifies the bitcode musl-bc.sh wrote: per source file, defined functions, inline-assembly call sites by kind,
and instructions that use a floating-point type, inside function bodies.

    python3 classify.py <out dir>          # per-file TSV on stdout
    python3 classify.py <out dir> --table  # the summary tables (markdown)
"""

import collections
import os
import re
import subprocess
import sys

ASM = re.compile(r'\basm (?:sideeffect )?(?:alignstack )?(?:inteldialect )?(?:unwind )?"((?:[^"\\]|\\.)*)"')
FP = re.compile(r"\b(half|bfloat|float|double|x86_fp80|fp128)\b")
KINDS = ["empty", "syscall", "atomic", "thread pointer", "x87/SSE floating point", "other"]


def asm_kind(template):
    t = template.strip()
    if not t:
        return "empty"
    if "syscall" in t:
        return "syscall"
    if "%fs:" in t or "fs:0" in t:
        return "thread pointer"
    if re.search(r"lock|xchg|cmpxchg|mfence|pause|xadd", t):
        return "atomic"
    if re.search(r"\bf[a-z0-9]+\b|sqrt|ldmxcsr|stmxcsr|fnstcw|fldcw|cvt|movs[sd]", t):
        return "x87/SSE floating point"
    return "other"


def analyse(path):
    text = subprocess.run(["llvm-dis-18", path, "-o", "-"], capture_output=True, text=True, check=True).stdout
    defs, kinds, fp, inside = 0, collections.Counter(), 0, False
    others = []
    for line in text.splitlines():
        if line.startswith("define "):
            defs += 1
            inside = True
            continue
        if line.startswith("}"):
            inside = False
            continue
        if not inside:
            continue
        for template in ASM.findall(line):
            kind = asm_kind(template)
            kinds[kind] += 1
            if kind == "other":
                others.append(template)
        if FP.search(line.split(", !dbg")[0]):
            fp += 1
    return defs, kinds, fp, others


def rows(out):
    for line in open(os.path.join(out, "compile.tsv")):
        source, status, _ = line.rstrip("\n").split("\t", 2)
        if status != "ok":
            yield source, status, None
            continue
        yield source, status, analyse(os.path.join(out, "bc", source + ".bc"))


def main():
    out = sys.argv[1]
    if "--table" not in sys.argv:
        print("source\tstatus\tdefined\t" + "\t".join("asm " + k for k in KINDS) + "\tfp instructions\tother asm")
        for source, status, result in rows(out):
            if result is None:
                print(f"{source}\t{status}\t" + "\t" * (len(KINDS) + 1))
                continue
            defs, kinds, fp, others = result
            print(f"{source}\t{status}\t{defs}\t" + "\t".join(str(kinds[k]) for k in KINDS) +
                  f"\t{fp}\t{' | '.join(sorted(set(others)))[:200]}")
        return
    table = [line.rstrip("\n").split("\t") for line in open(os.path.join(out, "classify.tsv"))][1:]
    status = collections.Counter(r[1] for r in table)
    print("| status | files |\n|---|---:|")
    for k, n in status.most_common():
        print(f"| {k} | {n} |")
    ok = [r for r in table if r[1] == "ok"]
    print("\n| inline assembly kind | files | call sites |\n|---|---:|---:|")
    for i, k in enumerate(KINDS):
        col = [int(r[3 + i]) for r in ok]
        print(f"| {k} | {sum(1 for c in col if c)} | {sum(col)} |")
    nonempty = [r for r in ok if any(int(r[3 + i]) for i in range(1, len(KINDS)))]
    fp = [r for r in ok if int(r[3 + len(KINDS)])]
    clean = [r for r in ok if r not in nonempty and r not in fp]
    print(f"\nfiles with non-empty inline assembly: {len(nonempty)}; with floating point: {len(fp)}; "
          f"with neither: {len(clean)} of {len(ok)} compiled")
    by_dir = collections.defaultdict(lambda: [0, 0, 0, 0])
    for r in ok:
        d = r[0].split("/")[1]
        by_dir[d][0] += 1
        by_dir[d][1] += r in nonempty
        by_dir[d][2] += r in fp
        by_dir[d][3] += r in clean
    print("\n| directory | compiled | non-empty asm | floating point | neither |\n|---|---:|---:|---:|---:|")
    for d, (a, b, c, e) in sorted(by_dir.items(), key=lambda x: -x[1][0]):
        print(f"| {d} | {a} | {b} | {c} | {e} |")


main()
