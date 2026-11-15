#!/usr/bin/env python3
"""Compare pre-hook, hook-off and hook-on executors on the existing executable test suites.

Run from any directory with Python 3; needs the usual Rust/front test dependencies. Builds a source
copy under .tmp (no worktree/branch changes). Region and outer run endpoints record literal journals,
outcomes and Stats; outer endpoints also record a std DefaultHasher fingerprint of full machine state.
Tests that deliberately request measurement are excluded: their tables are reproduced separately.
Logs and traces stay in .tmp; any difference fails this check. --test-threads=1 and at most 3 build jobs.
"""

import io
import os
from pathlib import Path
import shutil
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[2]
SCRATCH = ROOT / ".tmp/journal-check/off-audit"
BASE = "9817fe0^"

TRACE = r'''
impl Executor<'_> {
    pub(crate) fn journal_check_trace(&self) {
        use std::hash::{Hash, Hasher};
        use std::io::{BufWriter, Write};
        use std::sync::{Mutex, OnceLock};
        static TRACE: OnceLock<Mutex<BufWriter<std::fs::File>>> = OnceLock::new();
        let output = TRACE.get_or_init(|| Mutex::new(BufWriter::new(std::fs::OpenOptions::new().create(true).append(true)
            .open(std::env::var_os("JOURNAL_CHECK_TRACE").expect("trace path")).expect("trace file"))));
        let mut output = output.lock().expect("trace lock");
        if self.in_region {
            writeln!(output, "region {:?} steps={} journal={:?}", self.outcome, self.steps, self.journal).expect("trace");
        } else {
            let mut state = std::hash::DefaultHasher::new();
            self.memory.hash(&mut state);
            self.slots.hash(&mut state);
            format!("{:?}", self.frames).hash(&mut state);
            self.stack_pointer.hash(&mut state);
            self.tape.hash(&mut state);
            format!("{:?}", self.quantum.registers()).hash(&mut state);
            for (config, amp) in self.quantum.amplitudes() {
                config.hash(&mut state);
                amp.re.to_bits().hash(&mut state);
                amp.im.to_bits().hash(&mut state);
            }
            writeln!(output, "outer {:?} {:?} journal={:?} state={:016x}", self.outcome, self.stats(), self.journal, state.finish()).expect("trace");
        }
        output.flush().expect("flush trace");
    }
}
impl Drop for Executor<'_> {
    fn drop(&mut self) { self.journal_check_trace(); }
}
'''


def run(command, **kwargs):
    return subprocess.run(command, cwd=ROOT, check=True, **kwargs)


def instrument(path, enabled):
    executor = path / "executor.rs"
    text = executor.read_text()
    text = text.replace("return outcome;\n", "self.journal_check_trace();\n                return outcome;\n", 1)
    if enabled:
        text = text.replace("        Ok(executor)\n", "        executor.measure_journal_content(false);\n        Ok(executor)\n", 1)
    executor.write_text(text + TRACE)
    region = path / "region.rs"
    text = region.read_text().replace("        self.unwind();", "        self.journal_check_trace();\n        self.unwind();", 1)
    region.write_text(text)


def main():
    SCRATCH.mkdir(parents=True, exist_ok=True)
    source = SCRATCH / "source"
    if source.exists():
        shutil.rmtree(source)
    source.mkdir()
    archive = run(["git", "archive", "HEAD"], stdout=subprocess.PIPE).stdout
    with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
        bundle.extractall(source, filter="data")
    env = {**os.environ, "CARGO_BUILD_JOBS": "3", "CARGO_TARGET_DIR": str(SCRATCH / "target"),
           "BENNETT_FRONT": str(ROOT / "target/front/bennett-front")}
    for key in ["DISPLAY", "WAYLAND_DISPLAY"]:
        env.pop(key, None)
    original = {p.name: p.read_text() for p in (source / "crates/exec/src").glob("*.rs")}
    targets = []
    for crate, package in [("crates/exec", "bennett-exec"), ("front/tests", "bennett-front-tests")]:
        names = sorted(p.stem for p in (source / crate / "tests").glob("*.rs") if not p.stem.startswith("journal_"))
        targets.append((package, names))
    for mode in ["legacy", "off", "on-counts"]:
        src = source / "crates/exec/src"
        for name, text in original.items():
            if mode == "legacy":
                result = subprocess.run(["git", "show", f"{BASE}:crates/exec/src/{name}"], cwd=ROOT, capture_output=True)
                if result.returncode:
                    (src / name).unlink(missing_ok=True)
                    continue
                text = result.stdout.decode()
            (src / name).write_text(text)
        instrument(src, mode == "on-counts")
        trace = SCRATCH / f"{mode}.trace"
        trace.unlink(missing_ok=True)
        env["JOURNAL_CHECK_TRACE"] = str(trace)
        with (SCRATCH / f"{mode}.log").open("w") as log:
            for package, names in targets:
                command = ["cargo", "test", "--manifest-path", str(source / "Cargo.toml"), "-p", package]
                for name in names:
                    command += ["--test", name]
                command += ["--", "--nocapture", "--test-threads=1"]
                run(command, env=env, stdout=log, stderr=subprocess.STDOUT)
        print(f"{mode}: tests passed, {trace.stat().st_size} trace bytes", flush=True)
    reference = SCRATCH / "legacy.trace"
    for mode in ["off", "on-counts"]:
        run(["cmp", str(reference), str(SCRATCH / f"{mode}.trace")])
        print(f"legacy == {mode}: exact trace match", flush=True)
    print(f"traces and commands' logs: {SCRATCH}")


if __name__ == "__main__":
    main()
