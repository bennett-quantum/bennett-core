#!/usr/bin/env python3
"""Run C ladder evidence; resource stops never count as acceptance passes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
TESTS = {
    1: ["scalar_c_exhaustive_int8_native_forward_and_reverse",
        "scalar_c_independent_exhaustive_int8_native_and_reverse"],
    2: ["hashtable_native_forward_and_byte_exact_reverse",
        "hashtable_independent_native_forward_and_byte_exact_reverse"],
    3: ["musl_hello_native_reset_and_deterministic_tape_replay"],
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
    parser.add_argument("--reference-memory-mib", type=int, default=1024,
                        help="address-space cap for the independent table process")
    parser.add_argument("--reference-timeout", type=float, default=120,
                        help="seconds for the independent table process, a resource bound, not a semantic criterion")
    args = parser.parse_args()
    if args.reference_memory_mib <= 0 or args.reference_timeout <= 0:
        parser.error("resource bounds must be positive")
    if args.slot:
        # Acquire the slot before starting any per-process measurement/timeout.
        command = [str(args.slot.resolve()), "heavy", sys.executable, str(Path(__file__).resolve()),
                   "--reference-memory-mib", str(args.reference_memory_mib),
                   "--reference-timeout", str(args.reference_timeout)]
        for rung in args.rung or []:
            command.extend(["--rung", str(rung)])
        return subprocess.call(command, cwd=ROOT)
    for name, expected in SOURCES.items():
        data = (Path(__file__).resolve().parent / "fixtures" / name).read_bytes()
        if hashlib.sha256(data).hexdigest() != expected:
            raise SystemExit(f"fixture provenance mismatch: {name}")
    out = ROOT / ".tmp/northstar-ladder"
    out.mkdir(parents=True, exist_ok=True)
    env = {**os.environ, "CARGO_BUILD_JOBS": "3", "CMAKE_BUILD_PARALLEL_LEVEL": "3"}
    env.pop("DISPLAY", None)
    env.pop("WAYLAND_DISPLAY", None)
    env.pop("DBUS_SESSION_BUS_ADDRESS", None)
    report = {
        "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "scope": "C only, source-input entries; see README for domains and limitations",
        "rungs": {},
    }
    # Main can advance while this runner waits for its slot. Refresh build
    # products only after acquisition so measured source and binaries agree.
    prerequisites = [
        ["cmake", "-S", str(ROOT / "front"), "-B", str(ROOT / "target/front")],
        ["cmake", "--build", str(ROOT / "target/front"), "--parallel", "3"],
        [sys.executable, str(ROOT / "rt/musl/build.py"), "--jobs", "3"],
        ["cargo", "test", "--release", "-p", "bennett-front-tests", "--test", "ladder", "--no-run"],
    ]
    report["prerequisites"] = []
    with (out / "prerequisites.txt").open("wb") as log:
        for command in prerequisites:
            started = time.monotonic()
            result = subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
            report["prerequisites"].append({"command": command, "exit": result.returncode,
                                             "seconds": time.monotonic() - started})
            (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
            print(f"prerequisite {Path(command[0]).name}: exit {result.returncode}", flush=True)
            if result.returncode:
                return 1
    for rung in args.rung or TESTS:
        row = {"acceptance": "RUNNING", "stages": {}}
        report["rungs"][rung] = row
        for name in TESTS[rung]:
            command = ["cargo", "test", "--release", "-p", "bennett-front-tests", "--test", "ladder", name,
                       "--", "--exact", "--nocapture", "--test-threads=1"]
            command.append("--ignored")
            bounded = rung == 2 and "independent" in name
            stage_env = {**env, "BENNETT_LADDER_REFERENCE_TABLE": "bounded"} if bounded else env
            limit = args.reference_memory_mib * 1024 * 1024

            def memory_limit():
                resource.setrlimit(resource.RLIMIT_AS, (limit, limit))
                resource.setrlimit(resource.RLIMIT_CORE, (0, 0))

            started = time.monotonic()
            timed_out = False
            with subprocess.Popen(command, cwd=ROOT, env=stage_env,
                                  stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                  start_new_session=bounded,
                                  preexec_fn=memory_limit if bounded else None) as process:
                try:
                    data, _ = process.communicate(timeout=args.reference_timeout if bounded else None)
                    code = process.returncode
                except subprocess.TimeoutExpired:
                    # Cargo/slot wrappers have children: stop the entire isolated
                    # process group, not just the wrapper, and drain its output.
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass  # The group finished at the timeout boundary.
                    data, _ = process.communicate()
                    code, timed_out = None, True
            log = out / f"{name}.txt"
            log.write_bytes(data)
            print(data.decode(errors="replace"), end="", flush=True)
            # Reject cargo's successful zero-test result.
            ran = f"test {name} ... ".encode() in data and b"1 passed; 0 failed" in data
            passed = code == 0 and ran and not timed_out
            resource_stop = timed_out or (bounded and b"memory allocation of " in data)
            row["stages"][name] = {
                "test_exit": code, "selected_test_passed": passed,
                "selected_test_started": f"test {name} ... ".encode() in data,
                "seconds": time.monotonic() - started, "timed_out": timed_out,
                "resource_stop": resource_stop,
                "memory_limit_mib": args.reference_memory_mib if bounded else None,
                "log": str(log.relative_to(ROOT)),
            }
            # Persist immediately; a resource stop is not a semantic verdict.
            states = list(row["stages"].values())
            row["acceptance"] = ("FAIL" if any(not s["selected_test_passed"] and not s["resource_stop"] for s in states)
                                 else "UNMEASURED" if any(s["resource_stop"] for s in states)
                                 else "PASS" if len(states) == len(TESTS[rung]) else "RUNNING")
            (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
            print(f"rung {rung}, {name}: {row['acceptance']}", flush=True)
    return 0 if all(row["acceptance"] == "PASS" for row in report["rungs"].values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())
