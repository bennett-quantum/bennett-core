#!/usr/bin/env python3
"""Emit a portable measured table (no artifact paths or process transcripts)."""
import argparse
import csv
import json
from pathlib import Path
import re
import sys


def match(pattern, text):
    found = re.search(pattern, text)
    return found[1] if found else ''


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('reports', nargs='+', type=Path)
    args = ap.parse_args()
    fields = ['n', 'apply', 'shape', 'marked', 'mode', 'prep', 'iterations', 'passed',
              'wall_s', 'peak_rss_kib', 'forward_s', 'reverse_s', 'steps', 'journal_bytes',
              'region_steps', 'region_work', 'region_journal_sum_bytes', 'region_overlay_sum_bytes',
              'peak_qubits', 'peak_configurations', 'dense_error', 'success', 'theory']
    writer = csv.DictWriter(sys.stdout, fieldnames=fields, delimiter='\t', lineterminator='\n')
    writer.writeheader()
    for path in args.reports:
        data = json.loads(path.read_text())
        for c in data['cases']:
            stdout = c['stdout']
            # RegionCost's counters are cumulative sums of per-run high water,
            # not simultaneous storage. Outer journal is a separate metric.
            cost = match(r'region_cost: RegionCost \{ ([^}]+)', stdout)
            outer = stdout.split('}, region_faults:', 1)[-1]
            row = dict(n=data['n'], apply=data.get('apply', 'NA'), **{k: c[k] for k in
                       ['shape', 'marked', 'mode', 'prep', 'iterations', 'passed']},
                       wall_s=f"{c['seconds']:.6f}", peak_rss_kib=c['peak_rss_kib'],
                       forward_s=match(r'forward_seconds (\S+)', stdout),
                       reverse_s=match(r'reverse_seconds (\S+)', stdout),
                       steps=match(r'stats Stats \{ steps: (\d+)', stdout),
                       journal_bytes=match(r'journal_bytes: Some\((\d+)\)', outer),
                       region_steps=match(r'region_steps: (\d+)', stdout),
                       region_work=match(r'work: (\d+)', cost),
                       region_journal_sum_bytes=match(r'journal_bytes: (\d+)', cost),
                       region_overlay_sum_bytes=match(r'overlay_bytes: (\d+)', cost),
                       peak_qubits=match(r'peak_live_qubits: (\d+)', stdout),
                       peak_configurations=match(r'peak_configurations: (\d+)', stdout),
                       dense_error=match(r'dense_deviation (\S+)', stdout),
                       success=match(r'success (\S+)', stdout), theory=match(r'theory (\S+)', stdout))
            writer.writerow({k: "NA" if v is None or v == "" else v for k, v in row.items()})


if __name__ == '__main__':
    main()
