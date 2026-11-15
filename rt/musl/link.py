#!/usr/bin/env python3
"""Compile one C program and resolve musl archive members, then emit its device program."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def run(args, **kwargs):
    return subprocess.run([str(a) for a in args], check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("-o", required=True, type=Path)
    parser.add_argument("--libdir", type=Path, default=ROOT / "target/musl")
    parser.add_argument("--front", type=Path, default=ROOT / "target/front/bennett-front")
    parser.add_argument("--arena-bytes", type=int, default=1 << 20)
    parser.add_argument("--stack-bytes", type=int, default=1 << 20)
    parser.add_argument("--region-budget", type=int, default=100000)
    parser.add_argument("--clang-flag", action="append", default=[])
    parser.add_argument("--keep", type=Path, help="retain intermediate files in this directory")
    args = parser.parse_args()
    lib = args.libdir.resolve()
    args.o = args.o.resolve()
    args.o.parent.mkdir(parents=True, exist_ok=True)
    clang = os.environ.get("CLANG", "clang-18")
    flags = json.loads((lib / "cflags.json").read_text())
    if args.arena_bytes < 4096 or args.arena_bytes % 4096:
        parser.error("--arena-bytes must be a positive whole number of 4096-byte pages")
    if not (lib / "index.tsv").exists():
        run(["python3", HERE / "resolve.py", lib / "members.txt", lib / "index.tsv"])
    temporary = tempfile.TemporaryDirectory(dir=args.o.parent) if args.keep is None else None
    scratch = Path(temporary.name) if temporary else args.keep.resolve()
    scratch.mkdir(parents=True, exist_ok=True)
    common = ["-O0", "-Xclang", "-disable-O0-optnone", "-gline-tables-only", "-emit-llvm", "-c"]
    run([clang, *common, "-nostdinc", "-isystem", lib / "sysroot/include", "-I", ROOT / "front/include",
         *args.clang_flag, args.source.resolve(), "-o", scratch / "program.bc"])
    ir = run(["llvm-dis-18", scratch / "program.bc", "-o", "-"], capture_output=True, text=True).stdout
    match = re.search(r"^define\b[^\n]* @main\(([^\n]*)\)", ir, re.M)
    if not match or "..." in match[1]:
        parser.error("the program needs a defined nonvariadic int main")
    count = 0 if not match[1].strip() else len(match[1].split(","))
    if count > 3:
        parser.error("main has more than three parameters")
    for source, output, extra in [("runtime.c", "runtime.bc", [f"-DBENNETT_ARENA_BYTES={args.arena_bytes}"]),
                                  ("start.c", "start.bc", [f"-DMAIN_ARGS={count}"])]:
        run([clang, *flags, *common, *extra, HERE / source, "-o", scratch / output])
    inputs = [scratch / name for name in ["program.bc", "runtime.bc", "start.bc"]]
    roots = []
    for _ in range(16):
        selected = run(["python3", HERE / "resolve.py", lib / "members.txt", lib / "index.tsv", *inputs, *roots],
                       capture_output=True, text=True).stdout.splitlines()
        run(["llvm-link-18", *inputs, *selected, "-o", scratch / "linked.bc"])
        run(["llvm-dis-18", scratch / "linked.bc", "-o", scratch / "linked.ll"])
        run(["python3", HERE / "weak.py", scratch / "linked.ll", scratch / "linked.ll"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        run(["llvm-as-18", scratch / "linked.ll", "-o", scratch / "linked.bc"])
        result = subprocess.run([str(args.front.resolve()), str(scratch / "linked.bc"), "-o", str(args.o),
                                 "--stack-bytes", str(args.stack_bytes), "--region-budget", str(args.region_budget)],
                                capture_output=True, text=True)
        if result.returncode == 0:
            return
        missing = re.search(r"^not handled yet: a call to @([^,]+), which has no definition", result.stderr, re.M)
        index = (lib / "index.tsv").read_text()
        if missing and missing[1] not in roots and re.search(r"\t(?:strong|weak)\t" + re.escape(missing[1]) + r"$", index, re.M):
            roots += ["-u", missing[1]]
            continue
        raise SystemExit(result.stderr)
    raise SystemExit("library resolution did not settle after 16 rounds")


if __name__ == "__main__":
    main()
