"""Resolves undefined weak references in a linked LLVM module in text form to null, as a static linker does:
every `@x = extern_weak ...` global and every `declare extern_weak ... @x(...)` is removed and each use of @x
becomes null. picolibc tests `&stdin != NULL` this way; bennett-front refuses a global or function that has no
definition, weak or not.

    python3 -I weak.py <in.ll> <out.ll>    # prints the names it resolved to null on stderr
"""

import re
import sys


def main():
    lines = open(sys.argv[1]).read().split("\n")
    names = set()
    kept = []
    for line in lines:
        m = re.match(r'(@[\w.$"-]+) = extern_weak ', line) or re.match(r'declare extern_weak .*?(@[\w.$"-]+)\(', line)
        if m and m.group(1) not in {"@__init_array_start", "@__init_array_end", "@__fini_array_start", "@__fini_array_end"}:
            names.add(m.group(1))
            continue
        kept.append(line)
    if names:
        pattern = re.compile("(" + "|".join(re.escape(n) for n in sorted(names, key=len, reverse=True)) +
                             r')(?![\w.$"-])')
        kept = [pattern.sub("null", line) if not line.startswith("!") else line for line in kept]
        print(" ".join(sorted(names)), file=sys.stderr)
    open(sys.argv[2], "w").write("\n".join(kept))


main()
