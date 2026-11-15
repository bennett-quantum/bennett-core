"""Picks the archive members a static linker would pull for some bitcode files, and prints their paths.

    python3 -I resolve.py <members.txt> <index.tsv> <input.bc>... [-u symbol]...

members.txt lists the library's member bitcode files in archive order. index.tsv caches their symbol tables
(written on first use). As GNU ld does with an archive: a member is pulled when it defines (strongly or weakly) a
symbol that is undefined so far; an undefined weak reference pulls nothing; repeat until nothing changes. Among
members defining the symbol, the first with a strong definition wins, else the first with a weak one. `-u` adds an
undefined symbol (a call LLVM's optimiser creates later, e.g. puts from printf). Unresolved symbols go to stderr.

Why not llvm-link --only-needed of one whole-library module: musl gives many symbols a weak dummy definition in
one file and a strong one in another that is linked only when needed (__syscall_cp, __stdio_exit, ...); merging
every member first makes the strong one win always and pulls in what the program never asked for.
"""

import os
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor


def symbols(path):
    out = subprocess.run(["llvm-nm-18", path], capture_output=True, text=True, check=True).stdout
    rows = []
    for line in out.splitlines():
        parts = line.split()
        if len(parts) < 2:
            continue
        kind, name = parts[-2], parts[-1]
        if kind == "U":
            rows.append(("undef", name))
        elif kind in ("W", "V"):
            rows.append(("weak", name))
        elif kind in ("T", "D", "B", "R", "C"):
            rows.append(("strong", name))
        # lower case: local symbols, or an undefined weak reference ('w'): neither pulls a member
    return rows


def load_index(members, index_path):
    if not os.path.exists(index_path):
        with ThreadPoolExecutor(32) as pool:
            tables = list(pool.map(symbols, members))
        # Several links may build the cache at once: each writes its own file and replaces atomically.
        fd, part = tempfile.mkstemp(dir=os.path.dirname(os.path.abspath(index_path)), suffix=".part")
        with os.fdopen(fd, "w") as f:
            for member, rows in zip(members, tables):
                for kind, name in rows:
                    f.write(f"{member}\t{kind}\t{name}\n")
        os.replace(part, index_path)
    table = {}
    for line in open(index_path):
        member, kind, name = line.rstrip("\n").split("\t")
        table.setdefault(member, []).append((kind, name))
    return table


def main():
    args = sys.argv[1:]
    members = [line.strip() for line in open(args[0]) if line.strip()]
    table = load_index(members, args[1])
    inputs, extra = [], []
    rest = iter(args[2:])
    for a in rest:
        if a == "-u":
            extra.append(next(rest))
        else:
            inputs.append(a)
    strong_def, weak_def = {}, {}
    for member in members:
        for kind, name in table.get(member, []):
            if kind == "strong":
                strong_def.setdefault(name, member)
            elif kind == "weak":
                weak_def.setdefault(name, member)
    defined, undefined = set(), set(extra)
    for path in inputs:
        for kind, name in symbols(path):
            (undefined if kind == "undef" else defined).add(name)
    pulled, order = set(), []
    unresolved = set()
    while True:
        todo = sorted(undefined - defined - unresolved)
        if not todo:
            break
        for name in todo:
            member = strong_def.get(name) or weak_def.get(name)
            if member is None:
                unresolved.add(name)
                continue
            if member in pulled:
                continue
            pulled.add(member)
            order.append(member)
            for kind, sym in table[member]:
                (undefined if kind == "undef" else defined).add(sym)
    for member in order:
        print(member)
    for name in sorted(unresolved):
        print(f"unresolved: {name}", file=sys.stderr)


main()
