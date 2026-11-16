#!/usr/bin/env python3
"""Native/stdout/status comparison and fresh-setup per-query device costs.

Run only stage 1 by default. --reverse is the separate stage 2 run.
Build with build.sh first. JSON is raw evidence, kept under target/ by default.
"""
import argparse
import ast
import json
from pathlib import Path
import re
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]


def run(argv):
    start = time.monotonic()
    p = subprocess.run([str(x) for x in argv], capture_output=True, text=True)
    return p, time.monotonic() - start


def device(path, entry='__bennett_entry', reverse=False):
    cmd = [ROOT/'target/debug/bennett', 'run', path, '--entry', entry,
           '--budget', '100000000', '--stats']
    if reverse:
        cmd.append('--reverse')
    p, wall = run(cmd)
    result = re.search(r'^result: (\d+)', p.stdout, re.M)
    console = re.search(r'^console: (".*")$', p.stdout, re.M)
    steps = re.search(r'^steps: (\d+)', p.stdout, re.M)
    return dict(status=int(result[1]) if result else None,
                console=ast.literal_eval('b'+console[1]).decode() if console else '',
                steps=int(steps[1]) if steps else None,
                process_status=p.returncode, seconds=wall, report=p.stdout, stderr=p.stderr)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('build', type=Path)
    ap.add_argument('--reverse', action='store_true')
    ap.add_argument('--stack-sweep', action='store_true')
    args = ap.parse_args()
    build = args.build.resolve()
    native, _ = run([build/'native'])
    observed = device(build/'program.bn', reverse=args.reverse)
    observed['native_status'] = native.returncode
    observed['matches_native'] = (observed['process_status'] == 0 and
                                  observed['status'] == native.returncode and
                                  observed['console'] == native.stdout)
    assert observed['matches_native'], observed
    if args.reverse:
        assert 'classical state identical, amplitudes within 0e0' in observed['report'], observed
        journal = int(re.search(r'^journal: (\d+) bytes', observed['report'], re.M)[1])
        # This driver executes no marks, reads of qubits, setjmps or gates:
        # journal bytes = one one-byte Undo tag per step plus 16 per u128 word.
        assert (journal-observed['steps']) % 16 == 0
        observed['journal_words'] = (journal-observed['steps'])//16
        observed['journal_words_per_step'] = observed['journal_words']/observed['steps']
    else:
        text = (build/'program.bn').read_text()
        setup = '\nfunc @sod_setup() -> i32 {\nentry:\n %s = call @experiment_setup()\n ret %s\n}\n'
        probe = build/'probe.bn'
        probe.write_text(text+setup)
        baseline = device(probe, 'sod_setup')
        assert baseline['status'] == 0, baseline
        costs = []
        for line in native.stdout.splitlines():
            fields = line.split()
            if fields[0] not in ['arith', 'like_name', 'like_note', 'scan']:
                continue
            q = ['arith', 'like_name', 'like_note', 'scan'].index(fields[0])
            r, expected = map(int, fields[1:])
            entry = ('\nfunc @sod_probe() -> i32 {\nentry:\n'
                     ' %s = call @experiment_setup()\n'
                     f' %v = call @experiment_query(i32 {q}, i32 {r})\n ret %v\n}}\n')
            probe.write_text(text+entry)
            result = device(probe, 'sod_probe')
            assert result['status'] == expected and result['process_status'] == 0, result
            costs.append(dict(query=fields[0], row=r, result=expected,
                              steps=result['steps']-baseline['steps']))
        observed['queries'] = costs
        observed['setup_steps'] = baseline['steps']
        observed['text_bytes'] = (build/'program.bn').stat().st_size
        if args.stack_sweep:
            total = int(re.search(r'^memory (\d+)', text, re.M)[1])
            floor = total-1048576  # sweep is for the default --stack-bytes build
            low, high = 0, 1048576
            trials = []
            while high-low > 1:
                middle = (high+low)//2
                probe.write_text(re.sub(r'^memory \d+', f'memory {floor+middle}', text, count=1, flags=re.M))
                result = device(probe)
                ok = result['process_status'] == 0 and result['console'] == native.stdout
                assert ok or 'fault: stack_overflow' in result['report'], result
                trials.append(dict(stack=middle, passed=ok, steps=result['steps']))
                if ok:
                    high = middle
                else:
                    low = middle
            observed['minimum_stack_bytes'] = high
            observed['stack_trials'] = trials
        probe.unlink()
    (build/('reverse.json' if args.reverse else 'forward.json')).write_text(json.dumps(observed, indent=2)+'\n')
    print(json.dumps(observed, indent=2))


if __name__ == '__main__':
    main()
