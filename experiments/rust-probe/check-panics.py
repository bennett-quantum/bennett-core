#!/usr/bin/env python3
"""Execute every overflowing pair in the native checked build (panic=abort)."""
import signal
import subprocess
import sys

native = sys.argv[1]
count = 0
for a in range(256):
    for b in range(256 - a, 256):
        result = subprocess.run(
            [native, "--panic-one", str(a), str(b)],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        assert result.returncode == -signal.SIGABRT, (a, b, result.returncode)
        assert result.stdout == b"", (a, b, result.stdout)
        count += 1
print(f"native_overflow_aborts\t{count}", flush=True)
