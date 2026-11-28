#!/usr/bin/env python3
"""build.py's reuse of archive members follows the files the driver reads.

A copy of the front's sources is built into a second bennett-cc whose declarations file lies in that copy (the
folder is fixed when the driver is built), and build.py builds one member, musl's close.c, with it (a `make`
wrapper on the path selects only that object). An unchanged second build reuses the member; after a line is added
to the copy's declarations file the member is compiled again, and an `#error` there stops the build, as it stops a
fresh one. Needs what build.py needs, CMake and LLVM 18; separate from the fast suites.
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
        prefix = front / "include/bq-prefix.h"
        prefix.write_text(prefix.read_text() + "/* changed */\n")
        result = build()
        assert result.returncode == 0 and states(lib) == {"src/unistd/close.c": "built"}, (result.stderr,)
        prefix.write_text(prefix.read_text() + "#error changed_declarations_file\n")
        result = build()
        # The driver's empty probe unit stops on the error first, before any member.
        assert result.returncode != 0 and "changed_declarations_file" in result.stderr, result.stderr
        assert "refused an empty unit" in result.stderr, result.stderr
        print("build reuse: passed (reused when unchanged; compiled again after the driver's declarations file "
              "changed)", flush=True)


if __name__ == "__main__":
    main()
