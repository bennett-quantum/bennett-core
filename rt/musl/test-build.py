#!/usr/bin/env python3
"""build.py's reuse of archive members follows the files the driver reads.

A copy of the front's sources is built into a second bennett-cc whose files lie in that copy (the folder is fixed
when the driver is built); there its declarations file and its baseline are symbolic links into two other folders.
build.py builds one member, musl's close.c, with that driver (a `make` wrapper on the path selects only that
object). An unchanged second build reuses the member; after a line is added, through the links, to the declarations
file and then to the baseline, the member is compiled again each time; an `#error` in the baseline stops the build,
as it stops a fresh one. Needs what build.py needs, strace, CMake and LLVM 18; separate from the fast suites.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def run(args, **kwargs):
    return subprocess.run([str(a) for a in args], capture_output=True, text=True, **kwargs)


def states(lib):
    return dict(line.split("\t") for line in (lib / "compile.tsv").read_text().splitlines())


def main():
    with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR")) as directory:
        work = Path(directory)
        front = work / "front"
        front.mkdir()
        for name in ("CMakeLists.txt", "llvm-major.txt"):
            shutil.copy(ROOT / "front" / name, front / name)
        shutil.copytree(ROOT / "front/src", front / "src")
        shutil.copytree(ROOT / "front/include", front / "include")
        (front / "tests").mkdir()
        shutil.copy(ROOT / "front/tests/checks.cpp", front / "tests/checks.cpp")
        (work / "rt").symlink_to(ROOT / "rt")  # the front's CMake file names ../rt
        for step in (["cmake", "-S", front, "-B", work / "build"],
                     ["cmake", "--build", work / "build", "--target", "bennett-cc", "--parallel",
                      os.environ.get("CARGO_BUILD_JOBS", "2")]):
            result = run(step)
            assert result.returncode == 0, (step, result.stdout, result.stderr)
        driver = work / "build/bennett-cc"
        tools = work / "bin"
        tools.mkdir()
        # build.py asks musl's make for its object list (the bennett-objects goal); answer with one object.
        (tools / "make").write_text(f'#!/bin/sh\ncase "$*" in\n  *bennett-objects*) echo obj/src/unistd/close.o;;\n'
                                    f'  *) exec {shutil.which("make")} "$@";;\nesac\n')
        (tools / "make").chmod(0o755)
        env = {**os.environ, "PATH": f"{tools}:{os.environ['PATH']}"}
        lib = work / "lib"

        def build():
            return run(["python3", HERE / "build.py", lib, "--driver", driver, "--jobs", "1"], env=env)

        result = build()
        assert result.returncode == 0 and states(lib) == {"src/unistd/close.c": "built"}, (result.stderr,)
        result = build()
        assert result.returncode == 0 and states(lib) == {"src/unistd/close.c": "cached"}, (result.stderr,)
        # The declarations file and the baseline become links into folders of their own.
        for name, folder in (("bq-prefix.h", "shared"), ("bq-baseline.c", "elsewhere")):
            (work / folder).mkdir()
            (front / "include" / name).rename(work / folder / name)
            (front / "include" / name).symlink_to(work / folder / name)
        result = build()
        assert result.returncode == 0 and states(lib) == {"src/unistd/close.c": "built"}, (result.stderr,)
        result = build()
        assert result.returncode == 0 and states(lib) == {"src/unistd/close.c": "cached"}, (result.stderr,)
        for name in ("bq-prefix.h", "bq-baseline.c"):
            linked = front / "include" / name
            linked.resolve().write_text(linked.read_text() + "/* changed */\n")
            result = build()
            assert result.returncode == 0 and states(lib) == {"src/unistd/close.c": "built"}, (name, result.stderr)
        baseline = front / "include/bq-baseline.c"
        baseline.resolve().write_text(baseline.read_text() + "#error changed_baseline\n")
        result = build()
        # The driver's empty probe unit stops on the error first, before any member.
        assert result.returncode != 0 and "changed_baseline" in result.stderr, result.stderr
        assert "refused an empty unit" in result.stderr, result.stderr
        print("build reuse: passed (reused when unchanged; compiled again after the driver's declarations file and "
              "baseline changed through links)", flush=True)


if __name__ == "__main__":
    main()
