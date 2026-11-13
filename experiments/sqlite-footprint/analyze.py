#!/usr/bin/env python3
"""Cross-branch analysis of one traced workload directory written by driver_trace.

    analyze.py BUILD_DIR RUN_DIR  ->  RUN_DIR/analysis.json and a text summary on stdout

Record layout (hooks.c): u32 site|flags, u32 size, u64 addr, u64 context key.
flags: bit 28 store, bit 29 libc/intrinsic memory routine (memcpy, memcmp, strlen, ...).

Two alignments of "the same access" across branches:
  A  (site, k)            k = index of this execution of the site within the whole region
  B  (call path, site, k) call path = chain of (call site, index of that call within the
                          caller frame); k counted within the frame instance
Both are exact for branches that follow the same path; where paths differ, A shifts every
later execution of a site that ran a different number of times before (over-counts
"varying"), B confines the shift to the frame where the paths differ.
"""
import json
import math
import os
import sys
from collections import Counter, defaultdict

import numpy as np

F_ST = 1 << 28
F_MEM = 2 << 28
SITE_MASK = (1 << 28) - 1
REC = np.dtype([("sf", "<u4"), ("size", "<u4"), ("addr", "<u8"), ("kb", "<u8")])
WREC = np.dtype([("line", "<u8"), ("mask", "<u8"), ("init", "u1", 64), ("fin", "u1", 64)])
PASS_RECORDS = 40_000_000  # records held per pass; a memory bound, not a time bound


def read_meta(run):
    meta, maps = {}, []
    for ln in open(os.path.join(run, "meta.txt")):
        k, _, v = ln.rstrip("\n").partition(" ")
        if k == "map":
            maps.append(v)
        else:
            meta.setdefault(k, v)
            if k == "status":
                kk, vv = v.split(":", 1)
                meta[kk.strip()] = vv.strip()
    return meta, maps


def regions(meta, maps):
    """Address ranges classifying an access: the memsys5 heap that holds all SQLite data,
    then every mapping of the process by name (stack, binary data/bss, libc, anonymous)."""
    hlo, hhi = (int(x, 16) for x in meta["heap"].split())
    out = [("sqlite_heap", hlo, hhi)]
    mapped = 0
    for m in maps:
        f = m.split()
        lo, hi = (int(x, 16) for x in f[0].split("-"))
        mapped += hi - lo
        name = os.path.basename(f[5]) if len(f) > 5 else "anon"
        out.append((f"{name}:{f[1]}", lo, hi))
    return out, mapped


def classify(addr, regs):
    cls = np.full(addr.shape, "unmapped", dtype=object)
    for name, lo, hi in reversed(regs):  # sqlite_heap (first) wins over its anon mapping
        cls[(addr >= lo) & (addr < hi)] = name
    return cls


def load_sites(build):
    sites = {}
    with open(os.path.join(build, "sites.tsv")) as f:
        next(f)
        for ln in f:
            s, kind, size, irf, srcf, line, outer = ln.rstrip("\n").split("\t")
            sites[int(s)] = (kind, srcf, int(line), outer)
    return sites


def stats(v):
    v = np.asarray(v, dtype=np.float64)
    mean, med = v.mean(), float(np.median(v))
    return {"min": int(v.min()), "median": med, "mean": mean, "max": int(v.max()),
            "max/mean": v.max() / mean, "max/median": v.max() / med,
            "p90": float(np.percentile(v, 90)), "p99": float(np.percentile(v, 99))}


def histogram(v, bins=12):
    v = np.asarray(v)
    lo, hi = int(v.min()), int(v.max())
    if lo == hi:
        return [[lo, hi, len(v)]]
    edges = np.linspace(lo, hi + 1, bins + 1)
    c, e = np.histogram(v, edges)
    return [[int(e[i]), int(e[i + 1]) - 1, int(c[i])] for i in range(bins)]


def log2_hist(v):
    c = Counter()
    for x in v:
        b = 0 if x <= 1 else math.ceil(math.log2(x))
        c[b] += 1
    return {f"<=2^{b}" if b else "1": c[b] for b in sorted(c)}


def lcp(a, b):
    n = min(len(a), len(b))
    if n == 0:
        return 0
    d = np.nonzero(a[:n] != b[:n])[0]
    return int(d[0]) if len(d) else n


def byte_set(addr, size):
    """Distinct byte addresses covered by [addr, addr+size)."""
    if len(addr) == 0:
        return np.zeros(0, np.uint64)
    pairs = np.unique(np.stack([addr, size.astype(np.uint64)], 1), axis=0)
    a, s = pairs[:, 0], pairs[:, 1].astype(np.int64)
    rep = np.repeat(a, s)
    off = np.arange(s.sum()) - np.repeat(np.cumsum(s) - s, s)
    return np.unique(rep + off.astype(np.uint64))


def in_set(addr, size, sorted_bytes):
    """True where [addr, addr+size) intersects the sorted byte set."""
    i = np.searchsorted(sorted_bytes, addr)
    ok = i < len(sorted_bytes)
    hit = np.zeros(len(addr), bool)
    hit[ok] = sorted_bytes[i[ok]] < addr[ok] + size[ok]
    return hit


def ordinal_key(sf):
    """Alignment A: (site|flags) << 32 | index of this execution of (site|flags) in the run."""
    order = np.argsort(sf, kind="stable")
    s = sf[order]
    start = np.r_[0, np.nonzero(s[1:] != s[:-1])[0] + 1]
    grp = np.repeat(start, np.diff(np.r_[start, len(s)]))
    k = np.empty(len(sf), np.uint64)
    k[order] = (np.arange(len(s)) - grp).astype(np.uint64)
    return (sf.astype(np.uint64) << np.uint64(32)) | k


def main(build, run):
    meta, maps = read_meta(run)
    N = int(meta["N"])
    regs, mapped = regions(meta, maps)
    sites = load_sites(build)
    res = {"query": meta["query"], "sql": meta["sql"], "N": N}

    # ---- per-branch counters (n4) -------------------------------------------------
    sums = []
    for r in range(N):
        d = dict(kv.split("=") for kv in open(os.path.join(run, f"sum.{r}.txt")).read().split())
        sums.append({k: int(v) for k, v in d.items()})
    perf = {}
    pp = os.path.join(run, "perf.tsv")
    if os.path.exists(pp):
        with open(pp) as f:
            next(f)
            for ln in f:
                r, rc, result, n = ln.split()
                perf[int(r)] = (int(n), int(result), int(rc))
    res["steps"] = {}
    for name in ("blocks", "llvm_instr"):
        v = [s[name] for s in sums]
        res["steps"][name] = stats(v) | {"hist": histogram(v)}
    if perf:
        v = [perf[r][0] for r in range(N)]
        res["steps"]["x86_instr"] = stats(v) | {"hist": histogram(v)}
        res["results_agree_with_plain_build"] = all(perf[r][1] == sums[r]["result"] for r in range(N))
    for name in ("loads", "stores", "memops", "load_bytes", "store_bytes"):
        res.setdefault("per_branch", {})[name] = stats([s[name] for s in sums])
    res["result_values"] = dict(Counter(str(s["result"]) for s in sums))
    res["per_branch_raw"] = [{k: s[k] for k in ("blocks", "llvm_instr", "loads", "stores", "memops", "result")}
                             | ({"x86_instr": perf[r][0]} if perf else {}) for r, s in enumerate(sums)]

    # ---- control-flow divergence --------------------------------------------------
    blk0 = np.fromfile(os.path.join(run, "blk.0.bin"), np.uint32)
    lcps, seqs = [], Counter()
    for r in range(N):
        b = np.fromfile(os.path.join(run, f"blk.{r}.bin"), np.uint32)
        lcps.append(lcp(blk0, b))
        seqs[hash(b.tobytes())] += 1
    nmin = min(s["blocks"] for s in sums)
    res["divergence"] = {"common_prefix_blocks": min(lcps), "shortest_run_blocks": nmin,
                         "common_prefix_fraction_of_shortest": min(lcps) / nmin,
                         "distinct_block_sequences": len(seqs)}

    # ---- written bytes and final values -------------------------------------------
    finals = defaultdict(dict)  # byte -> {r: final}
    init = {}
    for r in range(N):
        w = np.fromfile(os.path.join(run, f"wr.{r}.bin"), WREC)
        for rec in w:
            line, mask = int(rec["line"]), int(rec["mask"])
            for o in range(64):
                if mask >> o & 1:
                    finals[line + o][r] = int(rec["fin"][o])
                    init[line + o] = int(rec["init"][o])
    W = np.array(sorted(finals), np.uint64)
    differ = []
    for b, fr in finals.items():
        vals = set(fr.values())
        if len(fr) < N:
            vals.add(init[b])
        if len(vals) > 1:
            differ.append(b)
    differ = np.array(sorted(differ), np.uint64)

    # ---- bytes touched ------------------------------------------------------------
    touch_count = Counter()
    all_bytes = []
    total_records = 0
    for r in range(N):
        a = np.fromfile(os.path.join(run, f"acc.{r}.bin"), REC)
        total_records += len(a)
        bs = byte_set(a["addr"], a["size"])
        all_bytes.append(bs)
    cat = np.concatenate(all_bytes)
    ub, cnt = np.unique(cat, return_counts=True)
    del cat, all_bytes

    def by_region(arr):
        c = Counter(classify(arr, regs)) if len(arr) else Counter()
        return {k: int(v) for k, v in c.items()} | {"total": int(len(arr))}

    res["bytes"] = {"touched_any": by_region(ub), "touched_every": by_region(ub[cnt == N]),
                    "written_any": by_region(W), "final_value_differs": by_region(differ)}
    res["memory_context"] = {"sqlite_memory_used": int(meta["sqlite_memory_used"]),
                             "process_mapped_bytes": mapped, "VmRSS": meta.get("VmRSS")}
    words_any = int(len(np.unique(ub >> np.uint64(3))))

    # ---- aligned accesses (n5) -----------------------------------------------------
    passes = max(1, math.ceil(total_records / PASS_RECORDS))
    res["alignment"] = {}
    for alg in ("A", "B"):
        agg = defaultdict(int)
        lookup_hist = Counter()  # distinct addresses per varying scalar key
        fn_dyn, fn_lookup = Counter(), Counter()
        region_var = Counter()
        lookup_words_sum = 0
        n_keys_scalar = 0
        mem_rows = Counter()
        for p in range(passes):
            K, A, S, SF = [], [], [], []
            for r in range(N):
                a = np.fromfile(os.path.join(run, f"acc.{r}.bin"), REC)
                key = ordinal_key(a["sf"]) if alg == "A" else a["kb"]
                sel = (key % np.uint64(passes)) == p if passes > 1 else slice(None)
                K.append(key[sel]); A.append(a["addr"][sel]); S.append(a["size"][sel]); SF.append(a["sf"][sel])
            K, A, S, SF = (np.concatenate(x) for x in (K, A, S, SF))
            o = np.lexsort((S, A, K))
            K, A, S, SF = K[o], A[o], S[o], SF[o]
            kstart = np.r_[0, np.nonzero(K[1:] != K[:-1])[0] + 1]
            kend = np.r_[kstart[1:], len(K)]
            nbr = kend - kstart
            newaddr = np.r_[True, (K[1:] != K[:-1]) | (A[1:] != A[:-1])]
            ndist = np.add.reduceat(newaddr.astype(np.int64), kstart)
            newsz = np.r_[True, (K[1:] != K[:-1]) | (S[1:] != S[:-1])]
            # sizes vary only for memory routines (different lengths); count separately
            sf0 = SF[kstart]
            is_mem = (sf0 & F_MEM) != 0
            is_st = (sf0 & F_ST) != 0
            hitW = in_set(A, S.astype(np.uint64), W)
            key_hitW = np.maximum.reduceat(hitW.astype(np.int8), kstart) > 0
            var = ndist > 1
            # scalar accesses
            sc = ~is_mem
            n_keys_scalar += int(sc.sum())
            agg["keys_scalar"] += int(sc.sum())
            agg["dyn_scalar"] += int(nbr[sc].sum())
            agg["keys_scalar_in_all_branches"] += int((sc & (nbr == N)).sum())
            agg["dyn_scalar_in_all_branches"] += int(nbr[sc & (nbr == N)].sum())
            agg["keys_scalar_varying"] += int((sc & var).sum())
            agg["dyn_scalar_varying"] += int(nbr[sc & var].sum())
            for cls, m in (("load_readonly", ~is_st & ~key_hitW), ("load_from_written", ~is_st & key_hitW),
                           ("store", is_st)):
                agg[f"keys_varying_{cls}"] += int((sc & var & m).sum())
                agg[f"dyn_varying_{cls}"] += int(nbr[sc & var & m].sum())
            agg["lookup_sum_addresses"] += int(ndist[sc].sum())
            # words: distinct 8-byte words per key over all branches
            ws = A >> np.uint64(3)
            we = (A + S.astype(np.uint64) - np.uint64(1)) >> np.uint64(3)
            scal_rec = np.repeat(sc, nbr)
            kk = np.concatenate([K[scal_rec], K[scal_rec & (we != ws)]])
            ww = np.concatenate([ws[scal_rec], we[scal_rec & (we != ws)]])
            if len(kk):
                pairs = np.unique(np.stack([kk, ww], 1), axis=0)
                lookup_words_sum += len(pairs)
            for d in ndist[sc & var]:
                lookup_hist[int(d)] += 1
            vi = np.nonzero(sc & var)[0]
            for i in vi:
                site = int(sf0[i]) & SITE_MASK
                _, srcf, line, outer = sites.get(site, ("?", "?", 0, "?"))
                fn_dyn[srcf] += int(nbr[i])
                fn_lookup[srcf] += int(ndist[i])
            region_var.update(classify(A[kstart[vi]], regs))
            # memory routines (one record per call, size = bytes moved/compared)
            mm = is_mem
            nsz = np.add.reduceat(newsz.astype(np.int64), kstart)
            mem_rows["keys"] += int(mm.sum())
            mem_rows["dyn"] += int(nbr[mm].sum())
            mem_rows["keys_addr_varying"] += int((mm & var).sum())
            mem_rows["dyn_addr_varying"] += int(nbr[mm & var].sum())
            mem_rows["keys_len_varying"] += int((mm & (nsz > 1)).sum())
            mem_rows["bytes_total"] += int(S[np.repeat(mm, nbr)].astype(np.int64).sum())
            mem_rows["bytes_in_addr_varying"] += int(S[np.repeat(mm & var, nbr)].astype(np.int64).sum())
            del K, A, S, SF
        d = dict(agg)
        d["frac_keys_varying"] = d["keys_scalar_varying"] / d["keys_scalar"]
        d["frac_dyn_varying"] = d["dyn_scalar_varying"] / d["dyn_scalar"]
        d["frac_dyn_in_all_branches"] = d["dyn_scalar_in_all_branches"] / d["dyn_scalar"]
        d["lookup_size_hist_varying"] = log2_hist(sorted(lookup_hist.elements()))
        vs = np.array(sorted(lookup_hist.elements())) if lookup_hist else np.array([0])
        d["lookup_size_varying"] = {"median": float(np.median(vs)), "mean": float(vs.mean()),
                                    "max": int(vs.max())}
        d["lookup_sum_words"] = lookup_words_sum
        d["words_touched_any"] = words_any
        d["naive_sum_words"] = n_keys_scalar * words_any
        d["lookup_vs_naive"] = lookup_words_sum / d["naive_sum_words"]
        d["lookup_vs_keys"] = lookup_words_sum / n_keys_scalar
        d["varying_by_region"] = dict(region_var)
        d["top_functions_varying_dyn"] = fn_dyn.most_common(10)
        d["top_functions_varying_lookup"] = fn_lookup.most_common(10)
        d["memory_routines"] = dict(mem_rows)
        res["alignment"][alg] = d
    json.dump(res, open(os.path.join(run, "analysis.json"), "w"), indent=1, default=str)
    print(json.dumps({k: v for k, v in res.items() if k != "per_branch_raw"}, indent=1, default=str))


if __name__ == "__main__":
    main(*sys.argv[1:3])
