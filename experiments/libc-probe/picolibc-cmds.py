"""Turns picolibc's meson compile database into one bitcode compile command per C library source, with the README's
clang flags in place of picolibc's optimisation and debug flags, and prints them as tab-separated lines:
source (relative to the picolibc checkout), kind (c | asm-source | generic), shell command.

    python3 -I picolibc-cmds.py <build dir> <bitcode dir>
    python3 -I picolibc-cmds.py <build dir> <bitcode dir> --cflags   # the flags of libc.a's sources, -I absolute

Sources: everything meson puts into libc.a, the x86 machine directory (libmachine.a) and libstrcmp.a. An x86
assembly source is reported as asm-source; when it defines symbols on x86_64 (memcpy, memset, setjmp, ...) and
picolibc has a generic C file of the same name in libc/string, that file is compiled instead (kind generic), as
musl-bc.sh's GENERIC=1 does. Skipped: libdummyhost.a (the probe runtime is the porting layer), the empty libraries.
"""

import json
import os
import shlex
import sys

DROP_PREFIX = ("-O", "-g", "-W", "-fdiagnostics-color", "-ffunction-sections", "-mstack-protector-guard", "-MD")
DROP_WITH_ARG = ("-o", "-c", "-MQ", "-MF")
FLAGS = ["-O0", "-Xclang", "-disable-O0-optnone", "-gline-tables-only", "-emit-llvm", "-w"]


def main():
    build, bcdir = sys.argv[1], sys.argv[2]
    db = json.load(open(os.path.join(build, "compile_commands.json")))
    libc_flags = None
    for entry in db:
        out = entry["output"]
        if not (out.startswith("libc.a.p/") or "libmachine.a.p/" in out or "libstrcmp.a.p/" in out):
            continue
        args = shlex.split(entry["command"])
        src = os.path.normpath(os.path.join(entry["directory"], entry["file"]))
        rel = src.split("/picolibc/", 1)[1]
        kept, skip = [], False
        for a in args[1:]:
            if skip:
                skip = False
                continue
            if a in DROP_WITH_ARG:
                skip = True
                continue
            if a.startswith(DROP_PREFIX) or a == entry["file"]:
                continue
            if a.startswith("-I") and not os.path.isabs(a[2:]):
                a = "-I" + os.path.normpath(os.path.join(entry["directory"], a[2:]))
            kept.append(a)
        if out.startswith("libc.a.p/") and libc_flags is None:
            libc_flags = kept
        kind, source = "c", src
        if rel.endswith(".S"):
            kind = "asm-source"
            base = os.path.basename(rel)[:-2]
            generic = os.path.join(src.split("/libc/", 1)[0], "libc", "string", base + ".c")
            if os.path.exists(generic):
                kind, source, kept = "generic", generic, libc_flags or kept
                rel = rel + " -> libc/string/" + base + ".c"
        if "--cflags" in sys.argv:
            continue
        if kind == "asm-source":
            print(f"{rel}\t{kind}\t")
            continue
        bc = os.path.join(bcdir, os.path.relpath(source, src.split("/picolibc/", 1)[0] + "/picolibc") + ".bc")
        cmd = " ".join(shlex.quote(a) for a in [args[0]] + kept + FLAGS + ["-c", source, "-o", bc])
        rel = os.path.relpath(source, src.split("/picolibc/", 1)[0] + "/picolibc")
        print(f"{rel}\t{kind}\tcd {shlex.quote(entry['directory'])} && mkdir -p {shlex.quote(os.path.dirname(bc))}"
              f" && {cmd}")
    if "--cflags" in sys.argv:
        print(" ".join(shlex.quote(a) for a in libc_flags))


main()
