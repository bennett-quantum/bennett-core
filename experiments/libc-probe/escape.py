"""Prints a file's bytes as `bennett run` prints its console: quoted, with Rust's `<[u8]>::escape_ascii`
(\\t \\r \\n \\\\ \\' \\" escaped, printable ASCII as is, every other byte as \\xNN); nothing for an empty file.

    python3 -I escape.py <file>
"""

import sys

data = open(sys.argv[1], "rb").read()
out = []
for b in data:
    c = chr(b)
    if c in "\t\r\n\\'\"":
        out.append({"\t": "\\t", "\r": "\\r", "\n": "\\n", "\\": "\\\\", "'": "\\'", '"': '\\"'}[c])
    elif 0x20 <= b < 0x7F:
        out.append(c)
    else:
        out.append(f"\\x{b:02x}")
if data:
    print('"' + "".join(out) + '"')
