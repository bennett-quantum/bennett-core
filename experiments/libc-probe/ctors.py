"""Does what a static linker and a C runtime's startup do with constructors and destructors, for a linked LLVM
module in text form: the entries of @llvm.global_ctors and @llvm.global_dtors become two ordinary arrays,
@__bennett_ctors and @__bennett_dtors (function pointers in priority order), with their lengths
@__bennett_ctors_n and @__bennett_dtors_n (i64), and the two special globals are removed. Function pointers
placed in an .init_array* or .fini_array* section by the library itself are appended to the respective array. The probe runtime calls
the constructors before main and the destructors, last first, at exit. Without this, bennett-front lowers the
special globals as data and no constructor runs (reduced/ctor-not-run.c).

    python3 -I ctors.py <in.ll> <out.ll>
"""

import re
import sys

ENTRY = re.compile(r"\{\s*i32\s+(-?\d+),\s*ptr\s+(@[\w.$\"-]+|null),\s*ptr\s+[^}]*\}")


def main():
    text = open(sys.argv[1]).read()
    lines = text.split("\n")
    found = {"ctors": [], "dtors": []}
    sections = {"ctors": [], "dtors": []}
    kept = []
    for line in lines:
        # A function pointer a library places in an init or fini section itself (picolibc's atexit handlers run
        # from .fini_array_onexit), after the prioritised entries, as picolibc's linker script orders them.
        m = re.match(r'@[\w.$"-]+ = .*\bptr (@[\w.$"-]+), section "\.(init|fini)_array[\w.]*"', line)
        if m:
            sections["ctors" if m.group(2) == "init" else "dtors"].append(m.group(1))
        m = re.match(r"@llvm\.global_(ctors|dtors) = appending global \[\d+ x \{ i32, ptr, ptr \}\] \[(.*)\]", line)
        if m:
            entries = [(int(p), f) for p, f in ENTRY.findall(m.group(2)) if f != "null"]
            found[m.group(1)] = [f for _, f in sorted(entries, key=lambda e: e[0])]  # stable: link order kept
            continue
        kept.append(line)
    for kind in found:
        found[kind] += sections[kind]
    out = []
    for line in kept:
        m = re.match(r"@__bennett_(ctors|dtors)(_n)? = external (?:dso_local )?(?:global|constant) (.*?)(, align \d+)?$", line)
        if m:
            kind, count = m.group(1), m.group(2)
            fns = found[kind]
            if count:
                line = f"@__bennett_{kind}_n = constant i64 {len(fns)}"
            elif fns:
                line = f"@__bennett_{kind} = constant [{len(fns)} x ptr] [" + ", ".join("ptr " + f for f in fns) + "]"
            else:
                line = f"@__bennett_{kind} = constant [1 x ptr] zeroinitializer"
        out.append(line)
    open(sys.argv[2], "w").write("\n".join(out))


main()
