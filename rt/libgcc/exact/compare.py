"""compare.py [--raw] A B [random]: the output of cplx_exact.c from two builds, per helper: identical tuples,
tuples that agree by the NaN payload rule, and tuples that differ otherwise (the first few shown).

The NaN payload rule (the maintainer's decision of the afternoon of 2026-11-16): a result that differs from
the native one only in which NaN's payload it carries counts as agreement. Exactly: a component agrees when
its bits are equal, or when both are NaNs and each carries (quiet bit set, sign free) the payload of one of
the tuple's NaN operands or of the format's default NaN, the NaN an invalid operation makes. C and LLVM leave
which NaN operand an operation returns, and its sign, unspecified; nothing else is allowed. Exit status 1 if
any tuple differs otherwise; with `--raw`, if any tuple differs at all (raw bits, for the diagnostics that
show where NaN choices come from). `compare.py --self-test` checks the rule on cases worked by hand."""
import sys

ND = 20
ops = [("mulsc3", 8), ("divsc3", 8), ("muldc3", 16), ("divdc3", 16), ("mulxc3", 20), ("divxc3", 20)]
MASK64 = (1 << 64) - 1

# The directed operands of cplx_exact.c, as raw bits (x87: sign-exponent << 64 | significand).
DF = [0x00000000, 0x80000000, 0x3f800000, 0xc0400000, 0x00000001, 0xff7fffff, 0x00800000, 0x7f800000,
      0xff800000, 0x7fc00000, 0xffc12345, 0x3f400000, 0x7f000000, 0x01800000, 0x5f000000, 0x1f000000, 0x7f800001,
      0xff812345, 0x007fffff, 0x80000001]
DD = [0x0000000000000000, 0x8000000000000000, 0x3ff0000000000000, 0xc008000000000000, 0x0000000000000001,
      0xffefffffffffffff, 0x0010000000000000, 0x7ff0000000000000, 0xfff0000000000000, 0x7ff8000000000000,
      0xfff8000000012345, 0x3fe8000000000000, 0x7fe0000000000000, 0x0030000000000000, 0x5fe0000000000000,
      0x1fe0000000000000, 0x7ff0000000000001, 0xfff0000000012345, 0x000fffffffffffff, 0x8000000000000001]
DXE = [0x0000, 0x8000, 0x3fff, 0xc000, 0x0000, 0xfffe, 0x0001, 0x7fff, 0xffff, 0x7fff, 0xffff, 0x3ffe, 0x7ffe,
       0x0003, 0x5fff, 0x1fff, 0x7fff, 0xffff, 0x0000, 0x8000]
DXM = [0, 0, 0x8000000000000000, 0xc000000000000000, 1, 0xffffffffffffffff, 0x8000000000000000,
       0x8000000000000000, 0x8000000000000000, 0xc000000000000000, 0xc000000000012345, 0xc000000000000000,
       0x8000000000000000, 0x8000000000000000, 0x8000000000000000, 0x8000000000000000, 0x8000000000000001,
       0x8000000000012345, 0x7fffffffffffffff, 1]
DX = [e << 64 | m for e, m in zip(DXE, DXM)]


class Generator:
    """cplx_exact.c's xorshift and operand choices, drawn in the same order."""

    def __init__(self):
        self.state = 0x9e3779b97f4a7c15

    def next(self):
        s = self.state
        s ^= (s << 13) & MASK64
        s ^= s >> 7
        s ^= (s << 17) & MASK64
        self.state = s
        return s

    def f(self):
        r = self.next()
        k = r & 3
        if k == 0:
            return (r >> 32) & 0xffffffff
        if k == 1:
            return ((r >> 32) & 0x807fffff) | (0x7f - 24 + ((r >> 8) & 63)) << 23
        if k == 2:
            return DF[(r >> 8) % ND]
        return ((r >> 32) & 0x807fffff) | ((r >> 8) & 0xff) << 23

    def d(self):
        r, s = self.next(), self.next()
        k = r & 3
        if k == 0:
            return s
        if k == 1:
            return (s & 0x800fffffffffffff) | (0x3ff - 60 + ((r >> 8) & 127)) << 52
        if k == 2:
            return DD[(r >> 8) % ND]
        return (s & 0x800fffffffffffff) | ((r >> 8) & 0x7ff) << 52

    def x(self):
        r, m = self.next(), self.next()
        k = r & 3
        if k == 0:
            e = (r >> 16) & 0xffff
        elif k == 1:
            e = ((r >> 16) & 0x8000) | (0x3fff - 70 + ((r >> 8) & 127))
        elif k == 2:
            return DX[(r >> 8) % ND]
        else:
            e = (((r >> 16) & 0x8000) | ((r >> 4) & 0xf) | ((r >> 24) & 0x7) << 12 | ((r >> 8) & 0xff) << 4) & 0xffff
        m = m & 0x7fffffffffffffff if e & 0x7fff == 0 else m | 0x8000000000000000
        return e << 64 | m


def operands(random):
    """Per helper, in output order, the operand bits of every tuple: directed, then random."""
    gen = Generator()
    for draw, directed in ((Generator.f, DF), (Generator.d, DD), (Generator.x, DX)):
        for _ in range(2):
            tuples = [(directed[i // ND**3], directed[i // ND**2 % ND], directed[i // ND % ND], directed[i % ND])
                      for i in range(ND**4)]
            tuples += [tuple(draw(gen) for _ in range(4)) for _ in range(random)]
            yield tuples


def nan(v, w):
    if w == 8:
        return (v & 0x7fffffff) > 0x7f800000
    if w == 16:
        return (v & 0x7fffffffffffffff) > 0x7ff0000000000000
    return (v >> 64) & 0x7fff == 0x7fff and (v & ((1 << 63) - 1)) != 0


def payload(v, w):
    """A NaN's payload with its quiet bit set (an operation quiets a signalling NaN); the sign left out."""
    if w == 8:
        return v & 0x7fffff | 0x400000
    if w == 16:
        return v & 0xfffffffffffff | 0x8000000000000
    return v & MASK64 | 0xc000000000000000


# The default NaN's payload: the quiet bit alone (x87: the integer and quiet bits).
DEFAULT = {8: 0x400000, 16: 0x8000000000000, 20: 0xc000000000000000}


def agrees(p, q, w, tuple_operands):
    """Whether result components p and q (bits) agree: equal, or by the NaN payload rule."""
    if p == q:
        return True
    if not (nan(p, w) and nan(q, w)):
        return False
    allowed = {payload(o, w) for o in tuple_operands if nan(o, w)} | {DEFAULT[w]}
    return payload(p, w) in allowed and payload(q, w) in allowed


def self_test():
    qnan, snan_payload, other = 0x7fc00000, 0x7f812345, 0x7fc0beef
    one, two = 0x3f800000, 0x40000000
    cases = [
        # Equal bits, NaN or not.
        ((one, one, 8, (one, one, one, one)), True),
        ((other, other, 8, (one, one, one, one)), True),
        # Not NaNs: never by the rule.
        ((one, two, 8, (qnan, one, one, one)), False),
        # One NaN, one number.
        ((qnan, one, 8, (qnan, one, one, one)), False),
        # Two input NaNs: either payload, either sign.
        ((0xffc12345, qnan, 8, (0xffc12345, qnan, one, one)), True),
        ((0x7fc12345 | 0x80000000, qnan, 8, (0x7fc12345, qnan, one, one)), True),
        # A signalling input NaN is carried quieted.
        ((0x7fc12345, 0xffc00000, 8, (snan_payload, one, one, one)), True),
        # The default NaN (an invalid operation's) against an input NaN.
        ((0xffc00000, 0x7fc12345, 8, (0x7fc12345, one, one, one)), True),
        # A payload no operand carries: not allowed.
        ((other, qnan, 8, (qnan, one, one, one)), False),
        ((other, 0x7fc12345, 8, (0x7fc12345, one, one, one)), False),
        # Double and x87, the same rule.
        ((0xfff8000000012345, 0x7ff8000000000000, 16, (0xfff8000000012345, 0, 0, 0)), True),
        ((0x7ff800000000beef, 0x7ff8000000000000, 16, (0xfff8000000012345, 0, 0, 0)), False),
        ((0xffff << 64 | 0xc000000000012345, 0x7fff << 64 | 0xc000000000000000, 20,
          (0x7fff << 64 | 0x8000000000012345, 0, 0, 0)), True),
        ((0xffff << 64 | 0xc00000000000beef, 0x7fff << 64 | 0xc000000000000000, 20, (0, 0, 0, 0)), False),
    ]
    for (args, expected) in cases:
        assert agrees(*args) == expected, (args, expected)
    # The generator: the first random float operand, worked by hand from the seed.
    gen = Generator()
    s = 0x9e3779b97f4a7c15
    s ^= (s << 13) & MASK64
    s ^= s >> 7
    s ^= (s << 17) & MASK64
    assert gen.next() == s
    print(f"compare.py self-test: {len(cases)} cases agree with the rule")


def main():
    if sys.argv[1:] == ["--self-test"]:
        self_test()
        return
    args = sys.argv[1:]
    raw = args[:1] == ["--raw"]
    args = args[1:] if raw else args
    random = int(args[2]) if len(args) > 2 else 262144
    n = ND**4 + random
    a, b = (open(p).read().strip() for p in args[:2])
    assert len(a) == len(b) == n * sum(2 * w for _, w in ops), (len(a), len(b), n)
    pos = 0
    failures = 0
    for (name, w), tuples in zip(ops, operands(random)):
        size = n * 2 * w
        x, y = a[pos:pos + size], b[pos:pos + size]
        pos += size
        exact = by_rule = other = 0
        shown = nan_shown = 0
        for i in range(n):
            ra, ia = x[2 * i * w:(2 * i + 1) * w], x[(2 * i + 1) * w:(2 * i + 2) * w]
            rb, ib = y[2 * i * w:(2 * i + 1) * w], y[(2 * i + 1) * w:(2 * i + 2) * w]
            if (ra, ia) == (rb, ib):
                exact += 1
            elif all(agrees(int(p, 16), int(q, 16), w, tuples[i]) for p, q in ((ra, rb), (ia, ib))):
                by_rule += 1
                if nan_shown < 2:
                    nan_shown += 1
                    print(f"  {name} #{i} NaN payload: {ra} {ia} vs {rb} {ib}")
            else:
                other += 1
                if shown < 3:
                    shown += 1
                    print(f"  {name} #{i} ({'directed' if i < ND**4 else 'random'}): {ra} {ia} vs {rb} {ib}")
        failures += other + (by_rule if raw else 0)
        print(f"{name}: {n} tuples, {exact} identical, {by_rule} agree by the NaN payload rule, "
              f"{other} differ otherwise")
    assert pos == len(a) == len(b), (pos, len(a), len(b))
    sys.exit(bool(failures))


if __name__ == "__main__":
    main()
