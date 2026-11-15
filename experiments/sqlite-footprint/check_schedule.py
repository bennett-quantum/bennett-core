#!/usr/bin/env python3
"""Independent checks from blocks.tsv, blk.*.bin and the reconstructed keys.

Usage: check_schedule.py BUILD RUN SCHEDULE_DIR > check.json
Does not import schedule.py. Keeps the block traces in compact arrays and reads
the structural arrays one branch at a time, including for the 65M-event scan.
"""
import bisect
import heapq
import json
import sys
from collections import defaultdict
from graphlib import CycleError, TopologicalSorter
from pathlib import Path

import numpy as np


def majority_merge(seqs):
    # Bucket branches by their next block; a heap selects the largest bucket,
    # with the smallest block id breaking ties. Update only changed buckets.
    buckets = defaultdict(list)
    for r, seq in enumerate(seqs):
        buckets[int(seq[0])].append((r, 0))
    heap = [(-len(group), b) for b, group in buckets.items()]
    heapq.heapify(heap)
    out = []
    while heap:
        neg_count, b = heapq.heappop(heap)
        if -neg_count != len(buckets[b]) or not buckets[b]:
            continue
        out.append(b)
        ready, buckets[b] = buckets[b], []
        changed = {b}
        for r, pos in ready:
            pos += 1
            if pos < len(seqs[r]):
                nxt = int(seqs[r][pos])
                buckets[nxt].append((r, pos))
                changed.add(nxt)
        for nxt in changed:
            if buckets[nxt]:
                heapq.heappush(heap, (-len(buckets[nxt]), nxt))
    return out


def key_order(build, seqs, kdir, name):
    block_of, predecessors = {}, defaultdict(set)
    for r, seq in enumerate(seqs):
        with np.load(kdir / f"k.{r}.npz") as z:
            keys = z[name].tolist()
        assert len(keys) == len(seq)
        if name == "keyX":
            assert len(set(keys)) == len(keys), f"repeated exact key: branch {r}"
        for key, b in zip(keys, seq):
            b = int(b)
            assert block_of.setdefault(key, b) == b, "one key denotes two blocks"
            predecessors[key]
        for a, b in zip(keys, keys[1:]):
            predecessors[b].add(a)
    try:
        order = list(TopologicalSorter(predecessors).static_order())
    except CycleError:
        order = None
    acyclic = order is not None
    result = dict(keys=len(block_of), acyclic=acyclic)
    if acyclic:
        positions = {k: i for i, k in enumerate(order)}
        # A topological order must contain each branch's keys in order; since
        # every key determines one block, its block sequence also covers it.
        for r in range(len(seqs)):
            with np.load(kdir / f"k.{r}.npz") as z:
                pos = [positions[k] for k in z[name].tolist()]
            assert all(a < b for a, b in zip(pos, pos[1:]))
        result.update(cost=sum(build[block_of[k]] for k in order), slots=len(order))
    return result


def main():
    build, run, kdir = map(Path, sys.argv[1:4])
    weights = {}
    with (build / "blocks.tsv").open() as f:
        next(f)
        for line in f:
            b, _, _, n = line.rstrip("\n").split("\t")
            weights[int(b)] = int(n)
    w = np.array([weights[b] for b in range(len(weights))], np.int64)
    seqs = [np.fromfile(run / f"blk.{r}.bin", np.uint32)
            for r in range(len(list(run.glob("blk.*.bin"))))]
    lengths = [len(s) for s in seqs]
    instr = [int(w[s].sum()) for s in seqs]
    mean = sum(instr) / len(instr)
    assert len(w) <= 1 << 16, "block ids exceed schedule.py's packed field"
    assert max(lengths) < 1 << 18, "region offsets may exceed the packed field"
    pairs, reached = set(), set()
    lo, hi, max_count = {}, {}, np.zeros(len(w), np.int64)
    for seq in seqs:
        # Arithmetic pair encoding, independent of schedule.py's bit packing.
        pairs.update((np.arange(len(seq), dtype=np.int64) * len(w) + seq).tolist())
        blocks, first, count = np.unique(seq, return_index=True, return_counts=True)
        reverse = {int(b): len(seq) - 1 - int(i) for b, i in
                   zip(*np.unique(seq[::-1], return_index=True))}
        for b, i, c in zip(blocks.tolist(), first.tolist(), count.tolist()):
            reached.add(b)
            lo[b] = min(lo.get(b, i), i)
            hi[b] = max(hi.get(b, reverse[b]), reverse[b])
            max_count[b] = max(max_count[b], c)
    rw = sum(weights[b] for b in reached)
    costs = {
        "0b_step_exact": sum(weights[p % len(w)] for p in pairs),
        "0c_all_reached_blocks": max(lengths) * rw,
        "0d_all_reached_instr": max(instr) * rw,
        "2a_step_interval": sum(weights[b] * (hi[b] - lo[b] + 1) for b in reached),
    }
    del pairs
    sup = majority_merge(seqs)
    occurrences = defaultdict(list)
    for i, b in enumerate(sup):
        occurrences[b].append(i)
    for r, seq in enumerate(seqs):
        pos = -1
        for b in seq:
            where = occurrences[int(b)]
            j = bisect.bisect_right(where, pos)
            assert j < len(where), f"merge misses branch {r}"
            pos = where[j]
    costs["0a_supersequence"] = sum(weights[b] for b in sup)
    report = json.loads((kdir / "schedule.json").read_text())
    assert report["run_mean"] == mean
    assert report["run_min"] == min(instr)
    assert report["run_max"] == max(instr)
    assert report["blocks_mean"] == sum(lengths) / len(lengths)
    assert report["blocks_max"] == max(lengths)
    analysis = run / "analysis.json"
    if analysis.exists():
        steps = json.loads(analysis.read_text())["steps"]
        for name, values in (("blocks", lengths), ("llvm_instr", instr)):
            for stat, value in (("min", min(values)), ("max", max(values)),
                                ("mean", sum(values) / len(values))):
                assert steps[name][stat] == value, (name, stat)
    for name, cost in costs.items():
        assert int(report["families"][name]["cost"]) == cost, name
    assert report["families"]["0a_supersequence"]["steps"] == len(sup)
    assert json.loads((kdir / "trace.json").read_text())["issues"] == {}
    keys = {name: key_order(weights, seqs, kdir, name) for name in ("keyX", "keyN")}
    assert keys["keyX"]["acyclic"], "exact-key cost lacks a common waiting order"
    assert keys["keyX"]["cost"] == report["families"]["3a_struct_exact"]["cost"]
    json.dump(dict(N=len(seqs), mean=mean, costs=costs, greedy_slots=len(sup),
                   lower_bound_cost=int(w @ max_count), lower_bound_slots=int(max_count.sum()),
                   key_order=keys), sys.stdout, indent=2)
    print()


if __name__ == "__main__":
    main()
