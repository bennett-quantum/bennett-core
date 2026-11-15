# Pending runtime boundary regressions

These are reduced wrong-result witnesses against the first musl runtime boundary. They are deliberately excluded
from `rt/musl/test.py`, `cargo test`, `front/test.sh` and the default boundary run. They assert correct behavior,
so their failures stay visible when explicitly selected; they are not permanent expected-failure assertions.

After building musl, the front and the CLI, run one or all with:

```sh
python3 rt/musl/boundary.py --case writev-zero-bad-fd
python3 rt/musl/boundary.py --include-pending
```

The runner compiles each witness natively with clang/glibc as an independent reference, checks its native result
is zero with empty output, then compares device status **and console bytes**. In particular, an error returned
by `writev` does not excuse the unexpected `X` already emitted.

| ID | Test | Expected | Initial device behavior |
|---|---|---|---|
| RTM001 | `writev-zero-bad-fd` | Empty writev on invalid fd gives EBADF | Succeeds |
| RTM002 | `lseek-closed-fd` | Closed fd gives EBADF | ESPIPE |
| RTM003 | `ioctl-bad-fd` | Invalid fd gives EBADF | ENOTTY |
| RTM004 | `sigaction-invalid-signal` | Signal zero gives EINVAL | Succeeds |
| RTM005 | `madvise-partial-page` | DONTNEED discards a whole page | Only the first byte is zeroed |
| RTM006 | `writev-overflow-effects` | Invalid later vector gives EINVAL without output | Emits X, then gives EINVAL |
| RTM007 | `mremap-exit` | Ignore a failing mremap, return normally | bad_indirect fault |
| RTM008 | `brk-regrow` | Shrink/re-growth returns zero-filled pages | Stale byte remains |

RTM007 does not demand that mremap succeed: the bounded backend deliberately returns ENOMEM. RTM008 uses raw brk
before any allocation by the program and checks all break changes; it does not exercise the documented munmap
non-reclamation choice. The no-shrink control preserves its written byte on both systems.
