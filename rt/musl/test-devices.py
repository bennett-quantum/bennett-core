#!/usr/bin/env python3
"""Files and deterministic device inputs: native comparison and full executor reversal.

Requires built front, CLI and musl; separate from the fast cargo/front suites.
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
OUT = ROOT / ".tmp/rt/device-tests"


def run(args, **kw):
    result = subprocess.run([str(a) for a in args], capture_output=True, text=True, **kw)
    assert result.returncode == 0, (args, result.stdout, result.stderr)
    return result.stdout


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    tape = bytes((17*i+3) % 256 for i in range(512))
    (OUT / "entropy.bin").write_bytes(tape)
    (OUT / "files.json").write_text(json.dumps({"/fixtures/input": "610062ff63"}))
    started = time.monotonic()
    rows = []
    for name in ("files", "files-edge", "input-image", "clocks", "entropy", "entropy-exhaust"):
        starts = (1234999999998, (1 << 64)-2) if name == "clocks" else (0,)
        for start in starts:
            for level in ("-O0", "-O2"):
                bn = OUT / f"{name}-{start}{level}.bn"
                inputs = ["--clock-start", start, "--entropy-image", OUT / "entropy.bin"]
                if name == "input-image":
                    inputs += ["--file-image", OUT / "files.json"]
                if name == "entropy-exhaust":
                    inputs = []  # empty image; an ordinary named fault, also fully reversed
                run(["python3", HERE / "link.py", HERE / f"tests/{name}.c", "-o", bn,
                     f"--clang-flag={level}", "--clang-flag=-fno-vectorize", "--clang-flag=-fno-slp-vectorize", *inputs])
                run([ROOT / "target/debug/bennett", "check", bn])
                result = subprocess.run([str(ROOT / "target/debug/bennett"), "run", str(bn),
                                         "--entry", "__bennett_entry", "--reverse", "--stats", "--budget", "10000000"],
                                        capture_output=True, text=True)
                assert "returned to entry: classical state identical" in result.stdout, result
                if name == "entropy-exhaust":
                    assert result.returncode == 1 and "fault: unreachable" in result.stdout, result
                else:
                    assert result.returncode == 0 and "result: 0\n" in result.stdout, result
                if name in ("files", "files-edge", "input-image", "clocks"):
                    native = OUT / f"{name}.native"
                    extra = ["-DNATIVE_INPUTS", f"-DSTART={start}ULL"] if name == "clocks" else []
                    run(["clang-18", level, *extra, HERE / f"tests/{name}.c", "-o", native])
                    with tempfile.TemporaryDirectory(dir=OUT) as directory:
                        cwd = Path(directory)
                        if name == "input-image":
                            (cwd / "fixtures").mkdir()
                            (cwd / "fixtures/input").write_bytes(bytes.fromhex("610062ff63"))
                        reference = run([native], cwd=cwd, env={**os.environ}, preexec_fn=lambda: os.umask(0o22))
                    from test import escaped
                    console = next((x[9:] for x in result.stdout.splitlines() if x.startswith("console: ")), "")
                    assert console == escaped(reference.encode()), (name, console, reference)
                steps = next(x[7:] for x in result.stdout.splitlines() if x.startswith("steps: "))
                row = f"{name}\t{start}\t{level}\t{steps}\t{bn.stat().st_size}"
                rows.append(row)
                (OUT / "results.tsv").write_text("case\tclock_start\tlevel\tsteps\ttext_bytes\n" + "\n".join(rows) + "\n")
                (bn.with_suffix(".run.txt")).write_text(result.stdout)
                print(row, flush=True)
    print(f"device tests: {len(rows)} runs, {time.monotonic()-started:.2f}s", flush=True)


if __name__ == "__main__":
    main()
