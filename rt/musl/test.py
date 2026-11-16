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
    # tests/regress: witnesses of the independent testing pass, each self-checking (status 0 natively).
    regressions = sorted(f"regress/{path.stem}" for path in (HERE / "tests/regress").glob("*.c"))
    names = ["library", "exit", "arena", "unimplemented", "stderr", "heap", "signals", "abort", "stdin", "brk",
             "fenv", "fenv-mode", "fenv-flags", "complex", "setjmp", "setjmp-independent", "setjmp-masks",
             "prototypes", "proto-independent",
             *regressions]
    # Non-local jumps are lowered after the optimiser, so they are also checked from -O2 (against the same
    # native output); the small ones are also un-run to their entry state.
    jumps = [name for name in names if "setjmp" in name]
    # Calls through another prototype are lowered before the optimiser, which must not undo them.
    optimised = jumps + ["prototypes", "proto-independent"]
    for name, level in [(name, "-O0") for name in names] + [(name, "-O2") for name in optimised]:
        source = HERE / "tests" / f"{name}.c"
        program = args.out / f"{name.replace('/', '-')}{'' if level == '-O0' else level}.bn"
        command = ["python3", HERE / "link.py", source, "--libdir", args.libdir, "--front", args.front,
                   "--arena-bytes", 16384 if name == "arena" else 1048576, f"--clang-flag={level}", "-o", program]
        linked = run(command)
        assert linked.returncode == 0, linked.stderr
        checked = run([args.executor, "check", program])
        assert checked.returncode == 0, checked.stderr
        reverse = ["--reverse"] if name in jumps and name != "setjmp-masks" else []
        result = run([args.executor, "run", "--entry", "__bennett_entry", "--budget", 1000000000, *reverse, program])
        if reverse:
            assert "returned to entry: classical state identical" in result.stdout, (name, level, result)
        if name in ["unimplemented", "stderr", "fenv-mode", "fenv-flags"]:
            assert result.returncode == 1 and "fault: unreachable" in result.stdout, (name, result)
        else:
            if name == "arena":
                status, output = 0, b"pages=4 errno=12\n"
            else:
                native = args.out / f"{name.replace('/', '-')}.native"
                built = run(["clang-18", "-O0", source, "-lm", "-o", native])
                assert built.returncode == 0, built.stderr
                reference = subprocess.run([native], capture_output=True, stdin=subprocess.DEVNULL)
                if name == "proto-independent":
                    native_o2 = native.with_suffix(".native-O2")
                    built = run(["clang-18", "-O2", source, "-o", native_o2])
                    assert built.returncode == 0, built.stderr
                    reference_o2 = subprocess.run([native_o2], capture_output=True, stdin=subprocess.DEVNULL)
                    assert (reference.returncode, reference.stdout) == (reference_o2.returncode, reference_o2.stdout), name
                # A process killed by a signal has the status a shell reports.
                status = reference.returncode if reference.returncode >= 0 else 128 - reference.returncode
                output = reference.stdout
            assert result.returncode == 0 and f"result: {status}\n" in result.stdout, (name, result)
            console = next((line.removeprefix("console: ") for line in result.stdout.splitlines()
                            if line.startswith("console: ")), "")
            assert console == escaped(output), (name, console, output)
        print(f"{name} {level}: passed ({program.stat().st_size} text bytes)", flush=True)
    print(f"library tests: {time.monotonic() - started:.2f}s", flush=True)


if __name__ == "__main__":
    main()
