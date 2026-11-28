#!/usr/bin/env python3
"""Offline library integration tests, separate from the fast Rust and front suites."""
import argparse
import re
from pathlib import Path
import re
import subprocess
import tempfile
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


# Programs the device refuses by name (`unreachable`): an unimplemented system call, a write to stderr, sleeps
# that natively never end or depend on the host (`rt/musl/README.md`, "Sleeping"), and a timer.
REFUSED = ["unimplemented", "stderr", "sleep-cpu", "sleep-beyond", "sleep-alarm-clock", "timer-alarm",
           "timer-create"]


# Programs the device stops with `out_of_bounds`: a sleep request beyond device memory, which the runtime reads
# since it does not know where memory ends (natively EFAULT; `rt/musl/README.md`, "Sleeping").
BEYOND_MEMORY = ["sleep-beyond-memory"]


# Programs whose native output depends on the host, each with the device's output as a function of the native
# one: a host without a real-time clock device answers EOPNOTSUPP (-95) where the device, like a host with one,
# answers EINVAL (-22) for the alarm clocks' other flags.
HOST_DEPENDENT = {"sleep-alarm-flags": lambda native: native.replace(b" -95", b" -22")}


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


def caller_frame(args):
    """A sleep request in a frame above the C library's start-up (the entry caller_entry calls start-up), against
    native, where main is the entry and the request in its own frame."""
    source = HERE / "tests/sleep-caller.c"
    program = args.out / "sleep-caller.bn"
    linked = run(["python3", HERE / "link.py", source, "--libdir", args.libdir, "--front", args.front, "-o", program])
    assert linked.returncode == 0, linked.stderr
    result = run([args.executor, "run", "--entry", "caller_entry", "--budget", 100000000, program])
    native = args.out / "sleep-caller.native"
    built = run(["clang-18", "-O0", "-DNATIVE", source, "-o", native])
    assert built.returncode == 0, built.stderr
    reference = subprocess.run([native], capture_output=True, stdin=subprocess.DEVNULL)
    assert result.returncode == 0 and f"result: {reference.returncode}\n" in result.stdout, result
    console = next((line.removeprefix("console: ") for line in result.stdout.splitlines()
                    if line.startswith("console: ")), "")
    assert console == escaped(reference.stdout), (console, reference.stdout)
    print(f"sleep-caller: passed (status {reference.returncode})", flush=True)


def partial_request(args):
    """A sleep request whose seconds are the last eight bytes of device memory and whose nanoseconds lie past its
    end stops with `out_of_bounds` (natively EFAULT) for negative seconds too: the runtime copies the whole request
    before it checks a field. Eight bytes lower, the whole request fits and its fields are checked as natively."""
    source = HERE / "tests/sleep-partial.c"
    program = args.out / "sleep-partial.bn"
    linked = run(["python3", HERE / "link.py", source, "--libdir", args.libdir, "--front", args.front, "-o", program])
    assert linked.returncode == 0, linked.stderr
    memory = int(re.search(r"^memory (\d+)$", program.read_text(), re.M)[1])
    cases = [(memory - 8, seconds, 0, 1, None) for seconds in (-1, 0)]
    # A request in memory: a negative second count or nanoseconds of a second or more is EINVAL, zero sleeps.
    cases += [(memory - 16, -1, 0, 2, "result: 22"), (memory - 16, 0, 10**9, 2, "result: 22"),
              (memory - 16, 0, 0, 2, "result: 0")]
    for address, seconds, nanoseconds, fields, expected in cases:
        for call in range(3):  # nanosleep, clock_nanosleep, the raw system call
            values = [address, seconds % (1 << 64), nanoseconds, fields, call]
            result = run([args.executor, "run", "--entry", "partial_entry", "--budget", 10000000,
                          *[f"--arg={value}" for value in values], program])
            if expected is None:
                assert result.returncode == 1 and "fault: out_of_bounds" in result.stdout, (values, result)
            else:
                assert result.returncode == 0 and f"{expected}\n" in result.stdout, (values, result)
    print(f"sleep-partial: passed ({len(cases) * 3} runs)", flush=True)


# Structure-pointer and error-order checks across all implemented syscalls taking struct pointers
# (plus time(time_t *), the data buffers and the transfer cap): agreeing batches, single agreeing cases, a
# small-stack run, and named
# out_of_bounds stops for a range that reaches the end of device memory (natively EFAULT; known debt).
# Also in front/tests/tests/syscall_efault_independent.rs.
EFAULT_AGREE = ["efault-time", "efault-stat", "efault-iov", "efault-signals"]
EFAULT_CASES_AGREE = [
    *[("efault-time", case, False) for case in (
        "clock_gettime-low8", "clock_gettime-wrap8", "clock_getres-low8", "clock_getres-wrap8",
        "gettimeofday-tv-low8", "gettimeofday-tv-wrap8", "gettimeofday-tz-low8", "gettimeofday-tz-wrap4",
        "time-low8", "time-wrap4", "nanosleep-above-top")],
    *[("efault-stat", case, False) for case in (
        "stat-low8", "stat-wrap8", "lstat-low8", "lstat-wrap8", "fstat-low8", "fstat-wrap8",
        "newfstatat-low8", "newfstatat-wrap8")],
    *[("efault-iov", case, False) for case in (
        "writev-low8", "writev-wrap8", "writev-base-low8", "writev-ro-null", "writev-ro-badlen",
        "writev-ro-badcnt", "readv-low8", "readv-wrap8", "readv-base-low8", "readv-wo-null", "readv-wo-badlen",
        "readv-wo-badcnt", "readv-alias", "read-low8", "write-low8", "getrandom-low8", "getcwd-low8",
        "writev-wrap-second", "readv-wrap-second", "low-untouched", "ranges-and-cap", "positional-overflow")],
    ("efault-iov", "writev-ro-partial8", True),
    ("efault-iov", "readv-wo-partial8", True),
    *[("efault-signals", case, False) for case in (
        "sigaction-alias", "sigaction-sigkill-low8", "sigaction-sigkill-wrap8", "sigaction-act-low8",
        "sigaction-act-wrap8", "sigaction-old-low8", "sigaction-old-wrap8", "sigprocmask-badhow-low8",
        "sigprocmask-badhow-wrap4", "sigprocmask-set-low8", "sigprocmask-set-wrap4", "sigprocmask-old-low8",
        "sigprocmask-old-wrap4", "sigpending-null", "sigpending-size4", "sigpending-low8", "sigpending-wrap4",
        "sigaction-stored-mask")],
    ("efault-stat", "at-fdcwd-int", False),
]
# Agreeing cases run on the device with a small stack (--stack-bytes): each call's copies live in its own frame.
EFAULT_SMALL_STACK = [("efault-stack", "small-stack")]
SMALL_STACK_BYTES = 8192
EFAULT_NAMED_STOP = [
    ("efault-time", "clock_gettime-partial8", True, "ret=-1 errno=14"),
    ("efault-time", "clock_getres-partial8", True, "ret=-1 errno=14"),
    ("efault-time", "gettimeofday-tv-partial8", True, "ret=-1 errno=14"),
    ("efault-time", "gettimeofday-tz-partial4", True, "ret=-1 errno=14"),
    ("efault-time", "time-partial4", True, "ok=0 errno=14"),
    ("efault-stat", "stat-partial8", True, "ret=-1 errno=14"),
    ("efault-stat", "stat-partial136", True, "ret=-1 errno=14"),
    ("efault-stat", "lstat-partial8", True, "ret=-1 errno=14"),
    ("efault-stat", "fstat-partial8", True, "ret=-1 errno=14"),
    ("efault-stat", "newfstatat-partial8", True, "ret=-1 errno=14"),
    ("efault-iov", "writev-partial8", True, "ret=-1 errno=14"),
    ("efault-iov", "writev-partial16-valid0", True, "ret=-1 errno=14"),
    ("efault-iov", "writev-base-partial4", True, "ret=4 errno=0"),
    ("efault-iov", "readv-partial8", True, "ret=-1 errno=14"),
    ("efault-iov", "readv-partial16-valid0", True, "ret=-1 errno=14"),
    ("efault-iov", "readv-base-partial4", True, "ret=4 errno=0"),
    ("efault-signals", "sigaction-sigkill-partial8", True, "ret=-1 errno=14"),
    ("efault-signals", "sigaction-sigstop-partial8", True, "ret=-1 errno=14"),
    ("efault-signals", "sigaction-sig0-partial8", True, "ret=-1 errno=14"),
    ("efault-signals", "sigaction-sig65-partial8", True, "ret=-1 errno=14"),
    ("efault-signals", "sigaction-act-partial8", True, "ret=-1 errno=14"),
    ("efault-signals", "sigaction-act-partial24", True, "ret=-1 errno=14"),
    ("efault-signals", "sigaction-old-partial8", True, "ret=-1 errno=14"),
    ("efault-signals", "sigprocmask-badhow-partial4", True, "ret=-1 errno=14"),
    ("efault-signals", "sigprocmask-set-partial4", True, "ret=-1 errno=14"),
    ("efault-signals", "sigprocmask-old-partial4", True, "ret=-1 errno=14"),
    ("efault-signals", "sigpending-partial4", True, "ret=-1 errno=14"),
]


def efault_cases(args):
    natives = {}
    memory_sizes = {}
    for suite in EFAULT_AGREE:
        source = HERE / "tests" / f"{suite}.c"
        native = args.out / f"{suite}.native"
        built = run(["clang-18", "-O0", source, "-o", native])
        assert built.returncode == 0, built.stderr
        natives[suite] = native
        probe = args.out / f"{suite}-probe.bn"
        linked = run(["python3", HERE / "link.py", source, "--libdir", args.libdir, "--front", args.front,
                      "--clang-flag=-O0", "-o", probe, "--", "agree"])
        assert linked.returncode == 0, linked.stderr
        match = re.search(r"^memory (\d+)$", probe.read_text(), re.M)
        assert match, probe
        memory_sizes[suite] = match[1]

    def run_pair(suite, case, pass_edge, link_flags=()):
        with tempfile.TemporaryDirectory(dir=args.out) as directory:
            reference = subprocess.run([str(natives[suite]), case], cwd=directory, capture_output=True,
                                       stdin=subprocess.DEVNULL)
        assert reference.returncode == 0, (suite, case, reference)
        program = args.out / f"{suite}-{case}.bn"
        dev_args = [case, memory_sizes[suite]] if pass_edge else [case]
        linked = run(["python3", HERE / "link.py", HERE / "tests" / f"{suite}.c", "--libdir", args.libdir,
                      "--front", args.front, "--clang-flag=-O0", *link_flags, "-o", program, "--", *dev_args])
        assert linked.returncode == 0, linked.stderr
        if pass_edge:
            assert f"memory {memory_sizes[suite]}\n" in program.read_text(), (suite, case)
        result = run([args.executor, "run", "--entry", "__bennett_entry", "--budget", 10000000, program])
        return reference.stdout, result

    for suite in EFAULT_AGREE:
        output, result = run_pair(suite, "agree", True)
        assert result.returncode == 0 and "result: 0\n" in result.stdout, (suite, result)
        console = next((line.removeprefix("console: ") for line in result.stdout.splitlines()
                        if line.startswith("console: ")), "")
        assert console == escaped(output), (suite, console, output)
        print(f"efault {suite} agree: passed", flush=True)

    for suite, case, pass_edge, expected_substr in EFAULT_NAMED_STOP:
        output, result = run_pair(suite, case, pass_edge)
        assert expected_substr.encode() in output, (suite, case, output)
        assert result.returncode == 1 and "fault: out_of_bounds (code 3)" in result.stdout, (suite, case, result)
        print(f"efault {case}: named stop (out_of_bounds)", flush=True)

    for suite, case, pass_edge in EFAULT_CASES_AGREE:
        output, result = run_pair(suite, case, pass_edge)
        assert result.returncode == 0 and "result: 0\n" in result.stdout, (suite, case, result)
        console = next((line.removeprefix("console: ") for line in result.stdout.splitlines()
                        if line.startswith("console: ")), "")
        assert console == escaped(output), (suite, case, console, output)
        print(f"efault {case}: passed ({output.decode().strip()})", flush=True)

    for suite, case in EFAULT_SMALL_STACK:
        native = args.out / f"{suite}.native"
        built = run(["clang-18", "-O0", HERE / "tests" / f"{suite}.c", "-o", native])
        assert built.returncode == 0, built.stderr
        natives[suite] = native
        output, result = run_pair(suite, case, False, [f"--stack-bytes={SMALL_STACK_BYTES}"])
        assert result.returncode == 0 and "result: 0\n" in result.stdout, (suite, case, result)
        console = next((line.removeprefix("console: ") for line in result.stdout.splitlines()
                        if line.startswith("console: ")), "")
        assert console == escaped(output), (suite, case, console, output)
        print(f"efault {suite} {case}: passed ({SMALL_STACK_BYTES}-byte stack)", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--libdir", type=Path, default=ROOT / "target/musl")
    parser.add_argument("--out", type=Path, default=ROOT / ".tmp/rt/library-tests")
    parser.add_argument("--front", type=Path, default=ROOT / "target/front/bennett-front")
    parser.add_argument("--executor", type=Path, default=ROOT / "target/debug/bennett")
    parser.add_argument("--efault-only", action="store_true",
                        help="run only the structure-pointer EFAULT and error-order cases")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    if args.efault_only:
        efault_cases(args)
        print(f"efault tests: {time.monotonic() - started:.2f}s", flush=True)
        return
    # tests/regress: witnesses of the independent testing pass, each self-checking (status 0 natively).
    regressions = sorted(f"regress/{path.stem}" for path in (HERE / "tests/regress").glob("*.c"))
    names = ["library", "exit", *EXHAUSTED, "alloc-arithmetic", *REFUSED, *BEYOND_MEMORY, "heap", "signals", "sleep",
             "sleep-alarm-flags", "timer-own",
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
        if name in REFUSED:
            assert result.returncode == 1 and "fault: unreachable" in result.stdout, (name, result)
        elif name in BEYOND_MEMORY:
            assert result.returncode == 1 and "fault: out_of_bounds" in result.stdout, (name, result)
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
            output = HOST_DEPENDENT.get(name, lambda native: native)(reference.stdout)
            if output != reference.stdout:
                print(f"{name}: the host answers {reference.stdout!r}, compared as {output!r}", flush=True)
            assert result.returncode == 0 and f"result: {status}\n" in result.stdout, (name, result)
            console = next((line.removeprefix("console: ") for line in result.stdout.splitlines()
                            if line.startswith("console: ")), "")
            assert console == escaped(output), (name, console, output)
        print(f"{name} {level}: passed ({program.stat().st_size} text bytes)", flush=True)
    argument_cases(args)
    relinked_images(args)
    caller_frame(args)
    partial_request(args)
    efault_cases(args)
    print(f"library tests: {time.monotonic() - started:.2f}s", flush=True)


if __name__ == "__main__":
    main()
