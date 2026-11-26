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


# Heap exhaustion ends the run with the fault `out_of_memory`, keeping the lines flushed before it. Natively these
# programs would succeed (or allocate until the host refuses), so each states its own console: a function of
# the number k of lines that fit, here checked for every k up to a bound far above the arena's.
def lines(make):
    return lambda console: any(console == escaped(b"".join(make(i) for i in range(1, k + 1))) for k in range(1, 200))


EXHAUSTED = {
    "arena": lambda console: console == escaped(b"".join(b"page %d\n" % i for i in range(1, 5))),
    "heap-exhaust": lines(lambda i: b"%d\n" % i),
    "realloc-exhaust": lines(lambda i: b"%d\n" % (16 << i)),
    "mmap-exhaust": lambda console: console == escaped(b"small 1\n"),
    "brk-exhaust": lambda console: console == escaped(b"grown 1\n"),
}


# Run arguments (link.py -- ARGUMENT ..., --argv0): each case runs natively with the same argv (argv[0] included) and an
# empty environment, and on the device, comparing status and stdout. Arguments are exact bytes; link.py options
# may follow. The longest native argument, 131071 bytes and its NUL (Linux's MAX_ARG_STRLEN), starts with '-'.
LONGEST = b"-" + b"0" * 131069 + b"7"
ARGUMENT_CASES = [
    ("args", "no-arguments", None, [], ["-O0"]),  # link.py's default argv[0]
    ("args", "empty", b"prog", [b"", b"", b"x", b""], ["-O0"]),
    ("args", "many", b"prog", [b"%d" % i * (i % 7) for i in range(200)], ["-O0"]),
    ("args", "bytes", b"prog", [b"\xc3\xa9t\xc3\xa9", bytes(range(0x80, 0x100)), b"\x01\x02\x1f\x7f",
                                b" spaces \"quotes\" \\ ", b"-x", b"--", b"-o", b"--argv0=x", b"="],
     ["-O0", "-O2"]),
    ("args", "argv0", b"/usr/local/bin/named", [b"a"], ["-O0"]),
    ("args", "argv0-empty", b"", [b"a"], ["-O0"]),
    ("budget", "budget-enough", b"prog", [b"1000"], ["-O0", "-O2"]),
    ("budget", "budget-short", b"prog", [b"10", b"0x1b"], ["-O0"]),
    ("budget", "budget-range", b"prog", [b"18446744073709551616"], ["-O0"]),
    ("budget", "budget-rest", b"prog", [b"12abc"], ["-O0"]),
    ("budget", "budget-missing", b"prog", [], ["-O0"]),
    ("budget", "budget-longest", b"prog", [LONGEST], ["-O0"], ["--argument-bytes=135168"]),
]


def argument_cases(args):
    for source_name, case, argv0, arguments, levels, *options in ARGUMENT_CASES:
        source = HERE / "tests" / f"{source_name}.c"
        for level in levels:
            program = args.out / f"args-{case}{level}.bn"
            command = ["python3", HERE / "link.py", source, "--libdir", args.libdir, "--front", args.front,
                       f"--clang-flag={level}", "-o", program, *([] if argv0 is None else [b"--argv0=" + argv0]),
                       *(options[0] if options else []), "--", *arguments]
            linked = subprocess.run([a if isinstance(a, bytes) else str(a) for a in command], capture_output=True)
            assert linked.returncode == 0, linked.stderr
            result = run([args.executor, "run", "--entry", "__bennett_entry", "--budget", 1000000000, program])
            native = args.out / f"args-{source_name}{level}.native"
            built = run(["clang-18", level, source, "-o", native])
            assert built.returncode == 0, built.stderr
            reference = subprocess.run([b"prog" if argv0 is None else argv0, *arguments], executable=native, env={},
                                       capture_output=True, stdin=subprocess.DEVNULL)
            assert f"result: {reference.returncode}\n" in result.stdout, (case, level, result.stdout, reference)
            assert result.returncode == 0, (case, level, result)
            console = next((line.removeprefix("console: ") for line in result.stdout.splitlines()
                            if line.startswith("console: ")), "")
            assert console == escaped(reference.stdout), (case, level, console, reference.stdout)
            print(f"arguments {case} {level}: passed (status {reference.returncode})", flush=True)


def code_lines(path):
    """The device program's lines other than data items: its memory size and every function's text."""
    return [line for line in path.read_text().splitlines() if not line.startswith("data ")]


def relink(args, source, level, arguments, name):
    program = args.out / f"relink-{name}{level}.bn"
    linked = run(["python3", HERE / "link.py", source, "--libdir", args.libdir, "--front", args.front,
                  f"--clang-flag={level}", "-o", program, "--", *arguments])
    assert linked.returncode == 0, linked.stderr
    return program


def relinked_images(args):
    """A relink with other arguments, in count and in length, changes the image's data only: every function and
    the memory size are the same text. main sees each set of arguments; the applied function (a second entry;
    quantum operations in main of a musl-linked program are refused today) takes the same steps in both."""
    runs = []
    for name, arguments in (("one", ["1000"]), ("three", ["77", "a longer second argument", ""])):
        program = relink(args, HERE / "tests/args-apply.c", "-O0", arguments, name)
        classical = run([args.executor, "run", "--entry", "__bennett_entry", "--budget", 100000000, program])
        assert classical.returncode == 0 and f'console: "budget {arguments[0]}\\n"' in classical.stdout, classical
        applied = run([args.executor, "run", "--entry", "applied_entry", "--stats", "--budget", 100000000, program])
        assert applied.returncode == 0 and "region faults: 0 " in applied.stdout, applied
        counts = [line.strip() for line in applied.stdout.splitlines()
                  if line.startswith(("result: ", "steps: ", "  region steps: "))]
        assert len(counts) == 3 and int(counts[2].removeprefix("region steps: ")) > 0, applied.stdout
        runs.append((code_lines(program), counts))
    (first, first_counts), (second, second_counts) = runs
    assert first_counts == second_counts, (first_counts, second_counts)
    assert first == second, next((a, b) for a, b in zip(first, second) if a != b)
    optimised = [code_lines(relink(args, HERE / "tests/args.c", "-O2", arguments, name))
                 for name, arguments in (("none", []), ("several", ["x" * 100, "", "y"]))]
    assert optimised[0] == optimised[1], next((a, b) for a, b in zip(*optimised) if a != b)
    # Arguments that do not fit the area are refused by name.
    refused = run(["python3", HERE / "link.py", HERE / "tests/args.c", "--libdir", args.libdir, "--front", args.front,
                   "--argument-bytes=8", "-o", args.out / "relink-refused.bn", "--", "four"])
    assert refused.returncode == 2 and "the arguments need 10 bytes" in refused.stderr, refused
    print(f"arguments relinked: passed (code identical; {', '.join(first_counts)} in both)", flush=True)


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
    names = ["library", "exit", *EXHAUSTED, "alloc-arithmetic", "unimplemented", "stderr", "heap", "signals",
             "abort", "stdin", "brk",
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
        elif name in EXHAUSTED:
            assert result.returncode == 1 and "fault: out_of_memory (code 19)" in result.stdout, (name, result)
            console = next((line.removeprefix("console: ") for line in result.stdout.splitlines()
                            if line.startswith("console: ")), "")
            assert EXHAUSTED[name](console), (name, console)
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
    argument_cases(args)
    relinked_images(args)
    print(f"library tests: {time.monotonic() - started:.2f}s", flush=True)


if __name__ == "__main__":
    main()
