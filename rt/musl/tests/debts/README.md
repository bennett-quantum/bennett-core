# Named allocation coverage debt

RTMD001, `malloc-free-guard.c`, does `malloc(1); free(p); return 0`. Native exits normally; the initial Bennett
runtime faults `unreachable`. Referencing free pulls musl's full mallocng allocator instead of the lite bump
allocator. Its metadata setup requests a fixed PROT_NONE guard mapping, an operation the current runtime explicitly
refuses. This is a named coverage gap, separately counted from the wrong-result regressions in `../pending`.

```sh
python3 rt/musl/boundary.py --case malloc-free-guard
python3 rt/musl/boundary.py --include-debts
```

The default run excludes this debt. The route to remove it is to provide the allocation/protection operations
unmodified mallocng needs, preserving explicit faults for operations outside that support. malloc without free
and more than 32 atexit registrations are passing controls in the default boundary suite.
