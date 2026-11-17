#!/usr/bin/env python3
"""Compare copied sigjmp_buf stdout/status with native glibc and musl at O0/O2."""
import argparse
import ast
import json
import os
from pathlib import Path
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
NATIVE_TIMEOUT_SECONDS = 30
DEVICE_TIMEOUT_SECONDS = 180


def checked(command, timeout=DEVICE_TIMEOUT_SECONDS):
    result = subprocess.run([str(arg) for arg in command], capture_output=True, timeout=timeout)
    assert result.returncode == 0, (command, result.returncode, result.stderr.decode(errors="replace"))
    return result


def rows(data):
    lines = data.decode().splitlines()
    result = dict(line.split(": ") for line in lines)
    assert len(result) == len(lines) == 576, "all matrix rows must execute exactly once"
    return result


def native(args, source, tag, level):
    references = {}
    for libc, compiler in [("glibc", os.environ.get("CLANG", "clang-18")), ("musl", args.musl_cc)]:
        executable = args.out / f"{tag}-{libc}{level}"
        checked([compiler, level, "-fno-vectorize", "-fno-slp-vectorize", "-static", source, "-o", executable])
        reference = subprocess.run([str(executable)], capture_output=True, timeout=NATIVE_TIMEOUT_SECONDS)
        assert reference.returncode >= 0, (executable, "native signal", reference.returncode)
        executable.with_suffix(".stdout").write_bytes(reference.stdout)
        assert not reference.stderr, reference.stderr
        references[libc] = (reference.returncode, reference.stdout)
    return references


def device(args, source, tag, level):
    program = args.out / f"{tag}-device{level}.bn"
    checked(["python3", HERE / "link.py", source, "--libdir", args.libdir, "--front", args.front,
             f"--clang-flag={level}", "--clang-flag=-fno-vectorize", "--clang-flag=-fno-slp-vectorize", "-o", program])
    checked([args.executor, "check", program])
    result = checked([args.executor, "run", program, "--entry", "__bennett_entry", "--budget", 100000000])
    program.with_suffix(".stdout").write_bytes(result.stdout)
    lines = result.stdout.decode().splitlines()
    status = int(next(line.removeprefix("result: ") for line in lines if line.startswith("result: ")))
    console = next((line.removeprefix("console: ") for line in lines if line.startswith("console: ")), '""')
    return status, ast.literal_eval(console).encode("latin1")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--libdir", type=Path, default=ROOT / "target/musl")
    parser.add_argument("--musl-cc", type=Path, default=ROOT / "target/musl-native/install/bin/musl-clang")
    parser.add_argument("--front", type=Path, default=ROOT / "target/front/bennett-front")
    parser.add_argument("--executor", type=Path, default=ROOT / "target/debug/bennett")
    parser.add_argument("--out", type=Path, default=ROOT / "target/front-tests/sigjmp-copy")
    parser.add_argument("--partial-record", action="store_true", help="run the known partial-record defect witness")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    tag = "sigjmp_partial_record" if args.partial_record else "sigjmp_copy_matrix"
    source = ROOT / f"front/tests/c/{tag}.c"
    references = {}
    report = {}
    failed = False
    for level in ["-O0", "-O2"]:
        reference = native(args, source, tag, level)
        references[level] = reference
        result = device(args, source, tag, level)
        if args.partial_record:
            assert reference["glibc"] == reference["musl"] == (0, b""), (level, reference)
            report[level] = {"glibc_status": reference["glibc"][0], "musl_status": reference["musl"][0],
                             "device_status": result[0], "stdout_equal": result[1] == reference["glibc"][1]}
            failed |= result != reference["glibc"]
            print(f"{tag} {level}: glibc/musl {reference['glibc']!r}, device {result!r}", flush=True)
        else:
            glibc = rows(reference["glibc"][1])
            musl = rows(reference["musl"][1])
            device_rows = rows(result[1])
            differences = {key: {"glibc": value, "musl": musl[key]}
                           for key, value in glibc.items() if musl[key] != value}
            findings = {key: {"glibc": value, "musl": musl[key], "device": device_rows[key]}
                        for key, value in glibc.items() if device_rows[key] != value}
            report[level] = {"rows": len(glibc), "libc_differences": differences, "findings": findings}
            assert reference["glibc"][0] == reference["musl"][0] == 0, reference
            assert differences, "the matrix must exercise the decided glibc/musl distinction"
            failed |= result != reference["glibc"]
            print(f"{level}: {len(glibc)} rows, {len(findings)} device/glibc differences; "
                  f"{len(differences)} glibc/musl differences", flush=True)
    (args.out / f"{tag}-report.json").write_text(json.dumps(report, indent=2) + "\n")
    assert references["-O0"]["glibc"] == references["-O2"]["glibc"], "glibc optimization changes observations"
    if not args.partial_record:
        musl_o0 = rows(references["-O0"]["musl"][1])
        musl_o2 = rows(references["-O2"]["musl"][1])
        print(f"native musl changes {sum(musl_o0[key] != value for key, value in musl_o2.items())} rows between O0/O2", flush=True)
    assert not failed, report if args.partial_record else f"device differs from glibc; see {tag}-report.json"


if __name__ == "__main__":
    main()
