#!/usr/bin/env python3
"""Build musl's own selected archive members as LLVM 18 bitcode, without network or source edits."""
import argparse
import hashlib
import json
from concurrent.futures import ThreadPoolExecutor
import os
from pathlib import Path
import subprocess

HERE = Path(__file__).resolve().parent
SOURCE = HERE / "musl-1.2.5"


def run(args, **kwargs):
    return subprocess.run([str(a) for a in args], check=True, **kwargs)


# Members compiled from the device's own source, with extra options, in place of musl's: musl's generic <fenv.h>
# dummies and the two of its generic fenv functions that differ from glibc's, and the functions IEEE 754 defines
# exactly, which the float runtime computes as the native glibc does.
REPLACED = {Path(f"src/fenv/{_name}.c"): (HERE / "fenv.c", [f"-DM_{_name}"])
            for _name in ["fenv", "fesetexceptflag", "feholdexcept"]}
for _base in ["sqrt", "fma", "rint", "nearbyint", "lrint", "llrint", "round", "lround", "llround", "floor", "ceil",
              "trunc", "fmin", "fmax"]:
    for _name in [_base, _base + "f", _base + "l"]:
        REPLACED[Path(f"src/math/{_name}.c")] = (HERE / "math.c", ["-fno-math-errno", f"-DM_{_name}"])
REPLACED[Path("src/math/ilogbl.c")] = (HERE / "math.c", ["-fno-math-errno", "-DM_ilogbl"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", nargs="?", type=Path, default=HERE.parents[1] / "target/musl")
    parser.add_argument("--jobs", type=int, default=min(16, os.cpu_count() or 1))
    options = parser.parse_args()
    out = options.out.resolve()
    build = out / "build"
    build.mkdir(parents=True, exist_ok=True)
    clang = os.environ.get("CLANG", "clang-18")
    with (out / "configure.log").open("w") as log:
        run([SOURCE / "configure", "--target=x86_64", "--disable-shared"], cwd=build,
            env={**os.environ, "CC": clang}, stdout=log, stderr=log)
        run(["make", "-s", "obj/include/bits/alltypes.h", "obj/include/bits/syscall.h",
             "obj/src/internal/version.h", "install-headers", f"DESTDIR={out / 'sysroot'}", "prefix=/"],
            cwd=build, stdout=log, stderr=log)
    selection = build / "selection.mk"
    selection.write_text("bennett-objects:\n\t@echo $(LIBC_OBJS)\n")
    objects = run(["make", "-s", "-f", "Makefile", "-f", selection, "bennett-objects"],
                  cwd=build, capture_output=True, text=True).stdout.split()
    flags = ["-std=c99", "-nostdinc", "-ffreestanding", "-fexcess-precision=standard",
             "-fno-rounding-math", "-fno-strict-aliasing", "-fno-stack-protector", "-D_XOPEN_SOURCE=700",
             "-Wno-unused-command-line-argument"]
    for directory in [HERE / "overlay", SOURCE / "arch/x86_64", SOURCE / "arch/generic",
                      build / "obj/src/internal", SOURCE / "src/include", SOURCE / "src/internal",
                      build / "obj/include", SOURCE / "include"]:
        flags += ["-I", str(directory)]
    fingerprint = hashlib.sha256(json.dumps(flags).encode())
    own = {source for source, _ in REPLACED.values()}
    for path in sorted([*SOURCE.rglob("*"), *(HERE / "overlay").glob("*.h"), *own, Path(__file__)]):
        if path.is_file():
            fingerprint.update(str(path.relative_to(HERE)).encode())
            fingerprint.update(path.read_bytes())
    stamp = out / "fingerprint"
    unchanged = stamp.exists() and stamp.read_text() == fingerprint.hexdigest()
    sources = []
    for obj in objects:
        base = Path(obj.removeprefix("obj/")).with_suffix("")
        generic = Path(str(base).replace("/x86_64/", "/")).with_suffix(".c")
        if (SOURCE / generic).is_file():
            source = generic
        else:
            source = next((base.with_suffix(ext) for ext in [".c", ".s", ".S"]
                           if (SOURCE / base.with_suffix(ext)).is_file()), None)
        if source is None:
            raise RuntimeError(f"musl selected an object without a source: {obj}")
        sources.append(source)
    def compile_one(source):
        if source.suffix != ".c":
            return str(source), "assembly (not built)"
        target = out / "bc" / source.with_suffix(".bc")
        target.parent.mkdir(parents=True, exist_ok=True)
        if unchanged and target.exists():
            return str(source), "cached"
        with target.with_suffix(".log").open("w") as log:
            own, extra = REPLACED.get(source, (SOURCE / source, []))
            run([clang, *flags, *extra, "-O0", "-Xclang", "-disable-O0-optnone", "-gline-tables-only",
                 "-emit-llvm", "-c", own, "-o", target], stdout=log, stderr=log)
        return str(source), "built"
    with ThreadPoolExecutor(options.jobs) as pool:
        results = list(pool.map(compile_one, sources))
    members = [str(out / "bc" / source.with_suffix(".bc")) for source in sources if source.suffix == ".c"]
    (out / "members.txt").write_text("".join(member + "\n" for member in members))
    if not unchanged:
        (out / "index.tsv").unlink(missing_ok=True)  # resolve.py's symbol cache names the members by path
    (out / "compile.tsv").write_text("".join(f"{source}\t{state}\n" for source, state in results))
    (out / "cflags.json").write_text(json.dumps(flags))
    (out / "libc.a").unlink(missing_ok=True)
    run([os.environ.get("LLVM_AR", "llvm-ar-18"), "rcs", out / "libc.a", *members])
    stamp.write_text(fingerprint.hexdigest())
    print(f"musl 1.2.5: {len(members)} C members; {len(sources) - len(members)} assembly files omitted")


if __name__ == "__main__":
    main()
