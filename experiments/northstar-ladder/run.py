#!/usr/bin/env python3
"""Run C ladder evidence; an unmet §8 clause keeps the acceptance gate red."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
TESTS = {
    1: "scalar_c_exhaustive_int8_native_forward_and_reverse",
    2: "hashtable_native_forward_and_byte_exact_reverse",
    3: "musl_hello_native_reset_and_deterministic_tape_replay",
}
SOURCES = {
    "scalars.c": "171bb4f3fcd4e5aec23250c48ee4e5c8b07331357263c1052a18ed6025fa5f78",
    "hashtable.c": "8cbd5a3698aea0da1f7187b97be34a0ed33743394f045c49c943323f94abd4b9",
    "main_golden.c": "83c6d020567460b10e0f70aec25399b34f5dda8e2c2feb47f157f71fff0e8820",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rung", type=int, choices=TESTS, action="append")
    parser.add_argument("--slot", type=Path, help="optional shared-machine slot wrapper")
    args = parser.parse_args()
    for name, expected in SOURCES.items():
        data = (Path(__file__).resolve().parent / "fixtures" / name).read_bytes()
        if hashlib.sha256(data).hexdigest() != expected:
            raise SystemExit(f"fixture provenance mismatch: {name}")
    out = ROOT / ".tmp/northstar-ladder"
    out.mkdir(parents=True, exist_ok=True)
    env = {**os.environ, "CARGO_BUILD_JOBS": "2", "CMAKE_BUILD_PARALLEL_LEVEL": "2"}
    env.pop("DISPLAY", None)
    env.pop("WAYLAND_DISPLAY", None)
    report = {
        "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "scope": "C only, source-input entries; see README for domains and limitations",
        "rungs": {},
    }
    for rung in args.rung or TESTS:
        command = ["cargo", "test", "-p", "bennett-front-tests", "--test", "ladder", TESTS[rung],
                   "--", "--exact", "--nocapture", "--test-threads=1"]
        if rung != 1:
            command.append("--ignored")
        if args.slot:
            command = [str(args.slot.resolve()), "heavy", *command]
        started = time.monotonic()
        result = subprocess.run(command, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        log = out / f"rung-{rung}.txt"
        log.write_bytes(result.stdout)
        print(result.stdout.decode(errors="replace"), end="", flush=True)
        # Cargo's zero-tests exit is successful: require the exact selected test
        # to have actually run before treating it as measured evidence.
        ran = f"test {TESTS[rung]} ... ".encode() in result.stdout and b"1 passed; 0 failed" in result.stdout
        tested = result.returncode == 0 and ran
        missing = "second independent device executor" if rung in (1, 2) else None
        report["rungs"][rung] = {
            "test_exit": result.returncode,
            "selected_test_passed": tested,
            "seconds": time.monotonic() - started,
            "acceptance": "PASS" if tested and not missing else "FAIL" if not tested else "UNMET",
            "unmet_clause": missing,
            "log": str(log.relative_to(ROOT)),
        }
        # Persist after each rung, including a failure, so partial runs survive.
        (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(f"rung {rung}: {report['rungs'][rung]['acceptance']}", flush=True)
    return 0 if all(row["acceptance"] == "PASS" for row in report["rungs"].values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())
