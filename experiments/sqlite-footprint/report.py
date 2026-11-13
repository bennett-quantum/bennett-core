#!/usr/bin/env python3
"""Markdown tables from RUNS/<N>/<query>/analysis.json.   report.py RUNS > results.md"""
import json
import os
import sys

QUERIES = ["arith", "like_name", "like_note", "scan"]


def f(x, d=2):
    return f"{x:,.{d}f}" if isinstance(x, float) else f"{x:,}"


def pct(x):
    return f"{100 * x:.1f}%"


def main(runs):
    for n in sorted(os.listdir(runs), key=int):
        for q in QUERIES:
            p = os.path.join(runs, n, q, "analysis.json")
            if not os.path.exists(p):
                continue
            d = json.load(open(p))
            print(f"### N = {n}, `{q}`: `{d['sql']}`\n")
            print(f"Results over branches: {d['result_values']}\n")
            print("n4: steps per branch\n")
            print("| measure | min | median | mean | max | max/mean | max/median | histogram (12 equal bins, min..max) |")
            print("|---|---|---|---|---|---|---|---|")
            for k, label in (("blocks", "basic blocks"), ("llvm_instr", "LLVM IR instructions"),
                             ("x86_instr", "x86 user instructions (plain build)")):
                if k not in d["steps"]:
                    continue
                s = d["steps"][k]
                h = " ".join(str(b[2]) for b in s["hist"])
                print(f"| {label} | {f(s['min'])} | {f(s['median'], 0)} | {f(s['mean'], 0)} | {f(s['max'])} "
                      f"| {s['max/mean']:.2f} | {s['max/median']:.2f} | {h} |")
            dv = d["divergence"]
            print(f"\nControl flow: identical for the first {dv['common_prefix_blocks']} blocks "
                  f"({pct(dv['common_prefix_fraction_of_shortest'])} of the shortest run); "
                  f"{dv['distinct_block_sequences']} distinct block sequences over {d['N']} branches.\n")
            pb = d["per_branch"]
            print("n5: memory accesses\n")
            print(f"Per branch: loads median {f(pb['loads']['median'], 0)} (max {f(pb['loads']['max'])}), "
                  f"stores median {f(pb['stores']['median'], 0)} (max {f(pb['stores']['max'])}), "
                  f"memory-routine calls median {f(pb['memops']['median'], 0)}.\n")
            A, B = d["alignment"]["A"], d["alignment"]["B"]
            print("| quantity | alignment A (site, k) | alignment B (call path, site, k) |")
            print("|---|---|---|")
            rows = [
                ("aligned access positions (scalar)", "keys_scalar", f),
                ("dynamic scalar accesses, all branches", "dyn_scalar", f),
                ("dynamic accesses at positions every branch executes", "frac_dyn_in_all_branches", pct),
                ("**dynamic accesses with branch-varying address**", "frac_dyn_varying", pct),
                ("positions with branch-varying address", "frac_keys_varying", pct),
                ("varying: loads from memory the region never writes (dyn)", "dyn_varying_load_readonly", f),
                ("varying: loads from memory the region writes (dyn)", "dyn_varying_load_from_written", f),
                ("varying: stores (dyn)", "dyn_varying_store", f),
            ]
            for label, k, fmt in rows:
                print(f"| {label} | {fmt(A[k])} | {fmt(B[k])} |")
            for label, fn in (("lookup size of varying positions: median / mean / max (distinct addresses)",
                               lambda x: f"{x['lookup_size_varying']['median']:.0f} / {x['lookup_size_varying']['mean']:.1f} / {x['lookup_size_varying']['max']}"),
                              ("lookup size histogram (positions)",
                               lambda x: ", ".join(f"{k}: {v}" for k, v in x["lookup_size_hist_varying"].items())),
                              ("sum over positions of lookup size (8-byte words)", lambda x: f(x["lookup_sum_words"])),
                              ("naive sum: positions x words the region touches", lambda x: f(x["naive_sum_words"])),
                              ("lookup sum / naive sum", lambda x: f"{x['lookup_vs_naive']:.4f}"),
                              ("lookup sum / positions (mean words per access)", lambda x: f"{x['lookup_vs_keys']:.2f}"),
                              ("varying positions by region", lambda x: ", ".join(f"{k}: {v}" for k, v in x["varying_by_region"].items())),
                              ("memory-routine calls with varying address / length (positions)",
                               lambda x: f"{x['memory_routines']['keys_addr_varying']} / {x['memory_routines']['keys_len_varying']} of {x['memory_routines']['keys']}; bytes {f(x['memory_routines']['bytes_in_addr_varying'])} of {f(x['memory_routines']['bytes_total'])} at varying addresses")):
                print(f"| {label} | {fn(A)} | {fn(B)} |")
            by = d["bytes"]
            mc = d["memory_context"]
            print("\n| bytes | total | in SQLite heap | stack | other |")
            print("|---|---|---|---|---|")
            for k, label in (("touched_any", "touched by any branch"), ("touched_every", "touched by every branch"),
                             ("written_any", "written by any branch"), ("final_value_differs", "final value differs between branches")):
                x = by[k]
                other = x["total"] - x.get("sqlite_heap", 0) - x.get("[stack]:rw-p", 0)
                print(f"| {label} | {f(x['total'])} | {f(x.get('sqlite_heap', 0))} | {f(x.get('[stack]:rw-p', 0))} | {f(other)} |")
            print(f"\nContext: SQLite heap in use {f(mc['sqlite_memory_used'])} bytes; process mapped "
                  f"{f(mc['process_mapped_bytes'])} bytes (includes the 64 MiB arena); VmRSS {mc['VmRSS']}. "
                  f"Words touched by any branch: {f(A['words_touched_any'])}.\n")
            print("Top functions (source function after inlining) by dynamic varying-address accesses, alignment A: "
                  + ", ".join(f"{n} {c:,}" for n, c in A["top_functions_varying_dyn"]) + "\n")
            print("Top functions by summed lookup size, alignment A: "
                  + ", ".join(f"{n} {c:,}" for n, c in A["top_functions_varying_lookup"]) + "\n")


if __name__ == "__main__":
    main(sys.argv[1])
