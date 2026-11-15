"""The probe's output hash: 64-bit FNV-1a over the bytes of stdout, then one byte, the exit status mod 256.
The same function as hash() in musl-rt.c and picolibc-rt.c.

    python3 hash.py <stdout file> <exit status>   # prints the hash in decimal
"""

import sys

h = 0xCBF29CE484222325
for byte in open(sys.argv[1], "rb").read() + bytes([int(sys.argv[2]) % 256]):
    h = ((h ^ byte) * 0x100000001B3) % (1 << 64)
print(h)
