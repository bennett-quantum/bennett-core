"""Summarises a results.tsv written by run-all.sh: outcome counts, refusal reasons ranked, and the other classes.

    python3 summarize.py <results.tsv>
"""

import collections
import re
import sys

OUTCOMES = ["agrees", "refused", "fault", "wrong", "crash", "budget", "skipped"]
LABELS = {
    "agrees": "agrees",
    "refused": "refused by name",
    "fault": "named fault or stop (native passed)",
    "wrong": "wrong",
    "crash": "crash",
    "budget": "budget",
    "skipped": "skipped",
}


def normalise(reason):
    """A refusal's reason without its location, test-specific names or numbers."""
    text = reason.removeprefix("not handled yet: ")
    text = re.sub(r", at [^,]*$", "", text)
    # Intrinsics keep their family: @llvm.fabs.f64 -> @llvm.fabs; @llvm.umul.with.overflow.i32 -> ...with.overflow
    text = re.sub(r"@llvm\.([a-z_.]+?)(\.(?:[ivfp]\d+|p0|v\d+[if]\d+|f\d+|x86_fp80|f128|ppc_fp128))+\b", r"@llvm.\1", text)
    if "@llvm." not in text and "no definition" not in text:
        text = re.sub(r"@[\w.$]+", "@f", text)
    text = re.sub(r"\bqubit variable \w+", "qubit variable v", text)
    text = re.sub(r"the constant -?[0-9.]+e[+-][0-9]+$|the constant 0x[KLMH]?[0-9A-F]+$",
                  "a floating-point constant", text)
    text = re.sub(r"the constant (\w+)[ (].*", r"the constant expression \1(...)", text)
    text = re.sub(r"the initializer .* of type (.*) in @f", r"the initializer of type \1", text)
    text = re.sub(r"multiple of \d+", "multiple of N", text)
    return text


FAMILIES = [  # (family, pattern on the normalised reason), first match wins
    ("floating point", r"double|float|x86_fp80|fp128|floating-point|[su]itofp|fpto[su]i|fcmp|fadd|fmul|fdiv|fsub|fneg|@llvm\.(fabs|copysign|fmuladd|sqrt|is\.fpclass)"),
    ("variadic functions (definitions, and calls such as printf)", r"variadic"),
    ("inline assembly", r"inline assembly"),
    ("vector values", r"<\d+ x |extractelement|insertelement|shufflevector|the constant <"),
    ("aggregate (struct) values in registers", r"a value of type \{|extractvalue|insertvalue|zeroinitializer"),
    ("integers wider than 64 bits", r"type i(6[5-9]|[7-9]\d|\d{3,})\b|the constant \d{19,}"),
    ("computed goto (indirectbr, blockaddress)", r"indirectbr|blockaddress"),
    ("intrinsics: overflow and saturating arithmetic", r"with\.overflow|\.sat\b"),
    ("intrinsics: other", r"the intrinsic"),
    ("library not provided (stdio, setjmp, ctype, qsort ...)", r"no definition"),
    ("constant expressions on addresses", r"the constant expression"),
]


def family(reason):
    for name, pattern in FAMILIES:
        if re.search(pattern, reason):
            return name
    return "other: " + reason


def main():
    rows = [line.rstrip("\n").split("\t") for line in open(sys.argv[1]) if line.strip()]
    counts = collections.Counter(row[1] for row in rows)
    total = len(rows)
    ran = total - counts["skipped"]
    print(f"programs: {total}, not skipped: {ran}\n")
    print("| outcome | count | % of all | % of not skipped |")
    print("|---|---:|---:|---:|")
    for outcome in OUTCOMES:
        n = counts[outcome]
        of_ran = f"{100 * n / ran:.1f}" if outcome != "skipped" and ran else ""
        print(f"| {LABELS[outcome]} | {n} | {100 * n / total:.1f} | {of_ran} |")
    print()
    refused = [row for row in rows if row[1] == "refused"]
    if refused:
        print(f"### refusals by family ({len(refused)})\n")
        print("| count | family |")
        print("|---:|---|")
        for name, n in collections.Counter(family(normalise(row[2])) for row in refused).most_common():
            print(f"| {n} | {name} |")
        print()
    for outcome, key in [("refused", normalise), ("fault", None), ("skipped", None), ("crash", None),
                         ("wrong", None), ("budget", None)]:
        selected = [row for row in rows if row[1] == outcome]
        if not selected:
            continue
        reasons = collections.Counter((key or (lambda r: r))(row[2]) for row in selected)
        print(f"### {LABELS[outcome]} ({len(selected)})\n")
        print("| count | reason |")
        print("|---:|---|")
        for reason, n in reasons.most_common():
            print(f"| {n} | {reason.replace('|', '\\|')} |")
        print()
        if outcome in ("wrong", "crash", "fault", "budget"):
            print(" ".join(sorted(row[0] for row in selected)))
            print()


main()
