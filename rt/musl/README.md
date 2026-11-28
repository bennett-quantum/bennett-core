# musl 1.2.5 for Bennett

The upstream release is vendored unmodified; its licence and archive identity are in `VENDOR.md`.
The three overlay headers replace x86-64 system-call assembly, atomics and the thread pointer for the
single-threaded device. No network is used by the build.

Build the library's bitcode archive members with LLVM/clang 18, GNU make and Python 3:

```sh
python3 rt/musl/build.py                  # target/musl; an output directory may be supplied
```

The build configures musl out of tree, uses its own `LIBC_OBJS` selection, and substitutes its generic C
files for x86-64 assembly wherever present. It produces 1,345 bitcode members, a member list, `libc.a`,
installed public headers and JSON compiler flags. Its fingerprint includes all upstream files, the overlays,
the build script and compiler flags; unchanged builds reuse the members.

Compile, resolve the archive members, and lower one C program:

```sh
python3 rt/musl/link.py program.c -o program.bn
cargo run -p bennett -- run program.bn --entry __bennett_entry --budget 100000000
```

`link.py` uses musl's public headers, preserves the program's linkage, and pulls archive members as the
probe's static-link experiment does (`resolve.py`). It resolves undefined weak references to null except
the standard init/fini bounds supplied by the front, and retries library dependencies introduced by LLVM
(such as `puts` from `printf`). It never merges the entire library or internalizes the whole program.
`--clang-flag=FLAG` passes a compiler option; `--libdir`, `--front`, `--stack-bytes`, `--region-budget`
and `--arena-bytes` select build products and limits. Intermediate files are temporary unless `--keep DIR`
is supplied. The supplied program needs a defined `int main` with zero to three ordinary C parameters.

`start.c` gives musl's unmodified `__libc_start_main` a correctly typed adapter to `main`, the run's
arguments, an empty environment and a page-size auxiliary vector. The arguments are a run input
(`--arg`, `--argv0`; below), `argc=1` and `argv={"prog",0}` without them. Musl initializes its own thread
and TLS data, walks the front's constructor arrays, calls main, and runs its own `exit`: atexit handlers,
destructors and stdio flushing. Bennett supplies only the thread-pointer bridge and system-call boundary;
there is no probe hash oracle or hand-made exit sequence.

## The boundary

The device has empty stdin and one console. Reads of fd 0 (`read`, `readv`) report end of file.
Writes/writev to fd 1 append bytes with `__bennett_effect`; a vector is checked whole before any byte is
written. **A write of one or more bytes to fd 2 faults as `unreachable`**: stderr cannot disappear silently,
and cannot be mixed into the stdout-only comparison. Closing a standard fd is remembered; closed and
unsupported descriptors fail with `EBADF`. The console is not a terminal (`ioctl` gives `ENOTTY`), and
seeking it gives `ESPIPE`.

`exit`/`exit_group` finish through `__bennett_exit`, including from nested calls. The arena defaults to 1 MiB;
`--arena-bytes` is a link parameter in whole 4096-byte pages. `brk` grows from the bottom and anonymous
private mappings from the top; without `MAP_FIXED` an address is only a hint. **Exhaustion is the named fault
`out_of_memory`**, never `ENOMEM` or a null result: a growth of the break or a fresh mapping the arena cannot
hold (one larger than the arena too) calls `__bennett_out_of_memory`, which ends the run with the console as
written so far (stdio unflushed, as for every fault). Musl's arithmetic rejections before any system call
(`malloc(SIZE_MAX)`, an overflowing `calloc`, `mmap` of `PTRDIFF_MAX` bytes or more, the public `brk` and
`sbrk(n != 0)`, which musl never implements) stay null or `ENOMEM`, as natively (`docs/DESIGN.md`, "Files,
clock and entropy at the musl boundary"). `mremap` fails with `ENOMEM` (musl's `realloc` then copies); a `MAP_FIXED` mapping longer than
the arena fails with `ENOMEM`. The arena is monotone: `munmap` accepts valid
ranges but does not reclaim storage; use after release is outside the C allocation lifetime. A `MAP_FIXED`
anonymous mapping is accepted over arena pages already handed out and gives fresh zero pages there (musl's
`malloc` maps its guard page this way); elsewhere it traps. Pages the break gives back read as zero when it
grows over them again, and `MADV_DONTNEED` zeros whole pages; the hints `MADV_NORMAL`, `RANDOM`,
`SEQUENTIAL` and `WILLNEED` are accepted, other advice traps. Protections `PROT_NONE`, `PROT_READ` and
read/write are accepted by `mmap` and `mprotect` but not enforced: a program that passes natively never
touches memory it may not, and one that does would fault natively where the device goes on. Executable,
file-backed and shared mappings trap. This is a bounded allocation backend, not a host virtual-memory
implementation.

Signals are the ones a program sends itself (`raise`, `kill` or `tkill` of process and thread 1); there are
no asynchronous signals. They are delivered as the kernel delivers them, before the system call that raises
or unblocks them returns: dispositions, the signal mask, the pending set (`sigpending`) and installed
handlers, with `SA_SIGINFO` (a `siginfo_t` with the signal number and `SI_TKILL`; the context is zero),
`SA_RESETHAND`, `SA_NODEFER` and the handler's mask. A default action terminates with status 128 plus the
signal number without flushing stdio (`abort` gives 134), or ignores `SIGCHLD`, `SIGCONT`, `SIGURG` and
`SIGWINCH`; stop signals trap. A second real-time signal raised while one is pending traps (it would queue).
Hardware signals have no counterpart: a division by zero is the device's named fault, not `SIGFPE`.
Process and thread ids are 1. Every unimplemented system call traps as `unreachable`.
Threads and dynamic loading are not implemented. No unsupported operation is turned into a
successful no-op with an observable result.

### Structures in system calls

A system call reads and writes the program's memory as Linux does: it copies each input structure whole into
its own memory before it checks any value, checks only the copy, and writes each output once the result is
known, after its inputs are read and checked; no entry reads a field of the program's structure in place. So
an output that is also an input (`sigaction(s, &a, &a)`) gets the old value after the new one is read, as
natively. Two helpers in `copy.h` do every copy, `__bennett_copy_in` and `__bennett_copy_out`. Which bytes can
be read: device memory is the module's bytes from 0 to its memory size (static data from address 16, the stack
down from the top), a native program's address space is another, so the two agree only for live objects, null
and the lowest addresses, and ranges Linux refuses whatever is mapped. Linux checks a range in two stages, and so
does the device:

- The range, where the call checks it (Linux's `access_ok`): a range that wraps or ends above the top of the
  user address space, 2^47 − 4096 on x86-64 with four-level page tables, is `EFAULT` before any byte is touched;
  an empty range at an address above the top too. Device memory lies far below the top.
- The bytes, as they are touched: a nonempty range starting below address 16 faults at its first byte, so the
  call answers `EFAULT` with nothing transferred. Any other byte is read or written; one at the end of device
  memory or beyond stops the run there with the device's `out_of_bounds` (natively `EFAULT`, or a short
  transfer), since the runtime does not know the memory size (route: below, "Sleeping").

A structure copy checks both stages, then copies every byte; a copy of no bytes always succeeds. A data buffer
(`read`, `write`, `pread`, `pwrite`, the elements of `readv` and `writev`, `getrandom`) has its range checked
where Linux checks it and its first byte only where bytes are transferred: none at the end of a file, for a
directory (`EISDIR`), for `/dev/null` or for empty stdin, so a buffer below 16 is no error there, as natively.
One call transfers at most `MAX_RW_COUNT` bytes (0x7ffff000, Linux's `INT_MAX` rounded down to a page); a larger
count is capped. `getcwd` writes its result as a structure copy.

Each call's order is Linux's: the arguments it checks first, its inputs copied in, the values in them, the
change, the outputs written.

- `clock_gettime`, `clock_getres`, `gettimeofday` (the time value, then the zone) and `time`: the clock id,
  then the result written. A refused output of `clock_gettime`, of `gettimeofday`'s time value or of `time` is
  `EFAULT` after the clock was read, so that read is consumed; `clock_getres` and a time-zone-only
  `gettimeofday` read no clock.
- `read`, `write`, `pread`, `pwrite`: a negative position, the descriptor, a seekable file, the access mode,
  the buffer's range (a count above `LONG_MAX` never passes it), the position plus the whole count beyond
  `LONG_MAX` (`EINVAL`, Linux's `rw_verify_area`, whatever the file; `read` and `write` use the file's own
  position), the count capped, then the file's own answer and the bytes.
- `getrandom`: the flags, the length capped, its range, then the bytes.
- `stat`, `lstat`, `fstat`, `newfstatat`: the descriptor or path, then the whole `struct stat` written.
- `readv`, `writev`: the descriptor and its access mode (`EBADF`), a count of zero (0), a count above 1024
  (`EINVAL`), the whole array's range, then each element copied whole and its length checked as it is copied
  (a length above `LONG_MAX` is `EINVAL`, also before a later element that cannot be read, as natively), then
  every element's range before any byte moves, each length capped so that the total stays within
  `MAX_RW_COUNT`, the file's position plus that total beyond `LONG_MAX` (`EINVAL`), then the transfers from
  the copies. As Linux imports one element as a single buffer, its
  length is capped before its range is checked; several are each checked at their full length, then capped.
  The copies live in the call's own frame, sized for its count. A bad element after a good one ends a partial
  transfer, except on the console, whose elements' first bytes are checked before any output.
- `rt_sigaction`: the set size, the new action copied, the signal number (and `SIGKILL`, `SIGSTOP` for a new
  action), the change (a stored action's mask without `SIGKILL` and `SIGSTOP`, as Linux stores it), the old
  action written.
- `rt_sigprocmask`: the set size, the new set copied, `how`, the change, the old set written, then delivery
  of what the change unblocked.
- `rt_sigpending`: a set size above 8 is `EINVAL`; the first that many bytes of the pending set are written
  (none for 0, whatever the address).
- `nanosleep`, `clock_nanosleep`: "Sleeping", below.
- The path calls (`open`, `openat`, `creat`, `stat`, `lstat`, `newfstatat`, `access`, `faccessat`, `readlink`,
  `readlinkat`, `truncate`, `chdir`, `chmod`, `mkdir`, `mkdirat`, `rmdir`, `unlink`, `unlinkat`, `rename`): the
  arguments Linux checks first (`open`'s flags, `truncate`'s negative length, the access mode, `readlink`'s
  size), then each path string copied whole (`__bennett_copy_path`, Linux's `getname`), then the directory
  descriptor and the walk. The copy reads the bytes one by one up to the first NUL: a string starting below 16,
  at the top of the user address space or above it, or reaching the top before its NUL is `EFAULT`; `PATH_MAX`
  (4096) bytes without a NUL are `ENAMETOOLONG`, whatever follows; an empty string is `ENOENT`. A byte at the end
  of device memory stops the run with `out_of_bounds`, as for a structure; one whose NUL is the last byte of
  memory is read up to it. `rename` reports the old name's error, then its walk's, then the new name's, as
  Linux's `do_renameat2`; Linux copies both names first, the device the new one after the old walk, so that a
  new name past device memory stops the run only where Linux's answer is its `EFAULT`.

## Files and deterministic inputs

The carried archive models run as C on the device, with no host IO. File bytes, sparse 4096-byte
pages, names, inodes, descriptor flags, shared open-description offsets, cwd and umask are ordinary
journalled memory (`CARRIED.md` gives provenance). Regular files support open/openat/creat,
read/write/readv/writev, pread/pwrite, seek, dup/dup2/dup3, stat, descriptor/status fcntl flags,
truncate/ftruncate, readlink/readlinkat errors over the current non-symlink namespace, mkdir/rmdir, chdir/fchdir/getcwd, rename/unlink, chmod/umask and sync.
The initial namespace has `/`, `/tmp` (mode 01777), `/dev`, `/dev/random`, `/dev/urandom`, `/dev/null`, `/dev/zero` and
`/dev/console`. Duplication preserves the explicit stderr fault; fd 2 may be redirected to a file.

Errors follow Linux's order of checks where several apply (arguments before the path walk, earlier
components before later ones, a last component `.`, `..` or the root refused by kind before lookup), with
`PATH_MAX` and `NAME_MAX` enforced. The working directory and directory descriptors are inodes: they follow
renames, and once removed they name nothing (relative names then give `ENOENT`; Linux would still find `.`
and `..` there, a recorded debt). `O_ACCMODE` opens for neither reading nor writing, directories accept
`fsync`, and `SEEK_DATA`/`SEEK_HOLE` report the model's unwritten 4096-byte pages as holes, as ext4 and tmpfs
with 4 KiB blocks do. A vector with a bad element after a good one transfers the good part, except on the
console, which is checked whole ("Structures in system calls"). A directory descriptor of the `*at` calls is
an `int`, as in Linux, so `AT_FDCWD` passed with its upper 32 bits clear is still the working directory. The CPU-time clocks of the caller (`clock_getcpuclockid`,
`pthread_getcpuclockid`) share the clock's counter.

Supply the run's inputs when constructing its image:

```sh
python3 rt/musl/link.py program.c -o program.bn --clock-start 1234999999998 \
  --entropy-image entropy.bin --file-image files.json -- 100000 -v ''
bennett run program.bn --entry __bennett_entry --reverse --stats
```

The strings after the first `--` are the arguments of `main`, in order, from `argv[1]` (one may be `--` or
look like an option); `--argv0` replaces `argv[0]`, `prog` by default. The strings are the host's exact bytes (an empty string, bytes that are
not UTF-8), NUL-terminated and contiguous in writable data of the image, and `argv[argc]` is null. `start.h`
declares the count and the strings' area, which `link.py` defines with the other inputs; `start.c` builds from
them, on its own frame, the one array of pointers musl's start-up reads: `argv`, its null pointer, the empty
environment's null pointer and the auxiliary vector, as on a Linux process's initial stack (`8 * (argc + 8)` bytes
of the stack). Start-up reads nothing but the image. The area's size is a link parameter, `--argument-bytes`
(4096 by default); the strings with their NULs must fit. Because its size does not depend on the arguments and
both values are writable, so that no optimiser takes them into code, a relink with other arguments changes the
image's data only: every function is the same text, so an applied function given the same inputs takes the
same steps in both images. A caller that reads `argc` or `argv` can give it other inputs, and the whole run's
step count can differ, since start-up reads the strings (an empty `main` takes 2,155 steps with `argv[0]`
`prog` and 2,165 with `progg`). Each
argument is one string of `link.py`'s command line, so the host's `execve` limit of 128 KiB per string is the
native run's (`--argv0=S` spends eight bytes of it); the limit on the whole command line (about 2 MiB) also counts
`link.py`'s options and environment, so the longest accepted list is somewhat shorter than natively.

`files.json` maps canonical absolute paths to hex byte strings, for example
`{"/fixtures/input":"610062ff63"}`. Parent directories are created automatically. Installation occurs
before the first device syscall, including those called by constructors, and is itself journalled.
The defaults are no user files, clock start zero, an empty entropy image and no arguments. These are
inputs of the run image; to use another image currently relink. Host paths are read only by the image builder.

A clock read returns the unsigned 64-bit start plus the number of earlier clock reads, in nanoseconds,
wrapping modulo 2^64, plus the nanoseconds slept so far (below) for every clock but the CPU-time ones.
Accepted clock ids share that counter. `clock_gettime`, `gettimeofday` and `time`
consume one read when they request a value; `clock_getres` returns 1 ns without consuming a read.
This is independent of the number of executed instructions. `getrandom`/`getentropy` and both random
devices consume the same exact byte image. It is a seeded input, not a host entropy source or a PRNG.
Exhaustion traps as `unreachable` before writing any bytes, including on a multi-element readv.
Empty reads consume none. Undo restores the clock's counters, entropy's position, destinations and all
filesystem state.

### Sleeping

A sleep advances the run's clock by the requested duration and returns at once. `nanosleep`, `sleep`, `usleep`
and `clock_nanosleep` reach the system calls `nanosleep` and `clock_nanosleep`, which the runtime answers from a
third counter beside the start and the reads: the nanoseconds slept, ordinary memory like the others, so undo
restores it and a sleep inside `apply` is discarded with the applied function's other writes. This extends the
clock's rule without reading host time: a clock read is the start, plus the earlier reads, plus the nanoseconds
slept, modulo 2^64. The clock's unit is the nanosecond, so a duration needs no rounding.

- A relative sleep of `d` nanoseconds adds `d` (modulo 2^64, as reads do); an absolute one (`TIMER_ABSTIME`) on a
  clock whose current value (the next read's, without consuming it) is `v` adds `T - v` for a target `T > v`
  and nothing otherwise, so the next read returns the greater of `v` and `T`. Later reads of `CLOCK_REALTIME`,
  `CLOCK_MONOTONIC`, `CLOCK_BOOTTIME`, `CLOCK_TAI` and the coarse and raw clocks, `gettimeofday` and `time`
  see the advance. The CPU-time clocks (`CLOCK_PROCESS_CPUTIME_ID`, `CLOCK_THREAD_CPUTIME_ID`, the caller's
  ids from `clock_getcpuclockid`, and so `clock()`) do not: natively a process's CPU time does not advance
  while it sleeps.
- A sleep returns 0 (`sleep` returns 0 seconds left). No signal can arrive while the device sleeps: the only
  signals are those a program sends itself, delivered when they are raised or, if blocked then, when the program
  unblocks them, and a sleep does not change the signal mask. So no sleep is interrupted and the remaining time
  is never written, as natively for a sleep that is not interrupted.
- Sleeps on `CLOCK_REALTIME`, `CLOCK_MONOTONIC`, `CLOCK_BOOTTIME` and `CLOCK_TAI` sleep; only the
  `TIMER_ABSTIME` bit of the flags has a meaning. Errors are Linux's, in its order: an unknown clock id is
  `EINVAL`; the thread CPU clock, the raw and coarse clocks and a clock behind a descriptor have no sleep
  (`EOPNOTSUPP`; musl and glibc answer `EINVAL` for `CLOCK_THREAD_CPUTIME_ID` before the call); then a request
  that cannot be read is `EFAULT` (Linux copies the whole request first, below); then a negative second count or
  nanoseconds outside `0 .. 999999999` is `EINVAL`; then a CPU-time id of an unknown kind is `EINVAL`, as on
  Linux, and so is a thread's: Linux refuses a sleep on the calling thread's own CPU-time clock and accepts one
  on another live thread's, and the device has one thread, so every thread id is the caller's or names nothing;
  on the alarm clocks, flags other than `TIMER_ABSTIME` are `EINVAL`, as on a host with a real-time clock
  device (a host without one answers `EOPNOTSUPP` for every sleep on them with a valid request, whatever the
  flags; `tests/sleep-alarm-flags.c` is compared with that answer read as `EINVAL`).
- Which requests can be read: as for every structure ("Structures in system calls", above), the request is
  copied whole before any field is checked; one starting below address 16, wrapping or ending above the top of
  the user address space is `EFAULT`, and one that reaches the end of device memory or beyond (below the top) stops the run in that copy with the device's `out_of_bounds`,
  whatever its fields hold (`tests/sleep-beyond-memory.c`, and `tests/sleep-partial.c` for seconds in memory and
  nanoseconds past its end; natively `EFAULT`): the runtime does not know the memory size, which the front fixes
  after it lays out the data. Route to `EFAULT` there, for every structure: the memory size given to the program
  (a symbol the front defines, as ELF's `_end`, or a device instruction that reads it).
- A CPU-time id of a process other than the device's one is `EINVAL` by the device's own rule: the device has
  one process, so the id names nothing. Linux answers `EINVAL` for a process that does not exist and accepts
  the clock of one that does (a parent's, say).
- On the process's CPU-time clock a sleep with nothing to wait (zero, or a target already reached) returns 0.
  A sleep that would have to wait is refused by name (`unreachable`): natively it never ends, since the
  process's CPU time does not advance while it sleeps.
- Also refused by name: a duration or a target of 2^64 nanoseconds or more (about 584 years, beyond the
  clock's range; natively such a sleep does not end in any run), and a sleep on `CLOCK_REALTIME_ALARM` or
  `CLOCK_BOOTTIME_ALARM` with a valid request and flags (natively `EOPNOTSUPP` without a real-time clock
  device, else `EPERM` or a sleep, by the host's capability to wake the system).

Timers are not implemented: `alarm`, `ualarm`, `setitimer`, `getitimer`, the `timer_*` calls and `timerfd`
stop by name (`unreachable`) at run time, as `pause` and `sigsuspend` do. All but `timer_create` reach the
system call, which is unimplemented; musl's `timer_create` also links its thread notification, whose thread exit
(`__unmapself`) is inline assembly the front does not translate, so `build.py` compiles `timer.c`, that stop, in
place of musl's member (`tests/timer-create.c`). Like musl's, it is pulled from the archive only when a program
calls `timer_create` or takes its address without defining it (an address taken is a reference as a call
is, so such a program links the stop and runs to the end if it never calls it): ISO C does not reserve the
name (`tests/timer-own.c`). With no timer and no asynchronous signal `pause` never returns natively;
`sigsuspend` traps even where natively it would return at once with `EINTR` (a pending signal that its mask
unblocks), a debt of the same route. The route is in `docs/DESIGN.md`, "Files, clock and entropy at the musl
boundary": an expiry is a value of the same clock; a sleep or a clock read that carries the clock past it makes
the timer expire there. A timer that notifies by a signal delivers it at that point through the existing
delivery of self-sent signals; a `timerfd` instead becomes readable, a read giving the number of expirations,
and sends no signal. A relative sleep interrupted by a delivered signal returns `EINTR` and writes the
remaining time; an interrupted absolute sleep leaves that output untouched (the caller retries with the same
target).

Metadata uses root ownership, a model inode/device identity, and zero timestamps. Unsupported
operations and flags trap by name instead of claiming success. The remaining debts and their routes
are in `HANDOFF.md`, including locks, links, directory enumeration, per-invocation image loading,
and permissions/metadata. The existing finite arena bounds the filesystem as well as the application
heap; a file operation that exhausts it ends the run with `out_of_memory`, so no half-made change is seen.

```sh
python3 rt/musl/test-devices.py   # native file/image comparisons, arithmetic clock oracle, tape, reversal
```

## Floating point

Measured before selecting the flags: the existing front refuses musl's `sin.c` built with
`-frounding-math` at `llvm.experimental.constrained.fpext.f64.f32`. With `-fno-rounding-math` it passes
floating-point lowering and waits on the ordinary library dependency `__rem_pio2`. The product build
therefore uses `-fno-rounding-math`. This is a build choice for musl's own files: they compute in whatever
mode is current, as natively, since every floating-point operation reads the floating-point runtime's mode.

`<fenv.h>` is Bennett's (`fenv.c`), compiled by `build.py` in place of musl's `src/fenv/fenv.c` (musl's generic
dummies, built where x86-64 has assembly, reported success for `fesetround(FE_UPWARD)` without changing the
rounding and never saw a flag) and of musl's generic `fesetexceptflag.c` and `feholdexcept.c`. The environment is
the floating-point runtime's state, as x86 has it: two units, SSE (`float`, `double`) and x87 (`long double`),
each with its own rounding mode and six status flags (`rt/softfloat/README.md`, "Global state"); `fenv.c` reaches
it through the runtime's `__bennett_fe*` functions, and musl's own `fesetround`, `feupdateenv` and
`fegetexceptflag` sit on it. Each function does what glibc's does on x86-64, which the native program uses:
`fesetround` sets both modes and `fegetround` reads x87's; `fetestexcept` reads both units' flags together and
`feclearexcept` clears both (x87's denormal-operand flag on every call, as glibc's mask does); `feraiseexcept` raises invalid and divide by zero in SSE and overflow, underflow and
inexact in x87; `fesetexceptflag` sets and clears SSE's flags and only clears x87's; these take the five standard
flags (glibc's `FE_ALL_EXCEPT`; musl's also names the denormal-operand flag, which glibc masks out).
`feholdexcept` saves and clears every flag of both units; `fegetenv` and `fesetenv` carry both halves of
`fenv_t` as `fnstenv`, `stmxcsr` and glibc's `fesetenv` do, the two modes and the denormal-operand flag
included. The rounding modes and the five standard flags are exact; the denormal-operand flag is carried as well
as measured and is outside the guarantee (`rt/softfloat/README.md`, "What is guaranteed"). An environment that
asks for what is not carried is a named fault (the device's `unreachable`: its
faults are a fixed table with no room for a reason): an unmasked exception (a trap), x87 precision other than
extended, or flush-to-zero or denormals-are-zero; so is a reserved MXCSR bit, which faults natively. The members
are pulled from the archive only when a program names one of these functions, so a program that never does
keeps floating point that writes no memory (`front/tests/tests/fenv.rs`).

The C library's functions whose result IEEE 754 defines exactly, `sqrt`, `fma`, `rint`, `nearbyint`, `lrint`,
`llrint`, `round`, `lround`, `llround`, `floor`, `ceil`, `trunc`, `fmin`, `fmax` and their `f` and `l` forms,
are compiled from `math.c` in place of musl's: each is the compiler's builtin under `FENV_ACCESS`, which the
front lowers to the floating-point runtime's routine. glibc computes these as the IEEE operation (an x86
instruction, or exact software), and so does the runtime; musl's software gives the same values but raised other
flags (a spurious inexact from `lround`, `lroundl`, `llroundf` and from exact results of `fmal`, a default NaN
for `fmaf(0, inf, NaN)`) and passed signalling NaNs through unquieted (`nearbyint`, `fma`), measured by
`front/tests/c/fenv_ops.c` against glibc; the other functions are replaced with them for the same reason, unmeasured
one by one against musl's. errno is not set,
as by musl's. `sqrtl` follows glibc's error path too: for a negative operand it raises invalid in SSE, as glibc's computes
its NaN. `ilogbl` is `math.c`'s as well: musl's value with the flags of glibc's, which computes with x87's
`fxtract` and `fistp` (the denormal-operand flag for a subnormal operand, x87's invalid for a NaN) and raises
invalid in SSE for 0, infinity and NaN. Twelve musl math files use `#pragma STDC FENV_ACCESS ON`; those that remain
musl's (`ilogb`, `ilogbf`) run with their constrained operations lowered and agree with glibc. `ilogbl` reads a
long double encoding no arithmetic produces (an unnormal, a pseudo-denormal) at face value, where glibc's x87
code reads it as x87 does. Programs on the device include `include/math.h` before musl's headers
(`link.py`): musl's `<math.h>` with the six quiet comparison macros (`isless`, `isgreater`, ..., `isunordered`)
as the compiler's builtins, as glibc's are. musl's test for a NaN first, so they never raised invalid, also not
for a signalling NaN, where glibc's `ucomisd` does. Neither of the probe's integer-only printf/scanf
patches is applied: every upstream file, including floating-point stdio, is intact.

## On the corpora

Measured on 2026-11-15 at `b961292` and again at `8bbb56c`, with the same outcome for every program
(`experiments/c-corpus/README.md`, "The three corpora at `8bbb56c`" and "at `b961292`"):
on GCC's C torture tests 1,560 of 1,575 runnable programs agree with native on status and stdout, on
c-testsuite 217 of 220, on GCC's `ieee/` 64 of 67; none is wrong and none crashes. What still does not run,
and what removes it:

| refused or faulted | programs | what removes it |
|---|---:|---|
| files: `open`, `fopen`, `freopen` previously trapped by name | 6 | carried file model at this boundary; targeted rerun results below |
| `setjmp`, `_setjmp`, `longjmp`: no definition (musl's are x86-64 assembly, not built) | 2 | removed: the device's own `setjmp` and `longjmp` (`docs/DESIGN.md`, "Non-local jumps"); both agree, run alone |
| `__mulsc3`, `__divsc3`: no definition | 2 GCC | removed: complex helpers (now libgcc, `rt/libgcc`); both agreed at `b961292` |
| musl's `ilogb`, `ilogbl` (and ten other math files) under `#pragma STDC FENV_ACCESS ON`: constrained operations refused by the front | 3 `ieee/` (`cdivchkf` was counted at `__divsc3` before the helpers) | removed (2026-11-25): constrained operations are lowered over the carried floating-point environment; all three agree (`rt/libgcc/exact/corpus.py`) |
| a program's own `int strlen(char *)` against the library's (c-testsuite `00025`) | 1 | a call through a mismatched prototype, a front refusal |
| not library calls: `llvm.returnaddress`, inline assembly, an over-aligned variadic argument, an unprototyped call with an extra argument (`bad_indirect`), a run past the step budget (`float` to `i128` was one; it agrees since the 128-bit conversions) | 7 (9 before `llvm.eh.sjlj.longjmp` became the device's `longjmp`) | the front's own debts (`HANDOFF.md`) |

## Tests and text size

```sh
python3 rt/musl/test.py                   # needs the library build, front, and debug CLI already built
```

These offline integration tests are separate from `cargo test` and `front/test.sh`. They compare native
stdout/status for floating-point printf/strtod/sqrt/sin/pow, for constructors, atexit, destructors and
nested exit, for `malloc`/`realloc`/`calloc`/`free`, for signals raised, blocked, ignored and handled, for
`abort` after a handler returns, for reads of the empty stdin through `scanf`, `getchar` and `fgets`, and for
a break that shrinks and grows again, for the floating-point environment, for run arguments
(`args.c`: none with the default `argv[0]`, empty strings, 200 arguments, bytes that are not ASCII or UTF-8,
strings that look like options or are `--`, another `argv[0]`, writes into the strings and the array;
`budget.c`: a step budget read with `strtoul` and passed on, in range, out of range, malformed, missing, and
the longest argument Linux passes, 131,071 bytes; each run natively with the same `argv` and an empty
environment; relinks of `args-apply.c` and `args.c` with arguments of another count and length give the same
text for every function, and the same steps inside an `apply`; arguments too large for their area are refused), and for complex products,
quotients and `cexp` (`complex.c`, over libgcc's helpers), for sleeping (`sleep.c`: the advance of each
clock seen by later reads, the CPU-time clocks unchanged, absolute targets, and Linux's errors in its order,
also through the raw system call), and for `setjmp`/`longjmp` across frames, `sigsetjmp` with
and without the signal mask (musl's `__sigsetjmp_tail`) and jumps out of signal handlers (`setjmp.c`); they check that heap
exhaustion by `mmap`, `malloc`, `realloc` and the break is the fault `out_of_memory` with the output flushed
before it kept (`arena`, `heap-exhaust`, `realloc-exhaust`, `mmap-exhaust`, `brk-exhaust`), that musl's
arithmetic rejections stay null against native (`alloc-arithmetic`), and named faults for stderr, an
unimplemented syscall, the refused sleeps (`sleep-cpu`, `sleep-beyond`, `sleep-alarm-clock`) and timers
(`timer-alarm`, `timer-create`), and `out_of_bounds` for a sleep request past device memory
(`sleep-beyond-memory`, and `sleep-partial` with only its seconds in memory). For `sleep-partial`, the test
driver reads the device memory size from the program and passes a request address to `partial_entry`.
Sleeps in `sleep`, `sleep-alarm-flags` and `sleep-caller` agree with native. The last runs under an entry that
calls start-up. A program's own `timer_create` (`timer-own`), another rounding mode and a flag test also agree
with native.
`tests/regress/` holds the self-checking witnesses of the independent testing pass of this boundary (each
returns 0 natively and failed or faulted here before its fix). The 35 programs (42 runs with the
`-O2` ones) took 173 seconds with the debug CLI on the shared machine, after building the library.
The run-argument cases and the relinks were measured with the 24 tests before the allocator ones and
before the longest-argument case: 243 s together on the shared machine (2026-11-25).
The debug CLI now reads their large data strings without overflowing its host stack.

The front writes zero runs of 16 bytes or more as one `zero N` data item, so the 1 MiB arena costs a
line, not a megabyte of explicit zeros. Measured at `6da7ef6` with `link.py` and default limits: a `printf`
hello world is 215,785 bytes of device text, and the library example (`tests/library.c`, with
floating-point `printf`, `strtod`, `sqrt`, `sin` and `pow`) 652,468 bytes, of which 26 `zero` items reserve
1,054,135 zero bytes; the same example was about 4.8 MB before `zero N`.

### Files/clock/entropy checks (2026-11-16)

`test-devices.py`: **24 passes** (16 fixture runs at `-O0` and non-vectorized `-O2`, and the 8 witnesses of
`tests/devices-regress/`), 147.31 s on the shared machine.
Files and constructor-loaded byte images agree with native stdout/status. Clock arithmetic and seed
bytes agree with a separate native C arithmetic oracle, including start `2^64-2` wrapping through zero
(since 2026-11-27 `clocks.c` also sleeps: relative, absolute, in the past, and a sleep that wraps the clock,
with the CPU-time clock read between; 43,108 and 43,336 steps at `-O0` for the two starts, 24 runs in 149 s).
`getentropy`, random-device reads and vectored tape consumption additionally check each expected byte.
All forward runs un-run to the identical entry snapshot (including buffers, both consumption counters,
filesystem/allocator state and the lazy image guard); amplitude error zero. The witnesses also un-run exactly
from cuts at a quarter, half and three quarters of their run.

`tests/devices-regress/` holds the self-checking witnesses of the independent testing pass of files, clocks
and entropy: each returns 0 natively (ext4) and failed on the device before its fix. They cover removing or
renaming `.`, `..` and the root (`rmdir(".")` removed the directory; `rename(d, "/")` replaced the root),
trailing slashes (`rename(file, "new/")` moved the file), a working directory removed and remade under its
name (a relative create landed in the new directory), `fchdir` to a removed directory, `O_TRUNC` on a
directory, `O_ACCMODE`, `fsync` of a directory, the error order of `ftruncate`, `truncate`, `access`,
`pread` and `pwrite`, `SEEK_DATA`/`SEEK_HOLE`, `NAME_MAX`/`PATH_MAX`, partial vectored IO, and the
caller's CPU-time clock ids. Clock and entropy values matched the decided arithmetic in every probe. Empty-image exhaustion is
an expected named fault, and is also fully reversed. Representative `-O0` costs:

| fixture | steps | journal bytes |
|---|---:|---:|
| files: sparse/truncate/append/shared offsets/dup/vector IO | 79,057 | 1,632,977 |
| file errors, modes, links, umask, cwd/rename | 64,475 | 1,328,123 |
| temporary names/readlink/stdio/anonymous tmpfile | 64,625 | 1,299,601 |
| supplied file bytes read in constructor and main | 41,218 | 832,978 |
| clock start 1,234,999,999,998 and 16 entropy bytes | 30,836 | 671,892 |
| clock start `2^64-2`, wrap, same entropy bytes | 31,245 | 680,861 |
| getentropy plus random-device/readv byte checks | 42,189 | 836,029 |
| empty entropy image, expected fault | 2,101 | 41,781 |

The integration suite remains separate from fast `cargo test` and `front/test.sh`; it requires the
explicit musl build. Unsupported operations and finite tape exhaustion have not been counted as
successful native random or wall-clock calls.

Targeted corpus confirmation: **all six prior file-system faults now agree with native**
(`experiments/c-corpus/README.md`, "Files and clocks"). `printf-2` / `user-printf` / `fprintf-2` take
58,353 / 58,416 / 57,717 steps at `9194605`; `/dev/zero` loops `loop-2f` / `loop-2g` and c-testsuite
`00187` agreed earlier on the file implementation at 35,235 / 35,235 / 102,998 steps. The temporary-name
cases found musl's `readlink` probe, now routed correctly for the current non-symlink namespace.
No whole corpus was rerun and no global corpus total is inferred from these six outcomes.

Final combined-revision verification after rebasing on `40c84d4`: `cargo test` **306 passed**,
`front/test.sh` **170 passed**, clippy all targets with warnings denied and formatting clean.
The 16 focused cases passed again in **135.20 s** on the shared machine, with every step/journal count
above unchanged and every entry snapshot restored exactly. This also checks the file/clock models
against the upstream atomic-rewind fixes and libgcc runtime update.

### Copied jump buffers: glibc/musl comparison

The maintainer's reference for a copied `sigjmp_buf` is glibc, even with musl linked
on the device. `front/tests/c/sigjmp_copy_matrix.c` retains 576 cases per optimization
level: eight copy paths (memcpy, struct assignment, two union paths, record-only,
record-plus-flag, mask-only and a full copy in pieces), savemask 0/1/-7, a destination
saved with 0/1, no re-save or re-saving the original with 0/1, jumps through either
buffer, and two masks between save and jump. Every saving frame stays live. The
partial-copy offsets are explicitly x86-64 glibc/musl ABI probes, not portable C
promises about the opaque buffer. Output includes the landing site, jump-value
check and mask; the harness compares complete stdout and exit status.

Build the bitcode library and a separate, unmodified native musl from the vendored
source (no network), from the repository root:

```sh
python3 rt/musl/build.py --jobs 3
mkdir -p target/musl-native/build
(cd target/musl-native/build && \
  ../../../rt/musl/musl-1.2.5/configure --disable-shared --prefix="$PWD/../install" CC=clang-18 && \
  make -j3 && make install)
cargo build -p bennett
front/test.sh
cargo test -p bennett-front-tests --test sigjmp_copy copied_sigjmp_buffers -- --ignored --nocapture
# Equivalent direct runner; --musl-cc/--libdir/--front/--executor/--out override locations:
python3 rt/musl/test-sigjmp.py
```

Measured on x86-64 with clang 18.1.3, glibc 2.39 and native musl 1.2.5: at both
`-O0` and `-O2`, all 576 device cases agree with glibc stdout/status.
Native musl differs from glibc in 94 cases at O0 and 92 at O2; the device follows
glibc in all of them. Native musl also changes 56 rows between optimization levels:
its saved-register reference to the original buffer can change both the landing
site and mask after that original is re-saved. Musl's result is measured separately
at each level, never substituted for the decided reference. Full native output,
device output and per-case differences are generated under `target/front-tests/sigjmp-copy`.

One defect remains: `sigjmp_partial_record.c` copies 16 bytes between two live
savemask-0 environments. This changes callee-saved-register bytes natively, leaving
the copy's continuation intact (glibc and musl status 0, empty stdout); it instead
replaces the device's continuation (status 1, empty stdout), at O0 and O2. The
ignored Rust witness is marked `defect:` and was explicitly observed failing:

```sh
cargo test -p bennett-front-tests --test sigjmp_copy a_partial_record_copy -- --ignored --nocapture
# Or: python3 rt/musl/test-sigjmp.py --partial-record
```

Root cause: the device stores the continuation in bytes 8..16 of its 32-byte
record; x86-64 glibc stores its saved instruction pointer in bytes 56..64. The
passing matrix's partial record copies end at bytes 64 or 72 and therefore do not
exercise this disagreement. Route: preserve glibc's byte-level copy semantics in
the lowered C environment while keeping the device instruction's layout separate,
then enable the witness and extend the prefix/suffix boundary sweep. Jumps after
a saving frame returned remain excluded, as decided.
