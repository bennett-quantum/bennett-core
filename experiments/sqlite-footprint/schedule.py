#!/usr/bin/env python3
"""Control schedules for coherent execution of the traced SQLite regions.

    schedule.py prep   BUILD_DIR CACHE                 parse the traced IR once (CFG, calls, loops)
    schedule.py trace  BUILD_DIR CACHE RUN_DIR OUT_DIR rebuild call and loop structure per branch
    schedule.py report BUILD_DIR CACHE RUN_DIR OUT_DIR [SPLITS] -> OUT_DIR/schedule.json, table on stdout

A schedule gives, for every step t of the coherent circuit, the code S_t that step provides. Cost =
sum over steps of the IR instructions in S_t. All families are computed from the same block traces
(blk.<r>.bin) and the IR CFG of the traced build (sqlite3.tr.ll; block ids are the __tr_bb ids).

Structure recovered from a block trace (checked: every intra-frame transition is a CFG edge, every
frame ends on a block whose terminator is `ret`, and no structural key repeats within a branch):
  * a call is the entry block of a function; its call site is the first not-yet-executed call slot of
    the caller's current block that can reach it (direct call to that function, or an indirect call);
  * a return is a frame whose current block ends in `ret`;
  * loops are the SCC loop forest of each function (Havlak/Steensgaard style: a loop is a non-trivial
    SCC, its headers are the SCC nodes entered from outside it, edges from inside to a header are back
    edges; remove them and recurse). For reducible CFGs these are the natural loops; irreducible
    cycles get more than one header and are reported.
Keys of one block execution:
  exact      (call sites, loop iteration indices, block) from the region root
  normalised the same with every iteration index replaced by '*' (one body per loop instance)
"""
import json
import os
import pickle
import re
import sys
from collections import defaultdict, Counter
from multiprocessing import Pool

import numpy as np

# ------------------------------------------------------------------------------------------- IR

LABEL_RE = re.compile(r"^([\w.$-]+):")
BB_RE = re.compile(r"call void @__tr_bb\(i32 (\d+), i32 (\d+)\)")
CALL_RE = re.compile(r"\bcall\b.*?(@[\w.$\"]+|%[\w.$\"]+)\(")
LBL_RE = re.compile(r"label %([\w.$-]+)")
LD_RE = re.compile(r"call void @__tr_ld\(ptr [^,]+, i32 (\d+), i32 \d+\)")


def parse_ir(path):
    """Per block: function, successors, terminator kind, call slots. Call slot = callee name (defined
    or external) or None for an indirect call. Instrumentation, intrinsics and inline asm are skipped."""
    funcs, defined = [], set()
    blocks = {}  # bid -> dict
    cur_f = None
    cur_b = None
    label_to_bid = {}
    pending = []  # (bid, [labels]) resolved at function end
    term_lines = []
    in_switch = False
    lines = open(path).read().split("\n")
    for ln in lines:
        if ln.startswith("define "):
            cur_f = re.search(r"@([\w.$\"]+)\(", ln).group(1)
            defined.add(cur_f)
            funcs.append(cur_f)
            label_to_bid = {}
            pending = []
            cur_label = "entry"
            continue
        if cur_f is None:
            continue
        if ln == "}":
            for bid, labs in pending:
                blocks[bid]["succ"] = sorted({label_to_bid[l] for l in labs})
            cur_f = None
            continue
        m = LABEL_RE.match(ln)
        if m:
            cur_label = m.group(1)
            continue
        m = BB_RE.search(ln)
        if m:
            bid = int(m.group(1))
            cur_b = {"func": cur_f, "label": cur_label, "n": int(m.group(2)), "calls": [], "term": None,
                     "succ": [], "lds": []}
            blocks[bid] = cur_b
            label_to_bid[cur_label] = bid
            continue
        s = ln.strip()
        ml = LD_RE.search(s)
        if ml:
            cur_b["lds"].append(int(ml.group(1)))
            continue
        if in_switch:
            pending[-1][1].extend(LBL_RE.findall(ln))
            if s.startswith("]"):
                in_switch = False
            continue
        if not ln.startswith("  "):
            continue
        if s.startswith(("br ", "switch ", "indirectbr ", "ret", "unreachable", "resume", "callbr ")):
            kind = s.split()[0]
            cur_b["term"] = kind
            labs = LBL_RE.findall(ln)
            pending.append((bid, labs))
            if kind == "switch" and not s.endswith("]") and "]" not in s:
                in_switch = True
            continue
        if " call " in " " + s or s.startswith("call") or "= call" in s or "tail call" in s:
            m = CALL_RE.search(s)
            if not m:
                continue
            name = m.group(1)
            if name.startswith("@"):
                nm = name[1:].strip('"')
                if nm.startswith(("__tr_", "__cyg_profile", "llvm.", "__sanitizer")):
                    continue
                cur_b["calls"].append(nm)
            else:
                if "asm" in s.split(name)[0]:
                    continue
                cur_b["calls"].append(None)
    return funcs, defined, blocks


def loop_forest(nodes, succ, entry):
    """SCC loop forest. Returns list of loops (headers set, body set, parent index) and for every node
    the chain of loop indices containing it, outermost first."""
    loops = []

    def sccs(sub, edges):
        # iterative Tarjan on the subgraph
        index, low, onst, st, out = {}, {}, set(), [], []
        idx = 0
        for root in sub:
            if root in index:
                continue
            work = [(root, iter(edges.get(root, ())))]
            index[root] = low[root] = idx; idx += 1
            st.append(root); onst.add(root)
            while work:
                v, it = work[-1]
                adv = False
                for w in it:
                    if w not in sub:
                        continue
                    if w not in index:
                        index[w] = low[w] = idx; idx += 1
                        st.append(w); onst.add(w)
                        work.append((w, iter(edges.get(w, ()))))
                        adv = True
                        break
                    elif w in onst:
                        low[v] = min(low[v], index[w])
                if adv:
                    continue
                work.pop()
                if work:
                    low[work[-1][0]] = min(low[work[-1][0]], low[v])
                if low[v] == index[v]:
                    comp = set()
                    while True:
                        w = st.pop(); onst.discard(w); comp.add(w)
                        if w == v:
                            break
                    out.append(comp)
        return out

    def rec(sub, edges, parent):
        for comp in sccs(sub, edges):
            if len(comp) == 1:
                v = next(iter(comp))
                if v not in edges.get(v, ()):
                    continue
            preds_out = set()
            for u in sub:
                for w in edges.get(u, ()):
                    if w in comp and u not in comp:
                        preds_out.add(w)
            if entry in comp:
                preds_out.add(entry)
            headers = preds_out if preds_out else {min(comp)}
            li = len(loops)
            loops.append({"headers": headers, "body": comp, "parent": parent})
            inner = {u: [w for w in edges.get(u, ()) if w in comp and w not in headers] for u in comp}
            rec(comp, inner, li)

    rec(set(nodes), {u: succ[u] for u in nodes}, -1)
    chain = {v: [] for v in nodes}
    order = sorted(range(len(loops)), key=lambda i: depth(loops, i))
    for i in order:
        for v in loops[i]["body"]:
            chain[v].append(i)
    return loops, chain


def depth(loops, i):
    d = 0
    while loops[i]["parent"] >= 0:
        i = loops[i]["parent"]; d += 1
    return d


def prep(build, cache):
    funcs, defined, blocks = parse_ir(os.path.join(build, "sqlite3.tr.ll"))
    nb = max(blocks) + 1
    tsv = {}
    with open(os.path.join(build, "blocks.tsv")) as f:
        next(f)
        for ln in f:
            b, fn, lab, n = ln.rstrip("\n").split("\t")
            tsv[int(b)] = (fn, lab, int(n))
    assert len(tsv) == nb == len(blocks), (len(tsv), nb, len(blocks))
    fid = {f: i for i, f in enumerate(funcs)}
    func = np.zeros(nb, np.int32)
    w = np.zeros(nb, np.int64)
    is_entry = np.zeros(nb, bool)
    is_ret = np.zeros(nb, bool)
    succ, calls = [None] * nb, [None] * nb
    fblocks = defaultdict(list)
    for b, d in blocks.items():
        assert tsv[b][0] == d["func"] and tsv[b][2] == d["n"], (b, tsv[b], d["func"], d["n"])
        func[b] = fid[d["func"]]
        w[b] = d["n"]
        is_entry[b] = d["label"] == "entry"
        is_ret[b] = d["term"] == "ret"
        succ[b] = tuple(d["succ"])
        # call slot: >=0 defined callee function id, -1 indirect, -2 external (never produces events)
        calls[b] = tuple(fid[c] if c in defined else (-1 if c is None else -2) for c in d["calls"])
        fblocks[d["func"]].append(b)
    # loops per function; global loop ids
    loop_func, loop_headers, loop_body, loop_parent = [], [], [], []
    chain = [()] * nb
    irreducible = []
    for f, bs in fblocks.items():
        ent = min(bs)
        assert is_entry[ent]
        loops, ch = loop_forest(bs, {b: succ[b] for b in bs}, ent)
        base = len(loop_func)
        for li, L in enumerate(loops):
            loop_func.append(fid[f])
            loop_headers.append(frozenset(L["headers"]))
            loop_body.append(frozenset(L["body"]))
            loop_parent.append(base + L["parent"] if L["parent"] >= 0 else -1)
            if len(L["headers"]) > 1:
                irreducible.append(base + li)
        for b in bs:
            chain[b] = tuple(base + i for i in ch[b])
    hdr_of = {}  # header block -> loop id (a header heads exactly one loop)
    for li, H in enumerate(loop_headers):
        for h in H:
            assert h not in hdr_of
            hdr_of[h] = li
    lds = [tuple(blocks[b]["lds"]) for b in range(nb)]
    data = dict(lds=lds, funcs=funcs, func=func, w=w, is_entry=is_entry, is_ret=is_ret, succ=succ, calls=calls,
                loop_func=loop_func, loop_headers=loop_headers, loop_body=loop_body, loop_parent=loop_parent,
                chain=chain, hdr_of=hdr_of, irreducible=irreducible, labels=[tsv[b][1] for b in range(nb)])
    pickle.dump(data, open(cache, "wb"))
    print(f"blocks={nb} funcs={len(funcs)} loops={len(loop_func)} irreducible_loops={len(irreducible)}",
          file=sys.stderr)


# ------------------------------------------------------------------------------ per-branch trace

G = None  # IR data in workers
ACC = np.dtype([("sf", "<u4"), ("size", "<u4"), ("addr", "<u8"), ("kb", "<u8")])
SITE_MASK = (1 << 28) - 1
REGION_LOOPS = None


def _init(cache, region_loops):
    global G, REGION_LOOPS
    G = pickle.load(open(cache, "rb"))
    REGION_LOOPS = frozenset(region_loops)


class Fail(Exception):
    pass


def dispatch_loop(g):
    """The bytecode dispatch loop of sqlite3VdbeExec (its largest top-level loop), its single header
    (the `switch(pOp->opcode)` block) and the load site of `pOp->opcode` in that header."""
    v = g["funcs"].index("sqlite3VdbeExec")
    vl = [li for li, f in enumerate(g["loop_func"]) if f == v and g["loop_parent"][li] == -1]
    L = max(vl, key=lambda li: len(g["loop_body"][li]))
    (h,) = g["loop_headers"][L]
    (site,) = g["lds"][h]
    return L, h, site


def trace_branch(args):
    """Rebuild call/loop structure for one branch. Writes OUT/k.<r>.npz with per-event arrays."""
    run, out, r = args
    func, is_entry, is_ret = G["func"], G["is_entry"], G["is_ret"]
    succ, calls, chain, hdr_of = G["succ"], G["calls"], G["chain"], G["hdr_of"]
    blk = np.fromfile(os.path.join(run, f"blk.{r}.bin"), np.uint32).tolist()
    n = len(blk)
    DL, DLH, DSITE = dispatch_loop(G)
    acc = np.fromfile(os.path.join(run, f"acc.{r}.bin"), ACC)
    pcs = acc["addr"][(acc["sf"] & SITE_MASK) == DSITE].tolist()  # pOp of every dispatch, in order
    del acc
    pci = 0
    keyS = np.zeros(n, np.int64)       # bytecode-specialised key (dispatch iterations keyed by pc)
    parS = np.zeros(n, np.int64)
    nodesS = {}
    trips_S = {}
    keyX = np.zeros(n, np.int64)       # exact key
    keyN = np.zeros(n, np.int64)       # normalised key (iteration indices -> *)
    parN = np.zeros(n, np.int64)       # normalised parent body node of the event
    foff = np.zeros(n, np.int32)       # step offset within the frame
    fkey = np.zeros(n, np.int64)       # frame id (index of the frame's entry event)
    reg = np.full(n, -1, np.int64)     # region instance key (normalised) or -1
    roff = np.zeros(n, np.int32)       # step offset within the region instance
    nodes = {}                         # normalised node -> (parent, kind, label, func)
    xnodes = {}                        # exact loop node -> trip count
    trips_n = {}                       # normalised loop node -> max trip in this branch
    trips_s = {}                       # static loop id -> max trip in this branch
    frames = []                        # per frame: (entry step, parent frame id, callsite label, path key)
    # frame: [func, cur, slot, loopstack, keyX, keyN, t0, fid, region, keyS]
    # loopstack entry: [loop, iter, xloop, xbody, nloop, sloop, sbody, backjumps, prevpc]
    root_ord = Counter()
    stack = []
    issues = Counter()

    def close_loops(fr, keep):
        ls = fr[3]
        while len(ls) > keep:
            L, it, xl, xb, nl, sl_, sb, _, _ = ls.pop()
            t = it + 1
            if L != DL:
                trips_S[sl_] = max(trips_S.get(sl_, 0), t)
            xnodes[xl] = max(xnodes.get(xl, 0), t)
            trips_n[nl] = max(trips_n.get(nl, 0), t)
            trips_s[L] = max(trips_s.get(L, 0), t)
            if fr[8] is not None and fr[8][2] == len(ls):
                fr[8] = None

    def body(fr):
        ls = fr[3]
        if ls:
            return ls[-1][3], ls[-1][4], ls[-1][6]
        return fr[4], fr[5], fr[9]

    def enter_loops(fr, loops_to_enter, t):
        for L in loops_to_enter:
            bx, bn, bs = body(fr)
            xl = hash((bx, 2, L))
            nl = hash((bn, 2, L))
            if nl not in nodes:
                nodes[nl] = (bn, 2, L, -1)
            xb = hash((xl, 0))
            sl_ = hash((bs, 2, L))
            if sl_ not in nodesS:
                nodesS[sl_] = (bs, 2, L, -1)
            ent = [L, 0, xl, xb, nl, sl_, sl_, 0, None]
            if L == DL:
                dispatch(ent)
            fr[3].append(ent)
            if L in REGION_LOOPS and fr[8] is None and not in_region():
                fr[8] = (nl, t, len(fr[3]) - 1)

    def dispatch(ent):
        """A new iteration of the dispatch loop: key its body by (backward jumps so far, pc)."""
        nonlocal pci
        pc = pcs[pci]
        pci += 1
        if ent[8] is not None and pc <= ent[8]:
            ent[7] += 1
        ent[8] = pc
        ent[6] = hash((ent[5], 4, ent[7], pc))
        if ent[6] not in nodesS:
            nodesS[ent[6]] = (ent[5], 4, hash((ent[7], pc)), -1)

    def in_region():
        for fr in stack:
            if fr[8] is not None:
                return fr[8]
        return None

    def pop_frame():
        fr = stack.pop()
        close_loops(fr, 0)
        if any(c >= 0 for c in calls[fr[1]][fr[2]:]):
            issues["frame_returned_with_pending_direct_call"] += 1

    for t, v in enumerate(blk):
        fv = int(func[v])
        if is_entry[v]:
            while True:
                if not stack:
                    root_ord[fv] += 1
                    cs = (-1, fv, root_ord[fv])
                    px, pn, ps = 0, 0, 0
                    break
                fr = stack[-1]
                sl = calls[fr[1]]
                j = fr[2]
                hit = -1
                while j < len(sl):
                    c = sl[j]
                    if c == fv or c == -1:
                        hit = j
                        break
                    if c >= 0:
                        break
                    j += 1
                if hit >= 0:
                    fr[2] = hit + 1
                    cs = (fr[1], hit, fv)
                    px, pn, ps = body(fr)
                    break
                if is_ret[fr[1]]:
                    pop_frame()
                    continue
                raise Fail(f"r={r} t={t}: call to {G['funcs'][fv]} from no slot of block {fr[1]}")
            kx = hash((px, 1, cs))
            kn = hash((pn, 1, cs))
            ks = hash((ps, 1, cs))
            if ks not in nodesS:
                nodesS[ks] = (ps, 1, cs, fv)
            if kn not in nodes:
                nodes[kn] = (pn, 1, cs, fv)
            frames.append((t, stack[-1][7] if stack else -1, cs))
            stack.append([fv, v, 0, [], kx, kn, t, t, None, ks])
            fr = stack[-1]
            enter_loops(fr, chain[v], t)
        else:
            while stack and is_ret[stack[-1][1]]:
                pop_frame()
            if not stack:
                raise Fail(f"r={r} t={t}: block {v} after all frames returned")
            fr = stack[-1]
            u = fr[1]
            if fr[0] != fv or v not in succ[u]:
                raise Fail(f"r={r} t={t}: {u}->{v} is not a CFG edge")
            if any(c >= 0 for c in calls[u][fr[2]:]):
                issues["block_left_with_pending_direct_call"] += 1
            cu, cv = chain[u], chain[v]
            k = 0
            while k < len(cu) and k < len(cv) and cu[k] == cv[k]:
                k += 1
            L = hdr_of.get(v, -1)
            if L >= 0 and L in cu:          # back edge of loop L: next iteration
                j = cu.index(L)
                close_loops(fr, j + 1)
                ent = fr[3][j]
                ent[1] += 1
                ent[3] = hash((ent[2], ent[1]))
                if L == DL:
                    dispatch(ent)
                enter_loops(fr, cv[j + 1:], t)
            else:
                close_loops(fr, k)
                enter_loops(fr, cv[k:], t)
            if u == DLH:  # record the opcode handler entered by this dispatch
                for ent in fr[3]:
                    if ent[0] == DL:
                        p_, kd, lb, f0 = nodesS[ent[6]]
                        if f0 != -1 and f0 != v:
                            raise Fail(f"r={r} t={t}: one (backjumps, pc) dispatches to two handlers")
                        nodesS[ent[6]] = (p_, kd, lb, v)
            fr[1] = v
            fr[2] = 0
        bx, bn, bs = body(fr)
        keyS[t] = hash((bs, 3, v))
        parS[t] = bs
        keyX[t] = hash((bx, 3, v))
        keyN[t] = hash((bn, 3, v))
        parN[t] = bn
        foff[t] = t - fr[6]
        fkey[t] = fr[7]
        rg = in_region()
        if rg is not None:
            reg[t] = rg[0]
            roff[t] = t - rg[1]
    while stack:
        if not is_ret[stack[-1][1]]:
            raise Fail(f"r={r}: trace ends inside {G['funcs'][stack[-1][0]]} at a non-ret block")
        pop_frame()
    if pci != len(pcs):
        raise Fail(f"r={r}: {len(pcs)} opcode loads but {pci} dispatches")
    if len(np.unique(keyX)) != n:
        raise Fail(f"r={r}: exact keys repeat within the branch")
    fr_arr = np.array([(a, b, c[0], c[1], c[2]) for a, b, c in frames], np.int64).reshape(-1, 5)
    np.savez(os.path.join(out, f"k.{r}.npz"), keyX=keyX, keyN=keyN, parN=parN, foff=foff, fkey=fkey,
             reg=reg, roff=roff, frames=fr_arr,
             nodes=np.array([(k, p, kd, hash(lb) if kd == 1 else lb, f) for k, (p, kd, lb, f) in nodes.items()],
                            np.int64).reshape(-1, 5),
             xtrip=np.array(list(xnodes.items()), np.int64).reshape(-1, 2),
             ntrip=np.array(list(trips_n.items()), np.int64).reshape(-1, 2),
             strip=np.array(list(trips_s.items()), np.int64).reshape(-1, 2),
             keyS=keyS, parS=parS,
             nodesS=np.array([(k, p, kd, hash(lb) if kd == 1 else lb, f) for k, (p, kd, lb, f) in nodesS.items()],
                             np.int64).reshape(-1, 5),
             trip_S=np.array(list(trips_S.items()), np.int64).reshape(-1, 2))
    return r, dict(issues), len(frames)


def trace(build, cache, run, out, region_loops):
    os.makedirs(out, exist_ok=True)
    N = len([f for f in os.listdir(run) if f.startswith("blk.")])
    with Pool(int(os.environ.get("JOBS", "8")), _init, (cache, region_loops)) as p:
        tot = Counter()
        for r, iss, nf in p.imap_unordered(trace_branch, [(run, out, r) for r in range(N)], chunksize=4):
            tot.update(iss)
    json.dump({"N": N, "issues": dict(tot), "region_loops": sorted(region_loops)},
              open(os.path.join(out, "trace.json"), "w"))
    print(json.dumps({"N": N, "issues": dict(tot)}), file=sys.stderr)


def region_loops(g):
    """Loops run as a step machine in the hybrid family: the bytecode dispatch loop of sqlite3VdbeExec
    (its largest loop: `for(pOp=...; ; pOp++) switch(pOp->opcode)`) and every irreducible loop."""
    v = g["funcs"].index("sqlite3VdbeExec")
    vl = [li for li, f in enumerate(g["loop_func"]) if f == v and g["loop_parent"][li] == -1]
    main = max(vl, key=lambda li: len(g["loop_body"][li]))
    return sorted({main} | set(g["irreducible"]))


# ---------------------------------------------------------------------------------------- report

def load_branch(args):
    run, kdir, r = args
    w = G["w"]
    b = np.fromfile(os.path.join(run, f"blk.{r}.bin"), np.uint32).astype(np.int32)
    z = np.load(os.path.join(kdir, f"k.{r}.npz"))
    path_of, cs_off, paths = {}, {}, set()
    for t, pf, c0, c1, c2 in z["frames"].tolist():
        cs = (c0, c1, c2)
        if pf < 0:
            p, off = (cs,), t
        else:
            p, off = path_of[pf] + (cs,), t - pf
        path_of[t] = p
        paths.add(p)
        lo, hi = cs_off.get(cs, (off, off))
        cs_off[cs] = (min(lo, off), max(hi, off))
    o = np.argsort(b, kind="stable")
    bs = b[o]
    st = np.r_[0, np.nonzero(bs[1:] != bs[:-1])[0] + 1]
    foff = z["foff"][o]
    t = np.arange(len(b), dtype=np.int32)[o]
    blk_rel = np.stack([bs[st], np.minimum.reduceat(foff, st), np.maximum.reduceat(foff, st)], 1)
    blk_abs = np.stack([bs[st], np.minimum.reduceat(t, st), np.maximum.reduceat(t, st)], 1)
    return dict(r=r, b=b, kx=z["keyX"], kn=z["keyN"], pn=z["parN"], rg=z["reg"], ro=z["roff"],
                nodes=z["nodes"], ntrip=z["ntrip"], paths=paths, cs_off=cs_off, blk_rel=blk_rel,
                blk_abs=blk_abs, instr=int(w[b].sum()))


def interval_union_size(iv):
    """iv: list of (lo, hi) inclusive -> merged list."""
    iv = sorted(iv)
    out = []
    for lo, hi in iv:
        if out and lo <= out[-1][1] + 1:
            out[-1][1] = max(out[-1][1], hi)
        else:
            out.append([lo, hi])
    return out


def in_intervals(code, starts, ends):
    i = np.searchsorted(starts, code, side="right") - 1
    ok = i >= 0
    ok[ok] = code[ok] <= ends[i[ok]]
    return ok


def supersequence(seqs):
    """Greedy majority merge: emit the block that is next for most unfinished sequences
    (ties: smallest block id); advance those sequences."""
    lens = np.array([len(s) for s in seqs])
    start = np.r_[0, np.cumsum(lens)[:-1]]
    allb = np.concatenate(seqs)
    pos = np.zeros(len(seqs), np.int64)
    out = []
    live = np.nonzero(pos < lens)[0]
    while len(live):
        nxt = allb[start[live] + pos[live]]
        u, c = np.unique(nxt, return_counts=True)
        x = u[np.argmax(c)]
        out.append(x)
        pos[live[nxt == x]] += 1
        live = live[pos[live] < lens[live]]
    return np.array(out, np.int32)


def is_subsequence(seq, sup, occ):
    """occ: block -> sorted positions in sup."""
    p = -1
    for x in seq.tolist():
        o = occ.get(x)
        if o is None:
            return False
        i = np.searchsorted(o, p, side="right")
        if i == len(o):
            return False
        p = o[i]
    return True


class Workload:
    def __init__(self, run, kdir, jobs):
        N = len([f for f in os.listdir(run) if f.startswith("blk.")])
        self.N = N
        with Pool(jobs, _init, (CACHE, ())) as p:
            res = sorted(p.map(load_branch, [(run, kdir, r) for r in range(N)], chunksize=4),
                         key=lambda d: d["r"])
        g = G
        self.w = g["w"]
        self.func = g["func"]
        self.len = np.array([len(d["b"]) for d in res])
        self.instr = np.array([d["instr"] for d in res])
        self.start = np.r_[0, np.cumsum(self.len)[:-1]]
        self.seqs = [d["b"] for d in res]
        E = int(self.len.sum())
        self.br = np.repeat(np.arange(N, dtype=np.int32), self.len)
        self.b = np.concatenate(self.seqs)
        self.t = (np.arange(E, dtype=np.int64) - self.start[self.br]).astype(np.int32)

        def ids(name):
            a = np.concatenate([d[name] for d in res])
            u, inv = np.unique(a, return_inverse=True)
            return u, inv.astype(np.int32)
        _, self.kx = ids("kx")
        _, self.kn = ids("kn")
        nodes = {}
        for d in res:
            for k, p_, kd, lb, f in d["nodes"].tolist():
                nodes[k] = (p_, kd, lb, f)
        self.nodes = nodes
        self.pn_keys, self.pn = ids("pn")
        rg = np.concatenate([d["rg"] for d in res])
        inr = rg != -1  # Python's hash() never returns -1
        self.rg_keys = np.unique(rg[inr])
        self.rg = np.full(len(rg), -1, np.int32)
        self.rg[inr] = np.searchsorted(self.rg_keys, rg[inr]).astype(np.int32)
        self.ro = np.concatenate([d["ro"] for d in res]).astype(np.int32)
        # per kn id: its parent node id and block (first occurrence)
        u, first = np.unique(self.kn, return_index=True)
        self.kn_pn = self.pn[first]
        self.kn_b = self.b[first]
        self.kn_inreg = self.rg[first] >= 0
        assert np.array_equal(u, np.arange(len(u)))
        # loop trips per normalised loop node
        nt = [d["ntrip"] for d in res]
        self.nt_br = np.repeat(np.arange(N), [len(x) for x in nt])
        nt = np.concatenate(nt)
        self.nt_key, self.nt_trip = nt[:, 0], nt[:, 1]
        self.paths = [d["paths"] for d in res]
        self.cs_off = [d["cs_off"] for d in res]
        self.blk_rel = [d["blk_rel"] for d in res]
        self.blk_abs = [d["blk_abs"] for d in res]
        inreg_nodes = set(self.pn_keys[np.unique(self.pn[self.rg >= 0])].tolist())
        self.inreg_nodes = inreg_nodes

    # ------------------------------------------------------------------ helpers
    def ev_mask(self, brmask):
        return brmask[self.br]

    def present(self, ids, evm, n):
        m = np.zeros(n, bool)
        m[ids[evm]] = True
        return m

    def mult_fn(self, T):
        """Multiplier of a node = product of trips of the loop nodes from the node up to the root."""
        nodes, memo = self.nodes, {0: 1}

        def mult(k):
            stack = []
            while k not in memo:
                stack.append(k)
                k = nodes[k][0]
            m = memo[k]
            for x in reversed(stack):
                if nodes[x][1] == 2:
                    m = m * T.get(x, 0)
                memo[x] = m
            return memo[stack[0]] if stack else m
        return mult

    def trips(self, brmask, static):
        sel = brmask[self.nt_br]
        keys, trips = self.nt_key[sel], self.nt_trip[sel]
        T = {}
        for k, t in zip(keys.tolist(), trips.tolist()):
            if t > T.get(k, 0):
                T[k] = t
        if not static:
            return T
        S = {}
        for k, t in T.items():
            L = self.nodes[k][2]
            S[L] = max(S.get(L, 0), t)
        return {k: S.get(self.nodes[k][2], 0) for k in self.nodes if self.nodes[k][1] == 2}, S

    # ------------------------------------------------------------------ families
    def evaluate(self, train, test, attribute=False):
        """Build every schedule from the branches in `train`; cost it; count the `test` branches it
        fails to cover. Returns {family: {cost, steps, fail, [by_func]}}."""
        w, func, g = self.w, self.func, G
        out = {}
        evtr, evte = self.ev_mask(train), self.ev_mask(test)
        te_ids = np.nonzero(test)[0]

        def fails(bad_events=None, bad_branches=None):
            m = np.zeros(self.N, bool)
            if bad_events is not None:
                m[self.br[evte][bad_events]] = True
            if bad_branches is not None:
                m |= bad_branches
            return int((m & test).sum())

        def byfunc(blocks, weights):
            if not attribute:
                return None
            c = np.bincount(func[blocks], weights=weights.astype(np.float64), minlength=len(g["funcs"]))
            top = np.argsort(-c)[:8]
            tot = c.sum()
            return [(g["funcs"][i], int(c[i]), round(c[i] / tot, 3)) for i in top if c[i] > 0]

        b_tr, b_te = self.b[evtr], self.b[evte]
        t_tr, t_te = self.t[evtr], self.t[evte]
        Ttr = int(self.len[train].max())
        # 0a greedy common supersequence
        sup = supersequence([self.seqs[r] for r in np.nonzero(train)[0]])
        occ = {}
        for i, x in enumerate(sup.tolist()):
            occ.setdefault(x, []).append(i)
        occ = {k: np.array(v) for k, v in occ.items()}
        bad = np.zeros(self.N, bool)
        for r in te_ids:
            bad[r] = not is_subsequence(self.seqs[r], sup, occ)
        out["0a_supersequence"] = dict(cost=int(w[sup].sum()), steps=len(sup), fail=fails(bad_branches=bad))
        # 0b step-aligned exact
        code_tr = (t_tr.astype(np.int64) << 16) | b_tr
        pres = np.unique(code_tr)
        code_te = (t_te.astype(np.int64) << 16) | b_te
        out["0b_step_exact"] = dict(cost=int(w[pres & 0xFFFF].sum()), steps=Ttr,
                                    fail=fails(bad_events=~np.isin(code_te, pres)))
        del code_tr, code_te
        # 0c / 0d every reached block (instruction) at every step
        reached = np.zeros(len(w), bool)
        reached[b_tr] = True
        rw = int(w[reached].sum())
        Itr = int(self.instr[train].max())
        badb = ~reached[b_te]
        out["0c_all_reached_blocks"] = dict(cost=Ttr * rw, steps=Ttr,
                                            fail=fails(bad_events=badb, bad_branches=self.len > Ttr),
                                            reached_blocks=int(reached.sum()), reached_instr=rw)
        out["0d_all_reached_instr"] = dict(cost=Itr * rw, steps=Itr,
                                           fail=fails(bad_events=badb, bad_branches=self.instr > Itr))
        # 2a global step interval per block
        ba = np.concatenate([self.blk_abs[r] for r in np.nonzero(train)[0]])
        lo = np.full(len(w), np.iinfo(np.int64).max)
        hi = np.full(len(w), -1)
        np.minimum.at(lo, ba[:, 0], ba[:, 1])
        np.maximum.at(hi, ba[:, 0], ba[:, 2])
        span = np.where(reached, hi - lo + 1, 0)
        ok = reached[b_te] & (t_te >= lo[b_te]) & (t_te <= hi[b_te])
        out["2a_step_interval"] = dict(cost=int((w * span).sum()), steps=int(hi.max() + 1),
                                       fail=fails(bad_events=~ok))
        # 2b function-relative intervals composed along call paths
        br_rel = np.concatenate([self.blk_rel[r] for r in np.nonzero(train)[0]])
        rlo = np.full(len(w), np.iinfo(np.int64).max)
        rhi = np.full(len(w), -1)
        np.minimum.at(rlo, br_rel[:, 0], br_rel[:, 1])
        np.maximum.at(rhi, br_rel[:, 0], br_rel[:, 2])
        C = {}
        paths = set()
        for r in np.nonzero(train)[0]:
            paths |= self.paths[r]
            for cs, (a, z) in self.cs_off[r].items():
                l0, h0 = C.get(cs, (a, z))
                C[cs] = (min(l0, a), max(h0, z))
        fiv = defaultdict(list)
        for p in paths:
            fiv[p[-1][1] if p[-1][0] == -1 else p[-1][2]].append(
                (sum(C[c][0] for c in p), sum(C[c][1] for c in p)))
        rb = np.nonzero(reached)[0]
        starts, ends, cost2b, steps2b = [], [], 0, 0
        for bb in rb.tolist():
            iv = interval_union_size([(a + rlo[bb], z + rhi[bb]) for a, z in fiv[int(func[bb])]])
            for a, z in iv:
                starts.append((bb << 32) + a)
                ends.append((bb << 32) + z)
                cost2b += int(w[bb]) * (z - a + 1)
                steps2b = max(steps2b, z + 1)
        starts, ends = np.array(starts, np.int64), np.array(ends, np.int64)
        o = np.argsort(starts)
        starts, ends = starts[o], ends[o]
        ok = in_intervals((b_te.astype(np.int64) << 32) + t_te, starts, ends)
        out["2b_func_interval"] = dict(cost=cost2b, steps=steps2b, fail=fails(bad_events=~ok),
                                       call_paths=len(paths))
        # 3a exact structural keys (iteration-specific unrolling)
        kx_tr = np.unique(self.kx[evtr])
        nkx = int(self.kx.max()) + 1
        kxb = np.zeros(nkx, np.int32)
        kxb[self.kx] = self.b
        pres = np.zeros(nkx, bool)
        pres[kx_tr] = True
        out["3a_struct_exact"] = dict(cost=int(w[kxb[kx_tr]].sum()), steps=len(kx_tr),
                                      fail=fails(bad_events=~pres[self.kx[evte]]),
                                      by_func=byfunc(kxb[kx_tr], w[kxb[kx_tr]]))
        # 3b/3c/3d normalised structure
        nkn = len(self.kn_pn)
        kn_pres = np.zeros(nkn, bool)
        kn_pres[self.kn[evtr]] = True
        kn_ids = np.nonzero(kn_pres)[0]
        Tb = self.trips(train, static=False)
        Tc, Sc = self.trips(train, static=True)
        nt_te = test[self.nt_br]
        nt_keys, nt_trips = self.nt_key[nt_te], self.nt_trip[nt_te]
        nt_brs = self.nt_br[nt_te]
        nt_inreg = np.array([k in self.inreg_nodes for k in nt_keys.tolist()], bool)

        def trip_bad(T, only_outside=False):
            lim = np.array([T.get(k, 0) for k in nt_keys.tolist()], np.int64)
            badt = nt_trips > lim
            if only_outside:
                badt &= ~nt_inreg
            m = np.zeros(self.N, bool)
            m[nt_brs[badt]] = True
            return m

        kn_bad_te = ~kn_pres[self.kn[evte]]
        pn_keys = self.pn_keys
        sw_func = np.bincount(func[[b for b in range(len(w)) if not g["chain"][b]]],
                              weights=w[[b for b in range(len(w)) if not g["chain"][b]]],
                              minlength=len(g["funcs"]))
        sw_loop = defaultdict(int)
        for b in range(len(w)):
            if g["chain"][b]:
                sw_loop[g["chain"][b][-1]] += int(w[b])

        def struct(T, sel_kn, label):
            mult = self.mult_fn(T)
            m = np.array([mult(int(k)) for k in pn_keys[self.kn_pn[sel_kn]].tolist()], np.int64)
            bl = self.kn_b[sel_kn]
            return int((m * w[bl]).sum()), int(m.sum()), bl, m * w[bl], mult

        for name, T, static in (("3b_struct_instance_trip", Tb, False), ("3c_struct_static_trip", Tc, True)):
            cost, steps, bl, ww, mult = struct(T, kn_ids, name)
            tb = trip_bad(T)
            out[name] = dict(cost=cost, steps=steps, fail=fails(bad_events=kn_bad_te, bad_branches=tb),
                             by_func=byfunc(bl, ww))
        # 3d: every arm of each laid-out body (static blocks), calls only where reached
        mult = self.mult_fn(Tc)
        pn_pres = np.zeros(len(pn_keys), bool)
        pn_pres[self.pn[evtr]] = True
        cost3d, steps3d, attr = 0, 0, Counter()
        for k in pn_keys[pn_pres].tolist():
            p_, kd, lb, f = self.nodes[k]
            m = mult(k)
            if kd == 1:
                sw, nb = int(sw_func[f]), None
                fn = f
            else:
                sw = sw_loop[lb]
                fn = g["loop_func"][lb]
            cost3d += m * sw
            attr[g["funcs"][fn]] += m * sw
        out["3d_struct_all_arms"] = dict(cost=int(cost3d), steps=None,
                                         fail=fails(bad_events=~pn_pres[self.pn[evte]],
                                                    bad_branches=trip_bad(Tc)),
                                         by_func=[(f, int(c), round(c / cost3d, 3)) for f, c in
                                                  attr.most_common(8)] if attribute else None)
        # 4 hybrid: structured outside the step-machine regions, step machine inside
        out_kn = kn_ids[~self.kn_inreg[kn_ids]]
        inr_tr = evtr & (self.rg >= 0)
        inr_te = evte & (self.rg >= 0)
        rg_tr, ro_tr, rb_tr = self.rg[inr_tr], self.ro[inr_tr], self.b[inr_tr]
        nreg = len(self.rg_keys)
        code = (rg_tr.astype(np.int64) << 34) | (ro_tr.astype(np.int64) << 16) | rb_tr
        pres_r = np.unique(code)
        rT = np.zeros(nreg, np.int64)
        np.maximum.at(rT, rg_tr, ro_tr.astype(np.int64) + 1)
        key2 = (rg_tr.astype(np.int64) << 16) | rb_tr
        u2, inv2 = np.unique(key2, return_inverse=True)
        lo2 = np.full(len(u2), np.iinfo(np.int64).max)
        hi2 = np.full(len(u2), -1)
        np.minimum.at(lo2, inv2, ro_tr)
        np.maximum.at(hi2, inv2, ro_tr)
        rg_te, ro_te, rb_te = self.rg[inr_te], self.ro[inr_te], self.b[inr_te]
        code_te = (rg_te.astype(np.int64) << 34) | (ro_te.astype(np.int64) << 16) | rb_te
        k2te = (rg_te.astype(np.int64) << 16) | rb_te
        j = np.searchsorted(u2, k2te)
        j = np.minimum(j, len(u2) - 1) if len(u2) else j
        found = (u2[j] == k2te) if len(u2) else np.zeros(len(k2te), bool)
        inreg_ev_te = (self.rg[evte] >= 0)
        out_bad = kn_bad_te & ~inreg_ev_te

        def bad_inreg(okmask):
            m = np.zeros(self.N, bool)
            m[self.br[inr_te][~okmask]] = True
            return m

        reg_variants = {
            "exact": (pres_r >> 34, w[pres_r & 0xFFFF], np.ones(len(pres_r), np.int64), pres_r & 0xFFFF,
                      bad_inreg(np.isin(code_te, pres_r))),
            "interval": (u2 >> 16, w[u2 & 0xFFFF] * (hi2 - lo2 + 1), hi2 - lo2 + 1, u2 & 0xFFFF,
                         bad_inreg(found & (ro_te >= lo2[j]) & (ro_te <= hi2[j]))),
            "all_reached": (u2 >> 16, w[u2 & 0xFFFF] * rT[u2 >> 16], rT[u2 >> 16], u2 & 0xFFFF,
                            bad_inreg(found & (ro_te < rT[rg_te]))),
        }
        for tname, T in (("b", Tb), ("c", Tc)):
            mult = self.mult_fn(T)
            m = np.array([mult(int(k)) for k in pn_keys[self.kn_pn[out_kn]].tolist()], np.int64)
            bl = self.kn_b[out_kn]
            s_cost, s_steps = int((m * w[bl]).sum()), int(m.sum())
            rmult = np.array([mult(self.nodes[int(k)][0]) for k in self.rg_keys.tolist()], np.int64)
            tb = trip_bad(T, only_outside=True)
            for vname, (ri, cw, cs_, cb, badm) in reg_variants.items():
                rm = rmult[ri] if len(ri) else np.zeros(0, np.int64)
                r_cost = int((rm * cw).sum())
                r_steps = int((rmult * rT).sum())
                allb = np.concatenate([bl, cb])
                allw = np.concatenate([m * w[bl], rm * cw])
                out[f"4{tname}_hybrid_{vname}"] = dict(
                    cost=s_cost + r_cost, steps=s_steps + r_steps, struct_cost=s_cost, region_cost=r_cost,
                    region_instances=nreg, fail=fails(bad_events=out_bad, bad_branches=tb | badm),
                    by_func=byfunc(allb, allw))
        return out


def static_sizes(roots):
    """What a compiler that knows nothing beyond the IR lays out: every block of a function plus,
    for every direct call slot, the callee laid out in place (loops once, indirect calls ignored, so
    a lower bound). Infinite if a direct-call cycle (recursion) is reachable."""
    g = G
    nf = len(g["funcs"])
    fb = defaultdict(list)
    for b in range(len(g["w"])):
        fb[int(g["func"][b])].append(b)
    memo, onstack = {}, set()

    def size(f):
        if f in memo:
            return memo[f]
        if f in onstack:
            return float("inf")
        onstack.add(f)
        s = 0
        for b in fb[f]:
            s += int(g["w"][b])
            for c in g["calls"][b]:
                if c >= 0:
                    s += size(c)
        onstack.discard(f)
        memo[f] = s
        return s
    out = {}
    for f in roots:
        sys.setrecursionlimit(100000)
        memo.clear()
        out[g["funcs"][f]] = size(f)
    return out


def region_static(loop):
    """Static code a step machine over `loop` must provide per step if it knows only the IR: the
    loop's blocks and every function reachable from them by direct calls (indirect calls counted)."""
    g = G
    fb = defaultdict(list)
    for b in range(len(g["w"])):
        fb[int(g["func"][b])].append(b)
    seen_f, blocks, ind = set(), set(g["loop_body"][loop]), 0
    work = list(blocks)
    while work:
        b = work.pop()
        for c in g["calls"][b]:
            if c == -1:
                ind += 1
            elif c >= 0 and c not in seen_f:
                seen_f.add(c)
                blocks.update(fb[c])
                work.extend(fb[c])
    return dict(blocks=len(blocks), instr=int(sum(int(g["w"][b]) for b in blocks)), functions=len(seen_f),
                indirect_call_slots=ind, total_ir_instr=int(g["w"].sum()))


CACHE = None


def report(build, cache, run, kdir, splits="5"):
    global CACHE
    CACHE = cache
    _init(cache, ())
    jobs = int(os.environ.get("JOBS", "8"))
    W = Workload(run, kdir, jobs)
    N = W.N
    full = W.evaluate(np.ones(N, bool), np.zeros(N, bool), attribute=True)
    rob = defaultdict(list)
    for s in range(int(splits)):
        rng = np.random.default_rng(s)
        perm = rng.permutation(N)
        train = np.zeros(N, bool)
        train[perm[:N // 2]] = True
        res = W.evaluate(train, ~train)
        for k, v in res.items():
            rob[k].append(v["fail"])
    roots = sorted({int(W.nodes[k][3]) for k in W.nodes if W.nodes[k][0] == 0 and W.nodes[k][1] == 1})
    stat = static_sizes(roots)
    regs = json.load(open(os.path.join(kdir, "trace.json")))["region_loops"]
    info = dict(N=N, run_mean=float(W.instr.mean()), run_max=int(W.instr.max()), run_min=int(W.instr.min()),
                blocks_mean=float(W.len.mean()), blocks_max=int(W.len.max()),
                families={k: v | {"fail_half": rob[k], "test_branches": N - N // 2} for k, v in full.items()},
                static_inline={k: (v if v != float("inf") else "infinite") for k, v in stat.items()},
                region_loops=regs, region_static={str(l): region_static(l) for l in regs},
                loop_static_trips_top=None)
    json.dump(info, open(os.path.join(kdir, "schedule.json"), "w"), indent=1, default=str)
    m, mx = info["run_mean"], info["run_max"]
    print(f"N={N} one run: mean {m:.0f} IR instr, max {mx}; blocks mean {W.len.mean():.0f} max {W.len.max()}")
    print(f"{'family':32} {'cost':>12} {'x mean':>9} {'x max':>9} {'steps':>9}  fail/{N - N // 2} (5 splits)")
    for k, v in info["families"].items():
        print(f"{k:32} {v['cost']:>12} {v['cost'] / m:>9.2f} {v['cost'] / mx:>9.2f} {str(v['steps']):>9}  "
              f"{v['fail_half']}")
    print(json.dumps({"static_inline": info["static_inline"], "region_static": info["region_static"]}))

if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "prep":
        prep(*sys.argv[2:4])
    elif cmd == "trace":
        g = pickle.load(open(sys.argv[3], "rb"))
        trace(*sys.argv[2:6], region_loops(g))
    elif cmd == "report":
        report(*sys.argv[2:])
