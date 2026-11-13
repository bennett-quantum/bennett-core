#!/usr/bin/env python3
"""Rewrite SanitizerCoverage-instrumented LLVM IR into the tracer's ABI.

Input: textual IR produced by
  clang -O2 -gline-tables-only -S -emit-llvm \
        -fsanitize-coverage=func,trace-pc-guard,trace-loads,trace-stores \
        -finstrument-functions-after-inlining

Output IR in which
  * every __sanitizer_cov_{load,store}N(ptr p) becomes __tr_ld/__tr_st(ptr p, i32 site, i32 N)
    with a static site id (one per IR instruction, so sites survive codegen merging);
  * loads/stores that SanitizerCoverage skips (i24) get the same hook inserted;
  * memory intrinsics and the libc string/memory routines that SQLite calls get a
    __tr_mem(ptr a, ptr b, i64 n, i32 site, i32 kind) hook inserted before the call
    (the call itself is kept);
  * __sanitizer_cov_trace_pc_guard calls are deleted (they were only needed to switch
    the coverage pass on) and every basic block gets __tr_bb(i32 block, i32 ninstr) after
    its phi nodes, where ninstr counts the block's original IR instructions.

Side outputs: sites.tsv and blocks.tsv describing every site and block.
"""
import re
import sys

KIND = {  # must match hooks.c
    "memcpy": 0, "memmove": 0, "memset": 1, "memcmp": 2, "bcmp": 2, "strlen": 3,
    "strcmp": 4, "strncmp": 5, "memchr": 6, "strchr": 7, "strrchr": 8,
    "strcspn": 9, "strspn": 10,
}
LIBC_RE = re.compile(r"@(memcpy|memmove|memset|memcmp|bcmp|strlen|strcmp|strncmp|memchr|strchr|strrchr|strcspn|strspn)\(")
INTR_RE = re.compile(r"@llvm\.(memcpy|memmove|memset)(?:\.inline)?\.p0[.\w]*\(")
SANCOV_RE = re.compile(r"call void @__sanitizer_cov_(load|store)(\d+)\(")
GUARD_RE = re.compile(r"^\s*call void @__sanitizer_cov_trace_pc_guard\(")
LABEL_RE = re.compile(r"^([\w.$-]+):")
DBG_RE = re.compile(r"!dbg (!\d+)")
NOCOUNT_RE = re.compile(r"@(__sanitizer_cov_|__cyg_profile_func_|llvm\.dbg\.|llvm\.lifetime\.|llvm\.returnaddress)")
I24_RE = re.compile(r"^\s*(?:%[\w.]+ = )?(load|store) (?:volatile )?i24(?:, | )")
ATTR_WORDS = {"noundef", "nonnull", "writeonly", "readonly", "nocapture", "noalias",
              "immarg", "returned", "signext", "zeroext", "inreg"}


def match_paren(s, i):
    """s[i] == '('; return index of the matching ')'."""
    depth = 0
    for j in range(i, len(s)):
        c = s[j]
        if c in "([{<":
            depth += 1
        elif c in ")]}>":
            depth -= 1
            if depth == 0:
                return j
    raise ValueError("unbalanced: " + s)


def split_top(s):
    out, depth, cur = [], 0, []
    for c in s:
        if c in "([{<":
            depth += 1
        elif c in ")]}>":
            depth -= 1
        if c == "," and depth == 0:
            out.append("".join(cur).strip())
            cur = []
        else:
            cur.append(c)
    if cur:
        out.append("".join(cur).strip())
    return out


def arg_value(arg):
    """'ptr noundef nonnull align 8 dereferenceable(64) %1' -> ('ptr', '%1')."""
    toks = arg.split(" ", 1)
    ty, rest = toks[0], toks[1]
    while True:
        m = re.match(r"(\w+)(\(\d+\))?\s+", rest)
        if not m:
            break
        w = m.group(1)
        if w in ATTR_WORDS or w.startswith("dereferenceable"):
            rest = rest[m.end():]
        elif w == "align":
            rest = re.sub(r"^align\s+\d+\s+", "", rest)
        else:
            break
    return ty, rest.strip()


def call_args(line, open_idx):
    close = match_paren(line, open_idx)
    return [arg_value(a) for a in split_top(line[open_idx + 1:close])]


def load_store_ptr(line):
    """Pointer operand of a load/store instruction line."""
    body = line.split(" = ", 1)[1] if " = load" in line else line
    body = body.split(", !")[0]  # drop metadata
    parts = split_top(body.strip())
    for p in parts:
        if p.startswith("ptr "):
            return arg_value(p)[1]
    raise ValueError(line)


def main(src, dst, sites_path, blocks_path):
    lines = open(src).read().split("\n")
    meta = {}
    for ln in lines:
        if ln.startswith("!") and " = " in ln:
            k, v = ln.split(" = ", 1)
            meta[k] = v

    def scope_name(sid):
        seen = 0
        while sid and seen < 64:
            seen += 1
            v = meta.get(sid, "")
            if "DISubprogram(" in v:
                m = re.search(r'name: "([^"]*)"', v)
                return m.group(1) if m else "?"
            m = re.search(r"scope: (!\d+)", v)
            sid = m.group(1) if m else None
        return "?"

    def loc(line):
        m = DBG_RE.search(line)
        if not m:
            return ("?", 0, "?")
        v = meta.get(m.group(1), "")
        lm = re.search(r"line: (\d+)", v)
        sm = re.search(r"scope: (!\d+)", v)
        inner = scope_name(sm.group(1)) if sm else "?"
        outer = inner
        im = re.search(r"inlinedAt: (!\d+)", v)
        hops = 0
        while im and hops < 64:
            hops += 1
            v2 = meta.get(im.group(1), "")
            sm2 = re.search(r"scope: (!\d+)", v2)
            outer = scope_name(sm2.group(1)) if sm2 else outer
            im = re.search(r"inlinedAt: (!\d+)", v2)
        return (inner, int(lm.group(1)) if lm else 0, outer)

    out = []
    sites = []   # (kind, size, irfunc, inner, line, outer)
    blocks = []  # (irfunc, label, ninstr)
    in_func = False
    func = None
    pending_block = None  # index in out where to insert bb hook, block id
    cur_block = None
    i = 0
    n = len(lines)

    def new_site(kind, size, line):
        sites.append((kind, size, func) + loc(line))
        return len(sites) - 1

    def open_block(label):
        nonlocal cur_block
        blocks.append([func, label, 0])
        cur_block = len(blocks) - 1

    while i < n:
        ln = lines[i]
        if not in_func:
            out.append(ln)
            if ln.startswith("define ") and ln.rstrip().endswith("{"):
                in_func = True
                func = re.search(r"@([\w.$\"]+)\(", ln).group(1)
                open_block("entry")
                bb_insert_at = len(out)
                out.append(None)  # placeholder for bb hook
                blocks[cur_block].append(bb_insert_at)
            i += 1
            continue
        if ln == "}":
            in_func = False
            out.append(ln)
            i += 1
            continue
        m = LABEL_RE.match(ln)
        if m:
            out.append(ln)
            open_block(m.group(1))
            # skip phis: placeholder goes after them
            j = i + 1
            while j < n and re.match(r"^\s+%[\w.]+ = phi ", lines[j]):
                out.append(lines[j])
                blocks[cur_block][2] += 1
                j += 1
            blocks[cur_block].append(len(out))
            out.append(None)
            i = j
            continue
        if not ln.startswith("  ") or ln.strip() == "":
            out.append(ln)
            i += 1
            continue
        # continuation lines of a multi-line instruction (switch cases, closing bracket)
        if ln.startswith("    ") or ln.startswith("  ]"):
            out.append(ln)
            i += 1
            continue
        # instruction line
        if GUARD_RE.match(ln):
            i += 1
            continue
        if not NOCOUNT_RE.search(ln):
            blocks[cur_block][2] += 1
        m = SANCOV_RE.search(ln)
        if m:
            kind, size = m.group(1), int(m.group(2))
            op = m.end() - 1
            ptr = call_args(ln, op)[0][1]
            sid = new_site("ld" if kind == "load" else "st", size, ln)
            dbg = DBG_RE.search(ln)
            hook = "__tr_ld" if kind == "load" else "__tr_st"
            out.append(f"  call void @{hook}(ptr {ptr}, i32 {sid}, i32 {size})" +
                       (f", !dbg {dbg.group(1)}" if dbg else ""))
            # the coverage call was not counted (NOCOUNT_RE); keep counts honest
            i += 1
            continue
        mi = I24_RE.match(ln)
        if mi:
            ptr = load_store_ptr(ln)
            kind = "ld" if mi.group(1) == "load" else "st"
            sid = new_site(kind, 3, ln)
            dbg = DBG_RE.search(ln)
            out.append(f"  call void @__tr_{kind}(ptr {ptr}, i32 {sid}, i32 3)" +
                       (f", !dbg {dbg.group(1)}" if dbg else ""))
            out.append(ln)
            i += 1
            continue
        mm = INTR_RE.search(ln) or LIBC_RE.search(ln)
        if mm and " call " in (" " + ln) and not ln.lstrip().startswith("declare"):
            name = mm.group(1)
            args = call_args(ln, mm.end() - 1)
            a = args[0][1]
            b = "null"
            nval = "0"
            if name in ("memcpy", "memmove"):
                b, nval = args[1][1], args[2]
            elif name == "memset":
                nval = args[2]
            elif name in ("memcmp", "bcmp", "strncmp"):
                b, nval = args[1][1], args[2]
            elif name in ("strcmp", "strcspn", "strspn"):
                b = args[1][1]
            elif name == "memchr":
                # the searched byte travels in b as an integer-valued pointer
                nval = args[2]
                cval = args[1]
                b = None
            elif name in ("strchr", "strrchr"):
                cval = args[1]
                b = None
            if isinstance(nval, tuple):
                if nval[0] != "i64":
                    raise ValueError("size type " + ln)
                nval = nval[1]
            pre = []
            if b is None:  # character argument for memchr/strchr/strrchr
                tmp = f"%__trc{len(sites)}"
                pre.append(f"  {tmp} = inttoptr i32 {cval[1]} to ptr")
                b = tmp
            sid = new_site("mem:" + name, 0, ln)
            dbg = DBG_RE.search(ln)
            out.extend(pre)
            out.append(f"  call void @__tr_mem(ptr {a}, ptr {b}, i64 {nval}, i32 {sid}, i32 {KIND[name]})" +
                       (f", !dbg {dbg.group(1)}" if dbg else ""))
            out.append(ln)
            i += 1
            continue
        out.append(ln)
        i += 1

    # fill bb placeholders
    for bid, b in enumerate(blocks):
        pos = b[3]
        out[pos] = f"  call void @__tr_bb(i32 {bid}, i32 {b[2]})"
    text = "\n".join(out)
    text = re.sub(r"declare void @__sanitizer_cov_(load|store)\d+\(ptr\)\n", "", text)
    text += ("\ndeclare void @__tr_ld(ptr, i32, i32)\ndeclare void @__tr_st(ptr, i32, i32)\n"
             "declare void @__tr_mem(ptr, ptr, i64, i32, i32)\ndeclare void @__tr_bb(i32, i32)\n")
    open(dst, "w").write(text)
    with open(sites_path, "w") as f:
        f.write("site\tkind\tsize\tir_func\tsrc_func\tline\touter_func\n")
        for sid, s in enumerate(sites):
            f.write(f"{sid}\t{s[0]}\t{s[1]}\t{s[2]}\t{s[3]}\t{s[4]}\t{s[5]}\n")
    with open(blocks_path, "w") as f:
        f.write("block\tir_func\tlabel\tninstr\n")
        for bid, b in enumerate(blocks):
            f.write(f"{bid}\t{b[0]}\t{b[1]}\t{b[2]}\n")
    print(f"sites={len(sites)} blocks={len(blocks)} ir_instr={sum(b[2] for b in blocks)}", file=sys.stderr)


if __name__ == "__main__":
    main(*sys.argv[1:5])
