#!/usr/bin/env python3
"""Build musl's own selected archive members as LLVM 18 bitcode, without network or source edits.

configure runs with clang; each member is compiled with bennett-cc, which includes the declarations file
front/include/bq-prefix.h by force and makes the declaration check of docs/HANDLES.md, section 4, so that the library
takes its declarations of the effect primitives from that file (section 15, R1 and R2)."""
import argparse
import codecs
import fcntl
import hashlib
import json
from concurrent.futures import ThreadPoolExecutor
import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
SOURCE = HERE / "musl-1.2.5"
ROOT = HERE.parents[1]


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
# timer_create stops by name (timer.c): musl's links the thread exit's inline assembly.
REPLACED[Path("src/time/timer_create.c")] = (HERE / "timer.c", [])


def driver_reads(driver, flags, scratch):
    """The files the driver reads besides a unit, as the system reports them: every regular file that the driver or a
    process it starts (clang for the unit and for its baseline) opens for reading or executes, while it compiles an
    empty unit with the members' own options, recorded by strace. These are the driver's declarations file and
    baseline wherever they lie (through symbolic links too), the clang it runs and their libraries; the driver's
    temporary files, in a folder of the probe's own, are left out. None (no reuse) where strace cannot be run."""
    strace = shutil.which("strace")
    if strace is None:
        return None
    with tempfile.TemporaryDirectory(dir=scratch) as folder:  # a folder of this run's own: builds may run at once
        return traced_reads(strace, driver, flags, Path(folder))


def traced_reads(strace, driver, flags, probe):
    (probe / "probe.c").write_text("")
    trace = probe / "probe.trace"
    command = [strace, "-f", "-qq", "-s", "65535", "-e", "trace=open,openat,execve", "-o", trace,
               driver, *flags, "-O0", "-o", probe / "probe.bc", probe / "probe.c"]
    traced = subprocess.run([str(a) for a in command], cwd=probe, env={**os.environ, "TMPDIR": str(probe)})
    if traced.returncode != 0:
        raise SystemExit(f"{driver} refused an empty unit, so no member can be compiled with it")
    read = set()
    for line in trace.read_text(errors="replace").splitlines():
        call = re.search(r'\b(open|openat|execve)\((?:AT_FDCWD, )?"((?:[^"\\]|\\.)*)"(?:, ([A-Z_|]+))?', line)
        if not call or line.rstrip().endswith("= -1") or re.search(r"= -1 E[A-Z]+ ", line):
            continue
        if call[1] != "execve" and re.search(r"O_WRONLY|O_RDWR|O_CREAT", call[3] or ""):
            continue
        path = (probe / os.fsdecode(codecs.escape_decode(call[2].encode())[0])).resolve()
        if path.is_file() and probe.resolve() not in path.parents and not path.is_relative_to("/proc"):
            read.add(path)
    return sorted(read)


def publish(path, write):
    """Writes a file under a temporary name beside it and renames it into place, so that a program reading it (a
    test linking with the archive while another test runs this build) sees the old file or the new one, never a part."""
    part = path.with_name(path.name + f".part{os.getpid()}")
    write(part)
    os.replace(part, path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", nargs="?", type=Path, default=HERE.parents[1] / "target/musl")
    parser.add_argument("--jobs", type=int, default=min(16, os.cpu_count() or 1))
    parser.add_argument("--driver", type=Path, default=ROOT / "target/front/bennett-cc", help="bennett-cc")
    options = parser.parse_args()
    out = options.out.resolve()
    build = out / "build"
    build.mkdir(parents=True, exist_ok=True)
    # One build of a folder at a time: tests run this script at once on target/musl, and configure, musl's headers
    # and the members are rewritten in place while a build that finds them changed runs.
    with (out / "build.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        build_locked(options, out, build)


def build_locked(options, out, build):
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
    driver = options.driver.resolve()
    # The members depend on the driver and on the files it reads, as on musl's own sources.
    fingerprint.update(driver.read_bytes())
    for path in sorted([*SOURCE.rglob("*"), *(HERE / "overlay").glob("*.h"), *own, Path(__file__)]):
        if path.is_file():
            fingerprint.update(str(path.relative_to(ROOT)).encode())
            fingerprint.update(path.read_bytes())
    reads = driver_reads(driver, flags, build)
    for path in reads or []:
        fingerprint.update(str(path).encode())
        fingerprint.update(path.read_bytes())
    stamp = out / "fingerprint"
    unchanged = reads is not None and stamp.exists() and stamp.read_text() == fingerprint.hexdigest()
    if reads is None:
        print("strace not found: the files the driver reads are unknown, so no member is reused")
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
            # bennett-cc takes clang's options, then `-o OUTPUT UNIT`; it adds -emit-llvm -c itself.
            run([driver, *flags, *extra, "-O0", "-Xclang", "-disable-O0-optnone",
                 "-gline-tables-only", "-o", target, own], stdout=log, stderr=log)
        return str(source), "built"
    with ThreadPoolExecutor(options.jobs) as pool:
        results = list(pool.map(compile_one, sources))
    members = [str(out / "bc" / source.with_suffix(".bc")) for source in sources if source.suffix == ".c"]
    publish(out / "members.txt", lambda part: part.write_text("".join(member + "\n" for member in members)))
    if not unchanged:
        (out / "index.tsv").unlink(missing_ok=True)  # resolve.py's symbol cache names the members by path
    publish(out / "compile.tsv",
            lambda part: part.write_text("".join(f"{source}\t{state}\n" for source, state in results)))
    publish(out / "cflags.json", lambda part: part.write_text(json.dumps(flags)))
    publish(out / "libc.a", lambda part: run([os.environ.get("LLVM_AR", "llvm-ar-18"), "rcs", part, *members]))
    publish(stamp, lambda part: part.write_text(fingerprint.hexdigest()))
    print(f"musl 1.2.5: {len(members)} C members; {len(sources) - len(members)} assembly files omitted")


if __name__ == "__main__":
    main()
