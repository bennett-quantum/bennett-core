### N = 256, `arith`: `SELECT (a * 7 + b) % 13 = 5 FROM t WHERE id = ?1`

Results over branches: {'0': 232, '1': 24}

n4: steps per branch

| measure | min | median | mean | max | max/mean | max/median | histogram (12 equal bins, min..max) |
|---|---|---|---|---|---|---|---|
| basic blocks | 545 | 577 | 577 | 608 | 1.05 | 1.05 | 3 6 13 32 32 37 48 24 30 24 4 3 |
| LLVM IR instructions | 3,555 | 3,752 | 3,749 | 3,909 | 1.04 | 1.04 | 2 3 11 21 23 38 49 38 38 18 11 4 |
| x86 user instructions (plain build) | 2,754 | 2,885 | 2,884 | 2,993 | 1.04 | 1.04 | 2 4 10 22 28 41 42 36 34 21 12 4 |

Control flow: identical for the first 199 blocks (36.5% of the shortest run); 256 distinct block sequences over 256 branches.

n5: memory accesses

Per branch: loads median 612 (max 630), stores median 214 (max 215), memory-routine calls median 9.

| quantity | alignment A (site, k) | alignment B (call path, site, k) |
|---|---|---|
| aligned access positions (scalar) | 859 | 859 |
| dynamic scalar accesses, all branches | 207,923 | 207,923 |
| dynamic accesses at positions every branch executes | 94.6% | 94.6% |
| **dynamic accesses with branch-varying address** | 10.1% | 10.1% |
| positions with branch-varying address | 12.5% | 12.5% |
| varying: loads from memory the region never writes (dyn) | 20,051 | 20,051 |
| varying: loads from memory the region writes (dyn) | 512 | 512 |
| varying: stores (dyn) | 512 | 512 |
| lookup size of varying positions: median / mean / max (distinct addresses) | 19 / 51.4 / 256 | 19 / 51.4 / 256 |
| lookup size histogram (positions) | <=2^1: 2, <=2^2: 6, <=2^3: 11, <=2^4: 6, <=2^5: 48, <=2^6: 13, <=2^7: 8, <=2^8: 13 | <=2^1: 2, <=2^2: 6, <=2^3: 11, <=2^4: 6, <=2^5: 48, <=2^6: 13, <=2^7: 8, <=2^8: 13 |
| sum over positions of lookup size (8-byte words) | 5,709 | 5,709 |
| naive sum: positions x words the region touches | 1,174,253 | 1,174,253 |
| lookup sum / naive sum | 0.0049 | 0.0049 |
| lookup sum / positions (mean words per access) | 6.65 | 6.65 |
| varying positions by region | sqlite_heap: 105, driver_trace:r--p: 2 | sqlite_heap: 105, driver_trace:r--p: 2 |
| memory-routine calls with varying address / length (positions) | 0 / 0 of 13; bytes 0 of 48,896 at varying addresses | 0 / 0 of 13; bytes 0 of 48,896 at varying addresses |

| bytes | total | in SQLite heap | stack | other |
|---|---|---|---|---|
| touched by any branch | 7,395 | 7,312 | 44 | 39 |
| touched by every branch | 1,511 | 1,434 | 44 | 33 |
| written by any branch | 832 | 788 | 44 | 0 |
| final value differs between branches | 53 | 43 | 10 | 0 |

Context: SQLite heap in use 331,008 bytes; process mapped 74,555,392 bytes (includes the 64 MiB arena); VmRSS 5416 kB. Words touched by any branch: 1,367.

Top functions (source function after inlining) by dynamic varying-address accesses, alignment A: sqlite3BtreeTableMoveto 7,483, sqlite3VarintValue 2,412, sqlite3VdbeExec 2,140, btreeParseCell 1,536, btreeParseCellPtr 1,092, sqlite3PcacheRelease 1,024, sqlite3PcacheFetchFinish 1,024, sqlite3Get4byte 1,018, pcache1FetchNoMutex 768, sqlite3VdbeOneByteSerialTypeLen 512

Top functions by summed lookup size, alignment A: sqlite3VdbeExec 1,886, sqlite3BtreeTableMoveto 1,276, btreeParseCellPtr 855, btreeParseCell 588, sqlite3VarintValue 440, sqlite3PcacheRelease 76, sqlite3PcacheFetchFinish 76, sqlite3Get4byte 73, pcache1FetchNoMutex 57, moveToChild 38

### N = 256, `like_name`: `SELECT name LIKE '%qu%' FROM t WHERE id = ?1`

Results over branches: {'0': 249, '1': 7}

n4: steps per branch

| measure | min | median | mean | max | max/mean | max/median | histogram (12 equal bins, min..max) |
|---|---|---|---|---|---|---|---|
| basic blocks | 530 | 564 | 567 | 630 | 1.11 | 1.12 | 6 34 38 46 44 24 31 19 9 2 0 3 |
| LLVM IR instructions | 3,373 | 3,560 | 3,567 | 3,907 | 1.10 | 1.10 | 8 30 36 47 45 25 34 18 8 2 2 1 |
| x86 user instructions (plain build) | 2,685 | 2,844 | 2,872 | 3,309 | 1.15 | 1.16 | 20 55 50 38 24 28 21 10 4 2 1 3 |

Control flow: identical for the first 189 blocks (35.7% of the shortest run); 256 distinct block sequences over 256 branches.

n5: memory accesses

Per branch: loads median 587 (max 630), stores median 221 (max 228), memory-routine calls median 11.

| quantity | alignment A (site, k) | alignment B (call path, site, k) |
|---|---|---|
| aligned access positions (scalar) | 860 | 866 |
| dynamic scalar accesses, all branches | 202,506 | 202,506 |
| dynamic accesses at positions every branch executes | 95.6% | 95.6% |
| **dynamic accesses with branch-varying address** | 10.8% | 10.8% |
| positions with branch-varying address | 13.8% | 13.5% |
| varying: loads from memory the region never writes (dyn) | 19,700 | 19,630 |
| varying: loads from memory the region writes (dyn) | 1,141 | 1,141 |
| varying: stores (dyn) | 1,024 | 1,024 |
| lookup size of varying positions: median / mean / max (distinct addresses) | 19 / 44.2 / 256 | 19 / 44.9 / 256 |
| lookup size histogram (positions) | <=2^1: 3, <=2^2: 9, <=2^3: 11, <=2^4: 8, <=2^5: 53, <=2^6: 18, <=2^7: 6, <=2^8: 11 | <=2^1: 1, <=2^2: 9, <=2^3: 11, <=2^4: 8, <=2^5: 53, <=2^6: 18, <=2^7: 6, <=2^8: 11 |
| sum over positions of lookup size (8-byte words) | 5,177 | 5,183 |
| naive sum: positions x words the region touches | 1,773,320 | 1,785,692 |
| lookup sum / naive sum | 0.0029 | 0.0029 |
| lookup sum / positions (mean words per access) | 6.02 | 5.98 |
| varying positions by region | sqlite_heap: 112, driver_trace:r--p: 7 | sqlite_heap: 110, driver_trace:r--p: 7 |
| memory-routine calls with varying address / length (positions) | 5 / 7 of 25; bytes 7,301 of 66,685 at varying addresses | 5 / 7 of 25; bytes 7,301 of 66,685 at varying addresses |

| bytes | total | in SQLite heap | stack | other |
|---|---|---|---|---|
| touched by any branch | 12,323 | 12,140 | 59 | 124 |
| touched by every branch | 1,523 | 1,415 | 51 | 57 |
| written by any branch | 877 | 818 | 59 | 0 |
| final value differs between branches | 82 | 70 | 12 | 0 |

Context: SQLite heap in use 331,008 bytes; process mapped 74,555,392 bytes (includes the 64 MiB arena); VmRSS 5328 kB. Words touched by any branch: 2,062.

Top functions (source function after inlining) by dynamic varying-address accesses, alignment A: sqlite3BtreeTableMoveto 7,483, sqlite3VarintValue 2,412, sqlite3VdbeExec 1,792, btreeParseCell 1,536, btreeParseCellPtr 1,092, sqlite3PcacheRelease 1,024, sqlite3PcacheFetchFinish 1,024, sqlite3Get4byte 1,018, patternCompare 882, sqlite3VdbeOneByteSerialTypeLen 768

Top functions by summed lookup size, alignment A: sqlite3VdbeExec 1,354, sqlite3BtreeTableMoveto 1,276, btreeParseCellPtr 855, btreeParseCell 588, sqlite3VarintValue 440, patternCompare 248, sqlite3PcacheRelease 76, sqlite3PcacheFetchFinish 76, sqlite3Get4byte 73, pcache1FetchNoMutex 57

### N = 256, `like_note`: `SELECT note LIKE '%zq%' FROM t WHERE id = ?1`

Results over branches: {'0': 201, '1': 55}

n4: steps per branch

| measure | min | median | mean | max | max/mean | max/median | histogram (12 equal bins, min..max) |
|---|---|---|---|---|---|---|---|
| basic blocks | 539 | 647 | 674 | 3,356 | 4.98 | 5.19 | 243 8 2 1 0 1 0 0 0 0 0 1 |
| LLVM IR instructions | 3,427 | 3,984 | 4,142 | 18,616 | 4.49 | 4.67 | 244 7 2 1 0 1 0 0 0 0 0 1 |
| x86 user instructions (plain build) | 2,726 | 3,540 | 3,842 | 28,919 | 7.53 | 8.17 | 248 4 1 1 0 1 0 0 0 0 0 1 |

Control flow: identical for the first 189 blocks (35.1% of the shortest run); 256 distinct block sequences over 256 branches.

n5: memory accesses

Per branch: loads median 641 (max 2,624), stores median 237 (max 657), memory-routine calls median 16.

| quantity | alignment A (site, k) | alignment B (call path, site, k) |
|---|---|---|
| aligned access positions (scalar) | 3,027 | 3,133 |
| dynamic scalar accesses, all branches | 223,442 | 223,442 |
| dynamic accesses at positions every branch executes | 82.5% | 78.0% |
| **dynamic accesses with branch-varying address** | 13.8% | 12.3% |
| positions with branch-varying address | 14.5% | 13.5% |
| varying: loads from memory the region never writes (dyn) | 24,417 | 22,067 |
| varying: loads from memory the region writes (dyn) | 4,829 | 4,314 |
| varying: stores (dyn) | 1,623 | 1,114 |
| lookup size of varying positions: median / mean / max (distinct addresses) | 3 / 21.9 / 256 | 4 / 22.5 / 256 |
| lookup size histogram (positions) | <=2^1: 134, <=2^2: 108, <=2^3: 61, <=2^4: 9, <=2^5: 67, <=2^6: 20, <=2^7: 21, <=2^8: 18 | <=2^1: 117, <=2^2: 108, <=2^3: 63, <=2^4: 9, <=2^5: 67, <=2^6: 20, <=2^7: 21, <=2^8: 18 |
| sum over positions of lookup size (8-byte words) | 9,658 | 9,753 |
| naive sum: positions x words the region touches | 47,753,952 | 49,426,208 |
| lookup sum / naive sum | 0.0002 | 0.0002 |
| lookup sum / positions (mean words per access) | 3.19 | 3.11 |
| varying positions by region | sqlite_heap: 351, driver_trace:r--p: 81, anon:rw-p: 6 | sqlite_heap: 337, driver_trace:r--p: 81, anon:rw-p: 5 |
| memory-routine calls with varying address / length (positions) | 87 / 86 of 401; bytes 249,520 of 307,628 at varying addresses | 87 / 86 of 401; bytes 249,520 of 307,628 at varying addresses |

| bytes | total | in SQLite heap | stack | other |
|---|---|---|---|---|
| touched by any branch | 123,129 | 122,792 | 71 | 266 |
| touched by every branch | 1,497 | 1,391 | 51 | 55 |
| written by any branch | 17,880 | 17,777 | 71 | 32 |
| final value differs between branches | 15,456 | 15,435 | 21 | 0 |

Context: SQLite heap in use 331,008 bytes; process mapped 74,555,392 bytes (includes the 64 MiB arena); VmRSS 5512 kB. Words touched by any branch: 15,776.

Top functions (source function after inlining) by dynamic varying-address accesses, alignment A: sqlite3BtreeTableMoveto 7,483, patternCompare 6,733, sqlite3PcacheRelease 3,096, sqlite3VarintValue 2,412, sqlite3VdbeExec 2,036, btreeParseCell 1,536, btreeParseCellPtr 1,092, sqlite3Get4byte 1,078, sqlite3PcacheFetchFinish 1,060, pcache1FetchNoMutex 795

Top functions by summed lookup size, alignment A: patternCompare 3,226, sqlite3VdbeExec 1,886, sqlite3BtreeTableMoveto 1,276, btreeParseCellPtr 855, btreeParseCell 588, sqlite3GetVarint32 463, sqlite3VarintValue 440, sqlite3PcacheRelease 156, sqlite3Get4byte 133, sqlite3PcacheFetchFinish 112

### N = 256, `scan`: `SELECT count(*) FROM t WHERE a = ?1`

Results over branches: {'2': 51, '0': 89, '4': 4, '1': 99, '3': 13}

n4: steps per branch

| measure | min | median | mean | max | max/mean | max/median | histogram (12 equal bins, min..max) |
|---|---|---|---|---|---|---|---|
| basic blocks | 15,743 | 15,894 | 15,897 | 16,050 | 1.01 | 1.01 | 11 13 28 27 15 36 25 30 20 25 18 8 |
| LLVM IR instructions | 124,948 | 125,335 | 125,337 | 125,753 | 1.00 | 1.00 | 14 13 12 22 45 29 34 28 22 24 8 5 |
| x86 user instructions (plain build) | 88,273 | 88,516 | 88,517 | 88,773 | 1.00 | 1.00 | 17 12 12 24 34 35 26 29 24 31 8 4 |

Control flow: identical for the first 249 blocks (1.6% of the shortest run); 227 distinct block sequences over 256 branches.

n5: memory accesses

Per branch: loads median 21,242 (max 21,281), stores median 8,289 (max 8,295), memory-routine calls median 14.

| quantity | alignment A (site, k) | alignment B (call path, site, k) |
|---|---|---|
| aligned access positions (scalar) | 29,821 | 29,822 |
| dynamic scalar accesses, all branches | 7,552,324 | 7,552,324 |
| dynamic accesses at positions every branch executes | 99.0% | 99.0% |
| **dynamic accesses with branch-varying address** | 2.6% | 2.6% |
| positions with branch-varying address | 2.6% | 2.6% |
| varying: loads from memory the region never writes (dyn) | 197,116 | 197,116 |
| varying: loads from memory the region writes (dyn) | 256 | 0 |
| varying: stores (dyn) | 0 | 0 |
| lookup size of varying positions: median / mean / max (distinct addresses) | 3 / 3.3 / 5 | 3 / 3.3 / 5 |
| lookup size histogram (positions) | <=2^1: 32, <=2^2: 739, <=2^3: 2 | <=2^1: 31, <=2^2: 739, <=2^3: 2 |
| sum over positions of lookup size (8-byte words) | 31,580 | 31,580 |
| naive sum: positions x words the region touches | 38,469,090 | 38,470,380 |
| lookup sum / naive sum | 0.0008 | 0.0008 |
| lookup sum / positions (mean words per access) | 1.06 | 1.06 |
| varying positions by region | sqlite_heap: 773 | sqlite_heap: 772 |
| memory-routine calls with varying address / length (positions) | 0 / 0 of 20; bytes 0 of 111,672 at varying addresses | 0 / 0 of 20; bytes 0 of 111,672 at varying addresses |

| bytes | total | in SQLite heap | stack | other |
|---|---|---|---|---|
| touched by any branch | 6,557 | 6,354 | 140 | 63 |
| touched by every branch | 6,483 | 6,291 | 140 | 52 |
| written by any branch | 942 | 802 | 140 | 0 |
| final value differs between branches | 12 | 6 | 6 | 0 |

Context: SQLite heap in use 331,008 bytes; process mapped 74,555,392 bytes (includes the 64 MiB arena); VmRSS 5320 kB. Words touched by any branch: 1,290.

Top functions (source function after inlining) by dynamic varying-address accesses, alignment A: sqlite3VdbeExec 197,116, createAggContext 256

Top functions by summed lookup size, alignment A: sqlite3VdbeExec 2,530, createAggContext 2

### N = 1024, `arith`: `SELECT (a * 7 + b) % 13 = 5 FROM t WHERE id = ?1`

Results over branches: {'0': 949, '1': 75}

n4: steps per branch

| measure | min | median | mean | max | max/mean | max/median | histogram (12 equal bins, min..max) |
|---|---|---|---|---|---|---|---|
| basic blocks | 567 | 601 | 600 | 627 | 1.05 | 1.04 | 9 26 50 82 91 142 159 168 158 107 21 11 |
| LLVM IR instructions | 3,694 | 3,875 | 3,871 | 4,016 | 1.04 | 1.04 | 9 20 57 68 89 139 163 177 158 111 19 14 |
| x86 user instructions (plain build) | 2,845 | 2,968 | 2,967 | 3,062 | 1.03 | 1.03 | 9 17 56 59 102 109 187 142 188 101 40 14 |

Control flow: identical for the first 201 blocks (35.4% of the shortest run); 1024 distinct block sequences over 1024 branches.

n5: memory accesses

Per branch: loads median 624 (max 642), stores median 214 (max 215), memory-routine calls median 9.

| quantity | alignment A (site, k) | alignment B (call path, site, k) |
|---|---|---|
| aligned access positions (scalar) | 868 | 868 |
| dynamic scalar accesses, all branches | 844,579 | 844,579 |
| dynamic accesses at positions every branch executes | 93.8% | 93.8% |
| **dynamic accesses with branch-varying address** | 11.4% | 11.4% |
| positions with branch-varying address | 13.6% | 13.6% |
| varying: loads from memory the region never writes (dyn) | 92,408 | 92,408 |
| varying: loads from memory the region writes (dyn) | 2,048 | 2,048 |
| varying: stores (dyn) | 2,048 | 2,048 |
| lookup size of varying positions: median / mean / max (distinct addresses) | 81 / 200.0 / 1024 | 81 / 200.0 / 1024 |
| lookup size histogram (positions) | <=2^1: 1, <=2^2: 6, <=2^3: 4, <=2^4: 7, <=2^5: 12, <=2^6: 5, <=2^7: 47, <=2^8: 15, <=2^9: 7, <=2^10: 14 | <=2^1: 1, <=2^2: 6, <=2^3: 4, <=2^4: 7, <=2^5: 12, <=2^6: 5, <=2^7: 47, <=2^8: 15, <=2^9: 7, <=2^10: 14 |
| sum over positions of lookup size (8-byte words) | 22,168 | 22,168 |
| naive sum: positions x words the region touches | 4,297,468 | 4,297,468 |
| lookup sum / naive sum | 0.0052 | 0.0052 |
| lookup sum / positions (mean words per access) | 25.54 | 25.54 |
| varying positions by region | sqlite_heap: 116, driver_trace:r--p: 2 | sqlite_heap: 116, driver_trace:r--p: 2 |
| memory-routine calls with varying address / length (positions) | 0 / 0 of 13; bytes 0 of 195,584 at varying addresses | 0 / 0 of 13; bytes 0 of 195,584 at varying addresses |

| bytes | total | in SQLite heap | stack | other |
|---|---|---|---|---|
| touched by any branch | 26,603 | 26,519 | 44 | 40 |
| touched by every branch | 1,512 | 1,435 | 44 | 33 |
| written by any branch | 1,328 | 1,284 | 44 | 0 |
| final value differs between branches | 57 | 46 | 11 | 0 |

Context: SQLite heap in use 1,174,784 bytes; process mapped 74,555,392 bytes (includes the 64 MiB arena); VmRSS 6356 kB. Words touched by any branch: 4,951.

Top functions (source function after inlining) by dynamic varying-address accesses, alignment A: sqlite3BtreeTableMoveto 34,253, sqlite3VarintValue 16,685, sqlite3VdbeExec 8,974, btreeParseCell 6,144, btreeParseCellPtr 4,773, sqlite3PcacheRelease 4,096, sqlite3PcacheFetchFinish 4,096, sqlite3Get4byte 4,072, pcache1FetchNoMutex 3,072, sqlite3VdbeOneByteSerialTypeLen 2,048

Top functions by summed lookup size, alignment A: sqlite3VdbeExec 7,952, sqlite3BtreeTableMoveto 5,242, btreeParseCellPtr 3,830, btreeParseCell 2,372, sqlite3VarintValue 2,242, sqlite3PcacheRelease 324, sqlite3PcacheFetchFinish 324, sqlite3Get4byte 321, pcache1FetchNoMutex 243, moveToChild 162

### N = 1024, `like_name`: `SELECT name LIKE '%qu%' FROM t WHERE id = ?1`

Results over branches: {'0': 1006, '1': 18}

n4: steps per branch

| measure | min | median | mean | max | max/mean | max/median | histogram (12 equal bins, min..max) |
|---|---|---|---|---|---|---|---|
| basic blocks | 548 | 588 | 589 | 648 | 1.10 | 1.10 | 17 56 109 155 243 172 119 80 44 15 8 6 |
| LLVM IR instructions | 3,468 | 3,677 | 3,684 | 4,006 | 1.09 | 1.09 | 22 54 111 162 231 177 125 77 38 13 10 4 |
| x86 user instructions (plain build) | 2,748 | 2,921 | 2,951 | 3,406 | 1.15 | 1.17 | 41 153 276 156 173 89 58 37 20 11 5 5 |

Control flow: identical for the first 191 blocks (34.9% of the shortest run); 1024 distinct block sequences over 1024 branches.

n5: memory accesses

Per branch: loads median 599 (max 642), stores median 221 (max 230), memory-routine calls median 11.

| quantity | alignment A (site, k) | alignment B (call path, site, k) |
|---|---|---|
| aligned access positions (scalar) | 879 | 889 |
| dynamic scalar accesses, all branches | 822,059 | 822,059 |
| dynamic accesses at positions every branch executes | 94.9% | 94.9% |
| **dynamic accesses with branch-varying address** | 12.1% | 12.0% |
| positions with branch-varying address | 14.9% | 14.4% |
| varying: loads from memory the region never writes (dyn) | 90,541 | 90,293 |
| varying: loads from memory the region writes (dyn) | 4,520 | 4,520 |
| varying: stores (dyn) | 4,096 | 4,096 |
| lookup size of varying positions: median / mean / max (distinct addresses) | 81 / 162.0 / 1024 | 81 / 165.7 / 1024 |
| lookup size histogram (positions) | <=2^1: 3, <=2^2: 6, <=2^3: 4, <=2^4: 9, <=2^5: 17, <=2^6: 13, <=2^7: 47, <=2^8: 14, <=2^9: 7, <=2^10: 11 | <=2^2: 6, <=2^3: 4, <=2^4: 9, <=2^5: 17, <=2^6: 13, <=2^7: 47, <=2^8: 14, <=2^9: 7, <=2^10: 11 |
| sum over positions of lookup size (8-byte words) | 19,426 | 19,436 |
| naive sum: positions x words the region touches | 6,789,396 | 6,866,636 |
| lookup sum / naive sum | 0.0029 | 0.0028 |
| lookup sum / positions (mean words per access) | 22.10 | 21.86 |
| varying positions by region | sqlite_heap: 124, driver_trace:r--p: 7 | sqlite_heap: 121, driver_trace:r--p: 7 |
| memory-routine calls with varying address / length (positions) | 5 / 7 of 27; bytes 29,738 of 268,030 at varying addresses | 5 / 7 of 27; bytes 29,738 of 268,030 at varying addresses |

| bytes | total | in SQLite heap | stack | other |
|---|---|---|---|---|
| touched by any branch | 46,019 | 45,835 | 59 | 125 |
| touched by every branch | 1,524 | 1,416 | 51 | 57 |
| written by any branch | 1,373 | 1,314 | 59 | 0 |
| final value differs between branches | 82 | 70 | 12 | 0 |

Context: SQLite heap in use 1,174,784 bytes; process mapped 74,555,392 bytes (includes the 64 MiB arena); VmRSS 6184 kB. Words touched by any branch: 7,724.

Top functions (source function after inlining) by dynamic varying-address accesses, alignment A: sqlite3BtreeTableMoveto 34,253, sqlite3VarintValue 16,685, sqlite3VdbeExec 7,168, btreeParseCell 6,144, btreeParseCellPtr 4,773, sqlite3PcacheRelease 4,096, sqlite3PcacheFetchFinish 4,096, sqlite3Get4byte 4,072, patternCompare 3,435, sqlite3VdbeOneByteSerialTypeLen 3,072

Top functions by summed lookup size, alignment A: sqlite3BtreeTableMoveto 5,242, sqlite3VdbeExec 5,194, btreeParseCellPtr 3,830, btreeParseCell 2,372, sqlite3VarintValue 2,242, patternCompare 340, sqlite3PcacheRelease 324, sqlite3PcacheFetchFinish 324, sqlite3Get4byte 321, pcache1FetchNoMutex 243

### N = 1024, `like_note`: `SELECT note LIKE '%zq%' FROM t WHERE id = ?1`

Results over branches: {'0': 802, '1': 222}

n4: steps per branch

| measure | min | median | mean | max | max/mean | max/median | histogram (12 equal bins, min..max) |
|---|---|---|---|---|---|---|---|
| basic blocks | 557 | 672 | 695 | 3,378 | 4.86 | 5.03 | 947 57 9 4 1 2 1 0 1 0 1 1 |
| LLVM IR instructions | 3,518 | 4,120 | 4,255 | 18,738 | 4.40 | 4.55 | 951 53 8 5 1 2 1 0 1 0 1 1 |
| x86 user instructions (plain build) | 2,789 | 3,696 | 3,904 | 29,005 | 7.43 | 7.85 | 978 31 6 2 1 2 1 0 1 0 1 1 |

Control flow: identical for the first 191 blocks (34.3% of the shortest run); 1024 distinct block sequences over 1024 branches.

n5: memory accesses

Per branch: loads median 656 (max 2,637), stores median 237 (max 655), memory-routine calls median 16.

| quantity | alignment A (site, k) | alignment B (call path, site, k) |
|---|---|---|
| aligned access positions (scalar) | 3,020 | 3,190 |
| dynamic scalar accesses, all branches | 905,931 | 905,931 |
| dynamic accesses at positions every branch executes | 82.1% | 77.7% |
| **dynamic accesses with branch-varying address** | 15.1% | 13.6% |
| positions with branch-varying address | 23.0% | 20.9% |
| varying: loads from memory the region never writes (dyn) | 110,404 | 100,577 |
| varying: loads from memory the region writes (dyn) | 19,814 | 17,783 |
| varying: stores (dyn) | 6,623 | 4,592 |
| lookup size of varying positions: median / mean / max (distinct addresses) | 6 / 49.8 / 1024 | 7 / 52.0 / 1024 |
| lookup size histogram (positions) | <=2^1: 159, <=2^2: 130, <=2^3: 116, <=2^4: 73, <=2^5: 68, <=2^6: 38, <=2^7: 52, <=2^8: 30, <=2^9: 16, <=2^10: 14 | <=2^1: 127, <=2^2: 130, <=2^3: 116, <=2^4: 73, <=2^5: 70, <=2^6: 38, <=2^7: 52, <=2^8: 30, <=2^9: 16, <=2^10: 14 |
| sum over positions of lookup size (8-byte words) | 29,386 | 29,596 |
| naive sum: positions x words the region touches | 201,026,300 | 212,342,350 |
| lookup sum / naive sum | 0.0001 | 0.0001 |
| lookup sum / positions (mean words per access) | 9.73 | 9.28 |
| varying positions by region | sqlite_heap: 532, driver_trace:r--p: 159, anon:rw-p: 5 | sqlite_heap: 502, driver_trace:r--p: 159, anon:rw-p: 5 |
| memory-routine calls with varying address / length (positions) | 164 / 163 of 401; bytes 1,159,330 of 1,381,025 at varying addresses | 164 / 163 of 401; bytes 1,159,330 of 1,381,025 at varying addresses |

| bytes | total | in SQLite heap | stack | other |
|---|---|---|---|---|
| touched by any branch | 521,647 | 521,302 | 71 | 274 |
| touched by every branch | 1,498 | 1,392 | 51 | 55 |
| written by any branch | 22,048 | 21,949 | 71 | 28 |
| final value differs between branches | 19,769 | 19,747 | 22 | 0 |

Context: SQLite heap in use 1,174,784 bytes; process mapped 74,555,392 bytes (includes the 64 MiB arena); VmRSS 6240 kB. Words touched by any branch: 66,565.

Top functions (source function after inlining) by dynamic varying-address accesses, alignment A: sqlite3BtreeTableMoveto 34,253, patternCompare 28,000, sqlite3VarintValue 16,685, sqlite3PcacheRelease 12,420, sqlite3VdbeExec 8,126, btreeParseCell 6,144, btreeParseCellPtr 4,773, sqlite3Get4byte 4,404, sqlite3PcacheFetchFinish 4,296, pcache1FetchNoMutex 3,222

Top functions by summed lookup size, alignment A: patternCompare 8,737, sqlite3VdbeExec 6,870, sqlite3BtreeTableMoveto 5,242, btreeParseCellPtr 3,830, btreeParseCell 2,372, sqlite3VarintValue 2,242, sqlite3GetVarint32 1,833, sqlite3PcacheRelease 668, sqlite3Get4byte 653, sqlite3PcacheFetchFinish 524

### N = 1024, `scan`: `SELECT count(*) FROM t WHERE a = ?1`

Results over branches: {'1': 355, '3': 61, '0': 382, '2': 211, '4': 12, '6': 1, '5': 2}

n4: steps per branch

| measure | min | median | mean | max | max/mean | max/median | histogram (12 equal bins, min..max) |
|---|---|---|---|---|---|---|---|
| basic blocks | 62,851 | 63,388 | 63,386 | 63,927 | 1.01 | 1.01 | 90 73 92 89 92 79 85 78 86 100 102 58 |
| LLVM IR instructions | 500,663 | 501,832 | 501,813 | 503,009 | 1.00 | 1.00 | 64 80 90 107 88 85 102 84 104 109 81 30 |
| x86 user instructions (plain build) | 352,410 | 353,036 | 353,034 | 353,696 | 1.00 | 1.00 | 53 65 100 101 106 99 101 111 110 91 72 15 |

Control flow: identical for the first 249 blocks (0.4% of the shortest run); 881 distinct block sequences over 1024 branches.

n5: memory accesses

Per branch: loads median 84,989 (max 85,054), stores median 32,755 (max 32,765), memory-routine calls median 14.

| quantity | alignment A (site, k) | alignment B (call path, site, k) |
|---|---|---|
| aligned access positions (scalar) | 118,834 | 118,835 |
| dynamic scalar accesses, all branches | 120,538,680 | 120,538,680 |
| dynamic accesses at positions every branch executes | 99.1% | 99.1% |
| **dynamic accesses with branch-varying address** | 2.6% | 2.6% |
| positions with branch-varying address | 2.6% | 2.6% |
| varying: loads from memory the region never writes (dyn) | 3,147,775 | 3,147,775 |
| varying: loads from memory the region writes (dyn) | 1,024 | 0 |
| varying: stores (dyn) | 0 | 0 |
| lookup size of varying positions: median / mean / max (distinct addresses) | 3 / 3.3 / 6 | 3 / 3.3 / 6 |
| lookup size histogram (positions) | <=2^1: 64, <=2^2: 3011, <=2^3: 4 | <=2^1: 63, <=2^2: 3011, <=2^3: 4 |
| sum over positions of lookup size (8-byte words) | 125,927 | 125,927 |
| naive sum: positions x words the region touches | 549,131,914 | 549,136,535 |
| lookup sum / naive sum | 0.0002 | 0.0002 |
| lookup sum / positions (mean words per access) | 1.06 | 1.06 |
| varying positions by region | sqlite_heap: 3079 | sqlite_heap: 3078 |
| memory-routine calls with varying address / length (positions) | 0 / 0 of 20; bytes 0 of 446,480 at varying addresses | 0 / 0 of 20; bytes 0 of 446,480 at varying addresses |

| bytes | total | in SQLite heap | stack | other |
|---|---|---|---|---|
| touched by any branch | 22,877 | 22,673 | 140 | 64 |
| touched by every branch | 22,803 | 22,610 | 140 | 53 |
| written by any branch | 1,438 | 1,298 | 140 | 0 |
| final value differs between branches | 14 | 8 | 6 | 0 |

Context: SQLite heap in use 1,174,784 bytes; process mapped 74,555,392 bytes (includes the 64 MiB arena); VmRSS 6228 kB. Words touched by any branch: 4,621.

Top functions (source function after inlining) by dynamic varying-address accesses, alignment A: sqlite3VdbeExec 3,147,775, createAggContext 1,024

Top functions by summed lookup size, alignment A: sqlite3VdbeExec 10,170, createAggContext 2

