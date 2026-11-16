# Carried runtime models

`vfs.c`, `vfs.h`, `devices.c` (from `src/musl/syscall.c`) and `models.c` are adapted from
[bennett-runtimes](https://github.com/bennett-quantum/bennett-runtimes), revision
`9268e696ef8709e87bd861daadaf678b8f5ae64e`. The archive is not modified.
The carried code is AGPL-3.0; its licence is retained in `CARRIED-LICENSE`.

Adaptations: core's effects/exit and existing arena/signals; empty stdin and explicit stderr fault;
clock start and entropy image as explicit inputs; no unbound models or unrequested block backend;
whole-vector preflight; checked allocations and named faults for unsupported flag/locking operations.
