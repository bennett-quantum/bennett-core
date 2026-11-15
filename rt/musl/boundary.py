#!/usr/bin/env python3
"""Targeted boundary tests. Failing witnesses require --include-pending/--include-debts or --case NAME."""
import argparse
from dataclasses import dataclass
from pathlib import Path
import re
import subprocess
import time

from test import escaped

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


@dataclass
class Case:
    name: str
    source: str
    kind: str = "specified"
    status: int = 0
    output: bytes = b""
    flags: tuple = ()


def cases():
    result = [Case("descriptors", "descriptors", output=b"ABC"),
              Case("arena", "arena"), Case("signals", "signals")]
    for name in ["startup-return", "startup-nested-exit", "startup-ctor-exit", "exit-immediate", "atexit-bulk", "malloc-lite"]:
        result.append(Case(name, name, "native"))
    for call in ["exit", "exit_group"]:
        result.append(Case(call, "exit-syscall", status=43, flags=(f"-DCALL=SYS_{call}",)))
    for call in ["kill", "tkill", "tgkill"]:
        result.append(Case(call, "abort-syscall", status=134, flags=(f"-DCALL=SYS_{call}",)))
    for status in [0, 255, 256, 257, -1, -2147483648]:
        result.append(Case(f"status-{status}", "status", status=status & 0xffffffff,
                           flags=(f"-DSTATUS=({status})",)))
    for number, name in enumerate(["stderr-zero", "stderr-writev", "open", "openat", "unknown",
                                    "mmap-fixed", "mmap-shared", "mmap-file", "mprotect-readonly",
                                    "madvise-other", "sigaction-handler"]):
        result.append(Case(name, "refusal", "fault", flags=(f"-DCASE={number}",)))
    pending = [Case(name, name, "pending") for name in [
        "writev-zero-bad-fd", "lseek-closed-fd", "ioctl-bad-fd", "sigaction-invalid-signal",
        "madvise-partial-page", "writev-overflow-effects", "mremap-exit", "brk-regrow"]]
    return result, pending, [Case("malloc-free-guard", "malloc-free-guard", "debt")]


def run(command):
    return subprocess.run([str(x) for x in command], cwd=ROOT, capture_output=True, timeout=90)


def check(case, args):
    folder = {"pending": "pending", "debt": "debts"}.get(case.kind, "boundary")
    source = HERE / "tests" / folder / f"{case.source}.c"
    program = args.out / f"{case.name}.bn"
    status, output = case.status, case.output
    if case.kind in ["native", "pending", "debt"]:
        binary = args.out / f"{case.name}.native"
        built = run(["clang-18", "-O0", *case.flags, source, "-o", binary])
        assert built.returncode == 0, built.stderr.decode()
        reference = run([binary])
        status, output = reference.returncode, reference.stdout
        if case.kind in ["pending", "debt"]:
            assert (status, output, reference.stderr) == (0, b"", b""), reference
    linked = run(["python3", HERE / "link.py", source, "--libdir", args.libdir, "--front", args.front,
                  "--arena-bytes", 16384, "-o", program,
                  *(f"--clang-flag={flag}" for flag in case.flags)])
    assert linked.returncode == 0, linked.stderr.decode()
    checked = run([args.executor, "check", program])
    assert checked.returncode == 0, checked.stderr.decode()
    device = run([args.executor, "run", program, "--entry", "__bennett_entry", "--budget", 100000000])
    text = device.stdout.decode()
    console = next((s.removeprefix("console: ") for s in text.splitlines() if s.startswith("console: ")), "")
    if case.kind == "fault":
        assert device.returncode == 1 and "fault: unreachable" in text and not console, text
    else:
        actual = re.search(r"^result: (\d+)", text, re.M)
        assert device.returncode == 0 and actual and int(actual[1]) == status and console == escaped(output), (
            f"expected result {status}, console {escaped(output)!r}; observed {text!r}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--libdir", type=Path, default=ROOT / "target/musl")
    parser.add_argument("--front", type=Path, default=ROOT / "target/front/bennett-front")
    parser.add_argument("--executor", type=Path, default=ROOT / "target/debug/bennett")
    parser.add_argument("--out", type=Path, default=ROOT / ".tmp/rt/boundary-tests")
    parser.add_argument("--include-pending", action="store_true")
    parser.add_argument("--include-debts", action="store_true")
    parser.add_argument("--case", action="append", help="run just this named case, including a pending witness")
    args = parser.parse_args()
    for key in ["libdir", "front", "executor", "out"]:
        setattr(args, key, getattr(args, key).resolve())
    args.out.mkdir(parents=True, exist_ok=True)
    positive, pending, debts = cases()
    selected = positive + (pending if args.include_pending else []) + (debts if args.include_debts else [])
    if args.case:
        selected = positive + pending + debts
    if args.case:
        unknown = set(args.case) - {case.name for case in selected}
        if unknown:
            parser.error(f"unknown cases: {sorted(unknown)}")
        selected = [case for case in selected if case.name in args.case]
    failed = 0
    start = time.monotonic()
    for case in selected:
        try:
            check(case, args)
            print(f"{case.name}: PASS", flush=True)
        except (AssertionError, subprocess.TimeoutExpired) as error:
            failed += 1
            print(f"{case.name}: FAIL: {error}", flush=True)
    print(f"boundary tests: {len(selected) - failed} passed, {failed} failed ({time.monotonic() - start:.2f}s)")
    return bool(failed)


if __name__ == "__main__":
    raise SystemExit(main())
