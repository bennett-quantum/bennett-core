//! Independent tests of structure-pointer checking and error order across every system call
//! implemented in `rt/musl/` that takes a pointer to a structure (`clock_gettime`, `clock_getres`,
//! `gettimeofday`, `stat`, `lstat`, `fstat`, `newfstatat`, `readv`, `writev`, `rt_sigaction`,
//! `rt_sigprocmask`, `rt_sigpending`) plus `time` (output `time_t *`) and the data buffers and transfer
//! counts of `read`, `write`, `readv`, `writev` and `getrandom`, and the path strings of every path call,
//! comparing the device against native Linux (clang 18 + host kernel).
//!
//! Each syscall is tested with:
//! - (a) a fully readable valid argument,
//! - (b) a fully readable invalid argument,
//! - (c) an input argument whose first part is readable and last part is not (at the end of a
//!   mapped page natively and at `memory_size` on the device; also below address 16, NULL, and
//!   wrapping around 2^64), and
//! - (d) an output pointer that is unreadable or partly so.
//!
//! A range that starts below address 16 (where bytes are touched) or wraps or ends above Linux's
//! user address space top, 2^47 - 4096 (where the call checks it), is `EFAULT` on the device as
//! natively; where the device stops by name with `Fault::OutOfBounds` for a range that reaches
//! `memory_size` (a known debt; the runtime does not know the memory size), the tests assert that
//! named stop. The tests after the first four were defect witnesses of an independent testing pass,
//! each observed failing before the runtime copied structures in and out whole (`rt/musl/copy.h`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, Once, OnceLock};

use bennett_exec::{Executor, Options, Outcome};
use bennett_front_tests::{clang, out_dir, root};
use bennett_ir::Fault;

/// Step budget per run; each small syscall probe finishes in under 50,000 steps.
const STEP_BUDGET: u64 = 10_000_000;

/// Ensures the musl bitcode library is built once per test process.
fn ensure_musl() {
    static MUSL: Once = Once::new();
    MUSL.call_once(|| {
        let workspace = root().canonicalize().expect("workspace");
        let build = Command::new("python3")
            .arg(workspace.join("rt/musl/build.py"))
            .arg("--jobs")
            .arg("3")
            .output()
            .expect("musl build");
        assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    });
}

/// Compiles `rt/musl/tests/{suite}.c` natively once per process and returns the executable path.
fn native_exe(suite: &str) -> PathBuf {
    static BUILT: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    let cache = BUILT.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().expect("native cache lock");
    if let Some(path) = guard.get(suite) {
        return path.clone();
    }
    let workspace = root().canonicalize().expect("workspace");
    let source = workspace.join("rt/musl/tests").join(format!("{suite}.c"));
    let exe = out_dir().join(format!("{suite}.native"));
    let status =
        Command::new(clang()).arg("-O0").arg(&source).arg("-o").arg(&exe).output().expect("clang native build");
    assert!(status.status.success(), "native build {suite}: {}", String::from_utf8_lossy(&status.stderr));
    guard.insert(suite.to_owned(), exe.clone());
    exe
}

/// Runs `suite` natively with `case` in an isolated working directory and returns its stdout.
fn run_native(suite: &str, case: &str) -> String {
    let exe = native_exe(suite);
    let cwd = out_dir().join(format!("efault-cwd-{suite}-{case}"));
    std::fs::create_dir_all(&cwd).expect("create native cwd");
    let output = Command::new(&exe).arg(case).current_dir(&cwd).output().expect("native run");
    assert_eq!(
        output.status.code(),
        Some(0),
        "native {suite} {case} failed: status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("native stdout utf8")
}

/// Links `rt/musl/tests/{suite}.c` with `link_flags` (options of `link.py`) and `args` and returns the loaded
/// device module.
fn link_suite(suite: &str, tag: &str, link_flags: &[&str], args: &[&str]) -> bennett_ir::ValidModule {
    ensure_musl();
    let workspace = root().canonicalize().expect("workspace");
    let source = workspace.join("rt/musl/tests").join(format!("{suite}.c"));
    let file = out_dir().join(format!("{suite}-{tag}.bn"));
    let mut cmd = Command::new("python3");
    cmd.arg(workspace.join("rt/musl/link.py")).arg(&source).args(link_flags).arg("-o").arg(&file).arg("--").args(args);
    let linked = cmd.output().expect("musl link");
    assert!(linked.status.success(), "link {suite} {tag}: {}", String::from_utf8_lossy(&linked.stderr));
    let text = std::fs::read_to_string(&file).expect("read linked bn");
    bennett_ir::load(&text).expect("valid device module")
}

/// The fixed `memory_size` of `suite` (independent of `argv` because `__bennett_argument_bytes`
/// has a fixed 4096-byte footprint).
fn suite_memory_size(suite: &str) -> u64 {
    static SIZES: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    let cache = SIZES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().expect("memory size cache lock");
    if let Some(&size) = guard.get(suite) {
        return size;
    }
    let module = link_suite(suite, "probe-size", &[], &["agree"]);
    let size = module.memory_size;
    guard.insert(suite.to_owned(), size);
    size
}

/// Links and runs `suite` on the device for `case`. When `pass_edge` is true, passes the module's
/// `memory_size` as `argv[2]` and asserts that the linked module's `memory_size` matches it.
fn run_device(suite: &str, case: &str, pass_edge: bool) -> (Outcome, String) {
    let module = if pass_edge {
        let edge = suite_memory_size(suite);
        let edge_str = edge.to_string();
        let module = link_suite(suite, case, &[], &[case, &edge_str]);
        assert_eq!(module.memory_size, edge, "{suite} {case}: memory_size shifted across relink");
        module
    } else {
        link_suite(suite, case, &[], &[case])
    };
    run_module(&module)
}

/// Runs a linked module from `__bennett_entry` and returns its outcome and console output.
fn run_module(module: &bennett_ir::ValidModule) -> (Outcome, String) {
    let entry = module.function_named("__bennett_entry").expect("__bennett_entry");
    let options = Options { budget: Some(STEP_BUDGET), ..Options::default() };
    let mut executor = Executor::new(module, entry, &[], options).expect("executor start");
    let outcome = executor.run();
    let stdout = String::from_utf8(executor.tape().to_vec()).expect("device stdout utf8");
    (outcome, stdout)
}

/// Asserts that `suite` on `case` agrees verbatim with native Linux.
fn assert_agrees(suite: &str, case: &str, pass_edge: bool) {
    let native = run_native(suite, case);
    let (outcome, device) = run_device(suite, case, pass_edge);
    assert_eq!(outcome, Outcome::Returned(Some(0)), "{suite} {case}: device outcome {outcome:?} (native:\n{native})");
    assert_eq!(device, native, "{suite} {case}: device output differs from native");
}

/// Asserts that `suite` on `case` returns `expected_native` natively and stops with
/// `Fault::OutOfBounds` on the device.
fn assert_named_stop(suite: &str, case: &str, pass_edge: bool, expected_native_substr: &str) {
    let native = run_native(suite, case);
    assert!(
        native.contains(expected_native_substr),
        "{suite} {case}: native output {native:?} did not contain {expected_native_substr:?}"
    );
    let (outcome, device) = run_device(suite, case, pass_edge);
    assert_eq!(
        outcome,
        Outcome::Faulted(Fault::OutOfBounds),
        "{suite} {case}: expected OutOfBounds, got {outcome:?} (stdout={device:?}, native={native:?})"
    );
}

#[test]
fn time_syscalls_agree_on_valid_invalid_and_null_and_stop_on_out_of_bounds() {
    assert_agrees("efault-time", "agree", true);
    assert_named_stop("efault-time", "clock_gettime-partial8", true, "ret=-1 errno=14");
    assert_named_stop("efault-time", "clock_getres-partial8", true, "ret=-1 errno=14");
    assert_named_stop("efault-time", "gettimeofday-tv-partial8", true, "ret=-1 errno=14");
    assert_named_stop("efault-time", "gettimeofday-tz-partial4", true, "ret=-1 errno=14");
    assert_named_stop("efault-time", "time-partial4", true, "ok=0 errno=14");
}

#[test]
fn stat_syscalls_agree_on_valid_invalid_and_null_and_stop_on_out_of_bounds() {
    assert_agrees("efault-stat", "agree", true);
    assert_named_stop("efault-stat", "stat-partial136", true, "ret=-1 errno=14");
    assert_named_stop("efault-stat", "lstat-partial8", true, "ret=-1 errno=14");
    assert_named_stop("efault-stat", "fstat-partial8", true, "ret=-1 errno=14");
    assert_named_stop("efault-stat", "newfstatat-partial8", true, "ret=-1 errno=14");
}

#[test]
fn iov_syscalls_agree_on_valid_invalid_and_null_and_stop_on_out_of_bounds() {
    assert_agrees("efault-iov", "agree", true);
    assert_named_stop("efault-iov", "writev-partial8", true, "ret=-1 errno=14");
    assert_named_stop("efault-iov", "writev-partial16-valid0", true, "ret=-1 errno=14");
    assert_named_stop("efault-iov", "readv-partial8", true, "ret=-1 errno=14");
    assert_named_stop("efault-iov", "readv-partial16-valid0", true, "ret=-1 errno=14");
}

#[test]
fn signal_syscalls_agree_on_valid_invalid_and_size_order_and_stop_on_out_of_bounds() {
    assert_agrees("efault-signals", "agree", true);
    assert_named_stop("efault-signals", "sigaction-act-partial24", true, "ret=-1 errno=14");
    assert_named_stop("efault-signals", "sigaction-old-partial8", true, "ret=-1 errno=14");
    assert_named_stop("efault-signals", "sigprocmask-set-partial4", true, "ret=-1 errno=14");
    assert_named_stop("efault-signals", "sigprocmask-old-partial4", true, "ret=-1 errno=14");
    assert_named_stop("efault-signals", "sigpending-partial4", true, "ret=-1 errno=14");
}

// ---------------------------------------------------------------------------------------------------------
// Former defect witnesses, now regression tests: each failed on the runtime before structures
// were copied in and out whole.

/// Formerly a defect witness: Linux's `rt_sigaction` copies `act` from userspace before checking whether `sig`
/// is in `1..=64` or is `SIGKILL`/`SIGSTOP`. `rt/musl/runtime.c` checked `sig` and `SIGKILL`/`SIGSTOP`
/// first, so even a partially readable (`memory_size - 8`) or wrapping (`(void *)-8`) `act` pointer
/// returns `-1 / EINVAL` instead of `-1 / EFAULT` (or `out_of_bounds`).
///
/// Run:
///   `front/test.sh --test syscall_efault_independent rt_sigaction_checks_act_readability_before_signal_validity -- --exact --nocapture`
///
/// Expected (native Linux): `ret=-1 errno=14` (`EFAULT`) for all five cases (or `out_of_bounds` on device).
/// Observed on the device before the fix: `ret=-1 errno=22` (`EINVAL`) for all five cases.
#[test]
fn rt_sigaction_checks_act_readability_before_signal_validity() {
    for (case, pass_edge) in [
        ("sigaction-sigkill-partial8", true),
        ("sigaction-sigstop-partial8", true),
        ("sigaction-sigkill-wrap8", false),
        ("sigaction-sig0-partial8", true),
        ("sigaction-sig65-partial8", true),
    ] {
        let native = run_native("efault-signals", case);
        let (outcome, device) = run_device("efault-signals", case, pass_edge);
        let accepted = outcome == Outcome::Faulted(Fault::OutOfBounds)
            || (outcome == Outcome::Returned(Some(0)) && device == native);
        assert!(accepted, "{case}: native={native:?}, device=({outcome:?}, {device:?})");
    }
}

/// Formerly a defect witness: Linux's `rt_sigprocmask` copies `set` from userspace before checking `how`.
/// `rt/musl/runtime.c` checked `how` first, so a partially readable (`memory_size - 4`) or wrapping
/// (`(void *)-4`) `set` pointer with `how = 99` returns `-1 / EINVAL` instead of `-1 / EFAULT`
/// (or `out_of_bounds`).
///
/// Run:
///   `front/test.sh --test syscall_efault_independent rt_sigprocmask_checks_set_readability_before_how -- --exact --nocapture`
///
/// Expected (native Linux): `ret=-1 errno=14` (`EFAULT`) (or `out_of_bounds` on device).
/// Observed on the device before the fix: `ret=-1 errno=22` (`EINVAL`).
#[test]
fn rt_sigprocmask_checks_set_readability_before_how() {
    for (case, pass_edge) in [("sigprocmask-badhow-partial4", true), ("sigprocmask-badhow-wrap4", false)] {
        let native = run_native("efault-signals", case);
        let (outcome, device) = run_device("efault-signals", case, pass_edge);
        let accepted = outcome == Outcome::Faulted(Fault::OutOfBounds)
            || (outcome == Outcome::Returned(Some(0)) && device == native);
        assert!(accepted, "{case}: native={native:?}, device=({outcome:?}, {device:?})");
    }
}

/// Formerly a defect witness: Linux's `writev` and `readv` check the file descriptor's read/write access mode
/// (`EBADF`) before validating `iovcnt`, checking `iov` for null/fault, or summing `iov_len`.
/// `rt/musl/devices.c` checked `iovcnt`, `!iov`, and `iov[i].iov_len` first and checked the descriptor's
/// access mode afterward, returning `EFAULT` or `EINVAL` instead of `EBADF`.
///
/// Run:
///   `front/test.sh --test syscall_efault_independent readv_and_writev_check_fd_access_mode_before_iov_arguments -- --exact --nocapture`
///
/// Expected (native Linux): `ret=-1 errno=9` (`EBADF`) for all six cases.
/// Observed on the device before the fix: `ret=-1 errno=14` (`EFAULT`) for `*-null` and `ret=-1 errno=22` (`EINVAL`) for `*-badlen` and `*-badcnt`.
#[test]
fn readv_and_writev_check_fd_access_mode_before_iov_arguments() {
    for case in [
        "writev-ro-null",
        "writev-ro-badlen",
        "writev-ro-badcnt",
        "readv-wo-null",
        "readv-wo-badlen",
        "readv-wo-badcnt",
    ] {
        assert_agrees("efault-iov", case, false);
    }
}

/// Formerly a defect witness: with `iov = (void *)(uintptr_t)-8` and `iovcnt = 1`, `rt/musl/devices.c` read
/// `v[0].iov_len` at `(uintptr_t)-8 + 8 == 0` (wrapping in 64-bit arithmetic to address 0, which
/// is in bounds and holds 0). Because `iov_len == 0`, `v[0].iov_base` at `-8` is never read and
/// both `writev` and `readv` return `0` instead of `-1 / EFAULT` (or `out_of_bounds`).
///
/// Run:
///   `front/test.sh --test syscall_efault_independent readv_and_writev_reject_wrapping_iov_pointer -- --exact --nocapture`
///
/// Expected (native Linux): `ret=-1 errno=14` (`EFAULT`) (or `out_of_bounds` on device).
/// Observed on the device before the fix: `ret=0 errno=0`.
#[test]
fn readv_and_writev_reject_wrapping_iov_pointer() {
    for case in ["writev-wrap8", "readv-wrap8"] {
        let native = run_native("efault-iov", case);
        let (outcome, device) = run_device("efault-iov", case, false);
        let accepted = outcome == Outcome::Faulted(Fault::OutOfBounds)
            || (outcome == Outcome::Returned(Some(0)) && device == native);
        assert!(accepted, "{case}: native={native:?}, device=({outcome:?}, {device:?})");
    }
}

/// Formerly a defect witness: `rt/musl/runtime.c`'s `SYS_rt_sigaction` wrote `*oldact` before reading `*act`.
/// When `act == oldact` (a valid, readable struct pointer), writing `*oldact` overwrites the new
/// action with the old disposition before installing it.
///
/// Run:
///   `front/test.sh --test syscall_efault_independent rt_sigaction_handles_aliased_act_and_oldact -- --exact --nocapture`
///
/// Expected (native Linux): `sigaction-alias: ret=0 errno=0 installed_ign=1 old_dfl=1`
/// Observed on the device before the fix: `sigaction-alias: ret=0 errno=0 installed_ign=0 old_dfl=1`
#[test]
fn rt_sigaction_handles_aliased_act_and_oldact() {
    assert_agrees("efault-signals", "sigaction-alias", false);
}

/// Formerly a defect witness: `rt/musl/runtime.c`'s `SYS_rt_sigpending` did not check for a null `set`
/// pointer; it writes `pending` to address 0 (which is in bounds of device memory) and returns `0`.
///
/// Run:
///   `front/test.sh --test syscall_efault_independent rt_sigpending_rejects_null_pointer_with_efault -- --exact --nocapture`
///
/// Expected (native Linux): `sigpending-null: ret=-1 errno=14`
/// Observed on the device before the fix: `sigpending-null: ret=0 errno=0`
#[test]
fn rt_sigpending_rejects_null_pointer_with_efault() {
    assert_agrees("efault-signals", "sigpending-null", false);
}

/// Formerly a defect witness: Linux's `rt_sigpending` accepts `sigsetsize <= sizeof(kernel_sigset_t)` (8),
/// so `sigsetsize = 4` succeeds with `0`. `rt/musl/runtime.c` required `sigsetsize == 8` and
/// returns `-1 / EINVAL`.
///
/// Run:
///   `front/test.sh --test syscall_efault_independent rt_sigpending_accepts_sigsetsize_smaller_than_eight -- --exact --nocapture`
///
/// Expected (native Linux): `sigpending-size4: ret=0 errno=0`
/// Observed on the device before the fix: `sigpending-size4: ret=-1 errno=22`
#[test]
fn rt_sigpending_accepts_sigsetsize_smaller_than_eight() {
    assert_agrees("efault-signals", "sigpending-size4", false);
}

/// Formerly a defect witness: device memory is flat from address 0 (`firstDataAddress = 16`, where runtime
/// globals begin), and `rt/musl/` only checks `!ptr` (address 0). Every implemented structure-pointer
/// syscall accepts non-null addresses below 16 (here `(void *)8`), returning success instead of
/// `EFAULT` (and for output structures larger than 8 bytes, overwriting static globals from address 16).
///
/// Run:
///   `front/test.sh --test syscall_efault_independent syscalls_reject_non_null_addresses_below_sixteen -- --exact --nocapture`
///
/// Expected (native Linux): `ret=-1 errno=14` (`ok=0 errno=14` for `time`) for every case.
/// Observed on the device before the fix: `ret=0 errno=0` (`ret=4 errno=0` for `iov_base`, `ok=1 errno=0` for `time`) for every case.
#[test]
fn syscalls_reject_non_null_addresses_below_sixteen() {
    let cases = [
        ("efault-time", "clock_gettime-low8"),
        ("efault-time", "clock_getres-low8"),
        ("efault-time", "gettimeofday-tv-low8"),
        ("efault-time", "gettimeofday-tz-low8"),
        ("efault-time", "time-low8"),
        ("efault-stat", "stat-low8"),
        ("efault-stat", "lstat-low8"),
        ("efault-stat", "fstat-low8"),
        ("efault-stat", "newfstatat-low8"),
        ("efault-iov", "writev-low8"),
        ("efault-iov", "writev-base-low8"),
        ("efault-iov", "readv-low8"),
        ("efault-iov", "readv-base-low8"),
        ("efault-signals", "sigaction-act-low8"),
        ("efault-signals", "sigaction-old-low8"),
        ("efault-signals", "sigprocmask-set-low8"),
        ("efault-signals", "sigprocmask-old-low8"),
        ("efault-signals", "sigpending-low8"),
    ];
    for (suite, case) in cases {
        assert_agrees(suite, case, false);
    }
}

/// Ranges the device can tell are unreadable (below 16 or wrapping) are `EFAULT` for every structure
/// and data buffer, the descriptor's mode is checked before a vector that cannot be read, and a
/// vector read over its own array uses the copy of the array, as natively.
#[test]
fn ranges_below_sixteen_or_wrapping_and_copied_vectors_agree() {
    let cases = [
        ("efault-time", "clock_gettime-wrap8", false),
        ("efault-time", "clock_getres-wrap8", false),
        ("efault-time", "gettimeofday-tv-wrap8", false),
        ("efault-time", "gettimeofday-tz-wrap4", false),
        ("efault-time", "time-wrap4", false),
        ("efault-stat", "stat-wrap8", false),
        ("efault-stat", "fstat-wrap8", false),
        ("efault-iov", "writev-ro-partial8", true),
        ("efault-iov", "readv-wo-partial8", true),
        ("efault-iov", "readv-alias", false),
        ("efault-iov", "read-low8", false),
        ("efault-iov", "write-low8", false),
        ("efault-iov", "getrandom-low8", false),
        ("efault-iov", "getcwd-low8", false),
        ("efault-signals", "sigaction-sigkill-low8", false),
        ("efault-signals", "sigaction-act-wrap8", false),
        ("efault-signals", "sigaction-old-wrap8", false),
        ("efault-signals", "sigprocmask-badhow-low8", false),
        ("efault-signals", "sigprocmask-set-wrap4", false),
        ("efault-signals", "sigprocmask-old-wrap4", false),
        ("efault-signals", "sigpending-wrap4", false),
    ];
    for (suite, case, pass_edge) in cases {
        assert_agrees(suite, case, pass_edge);
    }
}

/// A review witness: the shared dispatcher kept the copies of a 1024-element vector (16 KiB) in its frame, so a
/// valid `clock_gettime` stopped with `stack_overflow` on an 8 KiB stack. Each call's copies now live in its own
/// frame, a vector's sized for its count.
///
/// Expected (native Linux): every call `ret=0 errno=0`. Observed on the device before the fix: `StackOverflow`.
#[test]
fn small_stack_calls_keep_their_copies_in_their_own_frames() {
    for suite in ["efault-stack"] {
        let native = run_native(suite, "small-stack");
        let module = link_suite(suite, "small-stack", &["--stack-bytes=8192"], &["small-stack"]);
        let (outcome, device) = run_module(&module);
        assert_eq!(outcome, Outcome::Returned(Some(0)), "{suite}: device outcome {outcome:?} (native:\n{native})");
        assert_eq!(device, native, "{suite}: device output differs from native");
    }
}

/// Review witnesses of the copy-in step (each observed on the device before its fix):
/// - `writev-wrap-second`, `readv-wrap-second`: a wrapping second buffer is `EFAULT` before the first byte moves,
///   offset unchanged (device: `ret=1`, offset 1).
/// - `low-untouched`: a buffer below 16 faults only where bytes are transferred (device: `EFAULT` at the end of a
///   file, for `/dev/null`, a directory and empty stdin, where Linux answers 0, the count or `EISDIR`).
/// - `ranges-and-cap`: Linux's range bound (end at most 2^47 - 4096) and its cap of 0x7ffff000 bytes per call
///   (device: the uncapped count, `EINVAL` for an overflowing sum where Linux answers `EFAULT`).
/// - `sigaction-stored-mask`: a stored action's mask drops `SIGKILL` and `SIGSTOP` (device: kept them).
/// - `at-fdcwd-int`: `AT_FDCWD` passed as an int to the raw `*at` calls (device: `EBADF`).
/// - `sigpending-size4`: exactly the four requested bytes of the pending set are written (a sentinel check).
/// - `nanosleep-above-top`: a request above the top is `EFAULT` (device: the named stop `out_of_bounds`).
#[test]
fn transfers_check_ranges_where_linux_does_and_cap_counts() {
    let cases = [
        ("efault-iov", "writev-wrap-second"),
        ("efault-iov", "readv-wrap-second"),
        ("efault-iov", "low-untouched"),
        ("efault-iov", "ranges-and-cap"),
        ("efault-signals", "sigaction-stored-mask"),
        ("efault-signals", "sigpending-size4"),
        ("efault-stat", "at-fdcwd-int"),
        ("efault-time", "nanosleep-above-top"),
    ];
    for (suite, case) in cases {
        assert_agrees(suite, case, false);
    }
}

/// Re-check witness of the copy-in step: `positional-overflow`. After the buffer's range and before the cap, a
/// position plus the whole count beyond `LONG_MAX` is `EINVAL` (Linux's `rw_verify_area`), for `/dev/null`,
/// `/dev/zero` and a regular file, by `pread64` and `pwrite64` (device: the capped count, 0, the bytes or
/// `EFBIG`).
#[test]
fn positional_transfers_refuse_a_position_plus_count_beyond_long_max() {
    assert_agrees("efault-iov", "positional-overflow", false);
}

/// Witness of path strings read in place: every path call copies its string first, as Linux's `getname`: a
/// string starting below 16, at or above the top of the user address space, or wrapping is `EFAULT` (device:
/// `ENOENT` for one at 8, `out_of_bounds` for the others), and the copy comes after the arguments Linux checks
/// first and before the directory descriptor and the walk (`rename`: the old name's walk before the new name's
/// copy error). A string whose first byte lies past device memory stops by name.
#[test]
fn path_strings_are_copied_in_as_linux_copies_them() {
    assert_agrees("efault-path", "agree", true);
    for case in ["path-low8", "path-wrap8", "path-above-top"] {
        assert_agrees("efault-path", case, false);
    }
    assert_named_stop("efault-path", "path-at-edge", true, "ret=-1 errno=14");
}

/// Review witness of the path copy: `rename` read its new name's bytes before the old name's walk, so a new name
/// past device memory stopped the run with `out_of_bounds` where Linux answers the old walk's `ENOENT` (device
/// before: the named stop at the first row). A path whose NUL is the last byte of device memory is read up to it
/// (`path-edge-agree`); one with no NUL before the end, and a new name past the end after a good old walk, stop
/// by name (natively `EFAULT`).
#[test]
fn paths_at_the_end_of_device_memory_are_read_up_to_their_nul_in_linux_order() {
    assert_agrees("efault-path", "path-edge-agree", true);
    assert_named_stop("efault-path", "path-to-edge", true, "ret=-1 errno=14");
    assert_named_stop("efault-path", "rename-new-past-edge", true, "ret=-1 errno=14");
}
