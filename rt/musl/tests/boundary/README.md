# Targeted boundary checks

Run `python3 rt/musl/boundary.py` after building musl, the front and the CLI. This separate integration suite uses
a 16 KiB arena and small C programs; it does not run a C corpus or extend the fast Rust/front test suites.

The cases cover console writes and vector counts, zero lengths, invalid/closed descriptors, empty stdin,
two-ended arena growth and exact collision, overflow and exhaustion, non-reclaiming munmap, page-aligned discard,
signal masks and default dispositions, pid/tid registration, startup priority and exit order, stdio flushing,
nested and constructor exits, immediate and raw syscall exits, and the three abort signal calls. A 40-handler atexit case checks allocation beyond musl's first static block; malloc without free checks the lite bump allocator. Unsupported
calls and the provisional stderr behavior must fault as `unreachable`.

Startup/exit ordering is compared with native stdout/status. Device-specific choices use explicit expectations:
empty stdin, pid/tid 1, monotone munmap and no short writes from the unbounded console. The arena test uses raw
SYS_mprotect for its alignment check because upstream musl's public wrapper rounds addresses itself. It uses raw
SYS_mremap to check the ENOMEM choice separately from the public-wrapper failure in pending/mremap-exit.c.

The status cases preserve the full 32-bit entry-return result according to the device exit decision; native
process statuses would expose only the low eight bits. Their native mismatch is not classified as a bug here.
See `../pending/README.md` for the opt-in wrong-result witnesses.
