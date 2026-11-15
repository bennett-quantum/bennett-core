#!/usr/bin/env python3
"""Offline library integration tests, separate from the fast Rust and front suites."""
import argparse
from pathlib import Path
import subprocess
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def run(args):
    return subprocess.run([str(a) for a in args], capture_output=True, text=True)


def escaped(data):
    escape = {9: r"\t", 13: r"\r", 10: r"\n", 92: r"\\", 39: r"\'", 34: r'\"'}
    return '"' + ''.join(escape.get(b, chr(b) if 32 <= b < 127 else f"\\x{b:02x}") for b in data) + '"' if data else ''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--libdir", type=Path, default=ROOT / "target/musl")
    parser.add_argument("--out", type=Path, default=ROOT / ".tmp/rt/library-tests")
    parser.add_argument("--front", type=Path, default=ROOT / "target/front/bennett-front")
    parser.add_argument("--executor", type=Path, default=ROOT / "target/debug/bennett")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    for name in ["library", "exit", "arena", "unimplemented", "stderr"]:
        source = HERE / "tests" / f"{name}.c"
        program = args.out / f"{name}.bn"
        command = ["python3", HERE / "link.py", source, "--libdir", args.libdir, "--front", args.front,
                   "--arena-bytes", 16384 if name == "arena" else 1048576, "-o", program]
        linked = run(command)
        assert linked.returncode == 0, linked.stderr
        checked = run([args.executor, "check", program])
        assert checked.returncode == 0, checked.stderr
        result = run([args.executor, "run", "--entry", "__bennett_entry", "--budget", 100000000, program])
        if name in ["unimplemented", "stderr"]:
            assert result.returncode == 1 and "fault: unreachable" in result.stdout, (name, result)
        else:
            if name == "arena":
                status, output = 0, b"pages=4 errno=12\n"
            else:
                native = args.out / f"{name}.native"
                built = run(["clang-18", "-O0", source, "-lm", "-o", native])
                assert built.returncode == 0, built.stderr
                reference = subprocess.run([native], capture_output=True)
                status, output = reference.returncode, reference.stdout
            assert result.returncode == 0 and f"result: {status}\n" in result.stdout, (name, result)
            console = next((line.removeprefix("console: ") for line in result.stdout.splitlines()
                            if line.startswith("console: ")), "")
            assert console == escaped(output), (name, console, output)
        print(f"{name}: passed ({program.stat().st_size} text bytes)", flush=True)
    print(f"library tests: {time.monotonic() - started:.2f}s", flush=True)


if __name__ == "__main__":
    main()
