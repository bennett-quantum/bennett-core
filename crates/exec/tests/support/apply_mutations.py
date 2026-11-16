#!/usr/bin/env python3
"""Mutate only an ignored scratch copy. Run from the checkout with python3 FILE."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--only', help='run one mutation plus the unmodified control')
parser.add_argument('--require-all', action='store_true', help='fail when comparison misses a mutation')
args = parser.parse_args()

ROOT = Path(__file__).resolve().parents[4]
SCRATCH = ROOT / '.tmp' / 'apply-mutations'
SCRATCH.mkdir(parents=True, exist_ok=True)
tracked = subprocess.check_output(['git', '-C', str(ROOT), 'ls-files', '-z']).decode().split('\0')
for name in filter(None, tracked):
    src, dst = ROOT / name, SCRATCH / name
    if src.is_file():
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(src, dst)
probe = SCRATCH / 'crates/exec/tests/comparison_probe.rs'
probe.write_text('''use bennett_exec::{ApplyMode, Executor, Options, Outcome};
#[test]
fn comparison_probe() {
 let p = bennett_ir::load("memory 512\\nfunc @f() -> i1 {\\nentry:\\nret 1\\n}\\nfunc @main() {\\nentry:\\n%o = qalloc 1\\n%e = qalloc 5\\napply @f() -> %o, %e budget 4\\nret\\n}").expect("program");
 let mut e = Executor::new(&p,p.function_named("main").expect("main"),&[],Options {reversible:true,..Options::default()}).expect("entry");
 e.set_apply_mode(ApplyMode::Compare);
 // Test only the apply: a later instruction must not be the mutation detector.
 for _ in 0..3 { assert_eq!(e.step(), None); }
 println!("steps={}, budget={:?}",e.stats().steps,e.stats().budget);
 assert_eq!(e.outcome(), None);
 let _ = Outcome::Returned(None);
}
''')
path = SCRATCH / 'crates/exec/src/region.rs'
original = path.read_text()
needle = '        self.end_history_region(outer);\n        match stop {'
assert original.count(needle) == 1
mutations = {
    'control': '',
    'output': 'self.quantum.xor_register(out, 1);',
    'memory': 'self.memory.write(0, &[1]);',
    'slot': 'self.slots[0] ^= 1;',
    'journal': 'self.journal.as_mut().expect("journal").clear();',
    'tape': 'self.tape.push(1);',
    'region_steps': 'self.region_steps += 1;',
    'region_faults': 'self.region_faults += 1;',
    'fault_max_weight': 'self.region_fault_max_weight += 1.0;',
    'rerun_steps': 'self.rerun_steps += 1;',
    'mismatches': 'self.mismatches += 1;',
    'split_memory_bytes': 'self.split_memory_bytes += 1;',
    'split_journal_bytes': 'self.split_journal_bytes += 1;',
    'rewound_steps': 'self.rewound_steps += 1;',
    'splits': 'self.splits += 1;',
    'fault_weight': 'self.region_fault_weight += 1.0;',
    'status': 'self.status = crate::worlds::Status::Ended;',
    'partial': 'self.partial = !self.partial;',
    'pending': 'self.pending = !self.pending;',
    'merges': 'self.merges += 1;',
    'peak_worlds': 'self.peak_worlds += 1;',
    'steps': 'self.steps += 1;',
    'budget': 'self.budget = Some(0);',
    'in_region': 'self.in_region = true;',
    'backup': 'self.backups.push(self.clone_world(None));',
}
env = dict(os.environ, CARGO_BUILD_JOBS='3', RUST_TEST_THREADS='1')
env.pop('BENNETT_APPLY', None)
if args.only:
    assert args.only in mutations, 'unknown mutation: '+args.only
results = []
try:
    for name, code in mutations.items():
        if args.only and name not in ('control', args.only):
            continue
        path.write_text(original.replace(needle, '        self.end_history_region(outer);\n        '+code+'\n        match stop {'))
        result = subprocess.run(['cargo', '--config', 'profile.dev.package.bennett-exec.opt-level=0', 'test', '-p', 'bennett-exec', '--test', 'comparison_probe', '--', '--nocapture'], cwd=SCRATCH, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        (SCRATCH / (name+'.log')).write_text(result.stdout)
        assert 'could not compile' not in result.stdout, result.stdout
        detected = 'apply @f:' in result.stdout and result.returncode != 0
        assert result.returncode == 0 or detected, result.stdout
        print(name, 'CAUGHT' if detected else 'PASSED', flush=True)
        results.append((name, detected))
finally:
    path.write_text(original)
assert not results[0][1], 'control must pass'
# Report omissions without silently asserting them correct.
missed = [name for name, caught in results[1:] if not caught]
print('MISSED: '+', '.join(missed), flush=True)
if args.require_all:
    assert not missed, 'comparison accepted corrupted history state: '+', '.join(missed)
