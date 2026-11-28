//! Independent tests of structure-pointer checking and error order across every system call
//! implemented in `rt/musl/` that takes a pointer to a structure (`clock_gettime`, `clock_getres`,
//! `gettimeofday`, `stat`, `lstat`, `fstat`, `newfstatat`, `readv`, `writev`, `rt_sigaction`,
//! `rt_sigprocmask`, `rt_sigpending`) plus `time` (output `time_t *`), comparing the device
//! against native Linux (clang 18 + host kernel).
//!
//! Each syscall is tested with:
//! - (a) a fully readable valid argument,
//! - (b) a fully readable invalid argument,
//! - (c) an input argument whose first part is readable and last part is not (at the end of a
//!   mapped page natively and at `memory_size` on the device; also below address 16, NULL, and
//!   wrapping around 2^64), and
//! - (d) an output pointer that is unreadable or partly so.
//!
//! Where the device stops by name with `Fault::OutOfBounds` for an address past `memory_size` or
//! wrapping around 2^64 (a known debt; the device does not yet turn out-of-bounds memory accesses
//! into `EFAULT`), the active tests assert that named stop. Every non-named-stop difference
//! between native Linux and the device is recorded as an ignored `defect:` witness below.

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

/// Links `rt/musl/tests/{suite}.c` with `args` and returns the loaded device module.
fn link_suite(suite: &str, tag: &str, args: &[&str]) -> bennett_ir::ValidModule {
    ensure_musl();
    let workspace = root().canonicalize().expect("workspace");
    let source = workspace.join("rt/musl/tests").join(format!("{suite}.c"));
    let file = out_dir().join(format!("{suite}-{tag}.bn"));
    let mut cmd = Command::new("python3");
    cmd.arg(workspace.join("rt/musl/link.py")).arg(&source).arg("-o").arg(&file).arg("--").args(args);
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
    let module = link_suite(suite, "probe-size", &["agree"]);
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
        let module = link_suite(suite, case, &[case, &edge_str]);
        assert_eq!(module.memory_size, edge, "{suite} {case}: memory_size shifted across relink");
        module
    } else {
        link_suite(suite, case, &[case])
    };
    let entry = module.function_named("__bennett_entry").expect("__bennett_entry");
    let options = Options { budget: Some(STEP_BUDGET), ..Options::default() };
    let mut executor = Executor::new(&module, entry, &[], options).expect("executor start");
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
    assert_named_stop("efault-time", "clock_gettime-wrap8", false, "ret=-1 errno=14");
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
// Defect witnesses: non-named-stop differences between native Linux and the device.

/// Defect witness: Linux's `rt_sigaction` copies `act` from userspace before checking whether `sig`
/// is in `1..=64` or is `SIGKILL`/`SIGSTOP`. `rt/musl/runtime.c` checks `sig` and `SIGKILL`/`SIGSTOP`
/// first, so even a partially readable (`memory_size - 8`) or wrapping (`(void *)-8`) `act` pointer
/// returns `-1 / EINVAL` instead of `-1 / EFAULT` (or `out_of_bounds`).
///
/// Reproduce:
///   `front/test.sh --test syscall_efault_independent rt_sigaction_checks_act_readability_before_signal_validity -- --ignored --exact --nocapture`
///
/// Expected (native Linux): `ret=-1 errno=14` (`EFAULT`) for all five cases (or `out_of_bounds` on device).
/// Observed (device): `ret=-1 errno=22` (`EINVAL`) for all five cases.
#[test]
#[ignore = "defect: rt_sigaction checks sig and SIGKILL/SIGSTOP before reading act (observed EINVAL, expected EFAULT or out_of_bounds)"]
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

/// Defect witness: Linux's `rt_sigprocmask` copies `set` from userspace before checking `how`.
/// `rt/musl/runtime.c` checks `how` first, so a partially readable (`memory_size - 4`) or wrapping
/// (`(void *)-4`) `set` pointer with `how = 99` returns `-1 / EINVAL` instead of `-1 / EFAULT`
/// (or `out_of_bounds`).
///
/// Reproduce:
///   `front/test.sh --test syscall_efault_independent rt_sigprocmask_checks_set_readability_before_how -- --ignored --exact --nocapture`
///
/// Expected (native Linux): `ret=-1 errno=14` (`EFAULT`) (or `out_of_bounds` on device).
/// Observed (device): `ret=-1 errno=22` (`EINVAL`).
#[test]
#[ignore = "defect: rt_sigprocmask checks how before reading set (observed EINVAL, expected EFAULT or out_of_bounds)"]
fn rt_sigprocmask_checks_set_readability_before_how() {
    for (case, pass_edge) in [("sigprocmask-badhow-partial4", true), ("sigprocmask-badhow-wrap4", false)] {
        let native = run_native("efault-signals", case);
        let (outcome, device) = run_device("efault-signals", case, pass_edge);
        let accepted = outcome == Outcome::Faulted(Fault::OutOfBounds)
            || (outcome == Outcome::Returned(Some(0)) && device == native);
        assert!(accepted, "{case}: native={native:?}, device=({outcome:?}, {device:?})");
    }
}

/// Defect witness: Linux's `writev` and `readv` check the file descriptor's read/write access mode
/// (`EBADF`) before validating `iovcnt`, checking `iov` for null/fault, or summing `iov_len`.
/// `rt/musl/devices.c` checks `iovcnt`, `!iov`, and `iov[i].iov_len` first and checks the descriptor's
/// access mode afterward, returning `EFAULT` or `EINVAL` instead of `EBADF`.
///
/// Reproduce:
///   `front/test.sh --test syscall_efault_independent readv_and_writev_check_fd_access_mode_before_iov_arguments -- --ignored --exact --nocapture`
///
/// Expected (native Linux): `ret=-1 errno=9` (`EBADF`) for all six cases.
/// Observed (device): `ret=-1 errno=14` (`EFAULT`) for `*-null` and `ret=-1 errno=22` (`EINVAL`) for `*-badlen` and `*-badcnt`.
#[test]
#[ignore = "defect: readv/writev check iovcnt, iov, and iov_len before fd read/write mode (observed EFAULT/EINVAL, expected EBADF)"]
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

/// Defect witness: with `iov = (void *)(uintptr_t)-8` and `iovcnt = 1`, `rt/musl/devices.c` reads
/// `v[0].iov_len` at `(uintptr_t)-8 + 8 == 0` (wrapping in 64-bit arithmetic to address 0, which
/// is in bounds and holds 0). Because `iov_len == 0`, `v[0].iov_base` at `-8` is never read and
/// both `writev` and `readv` return `0` instead of `-1 / EFAULT` (or `out_of_bounds`).
///
/// Reproduce:
///   `front/test.sh --test syscall_efault_independent readv_and_writev_reject_wrapping_iov_pointer -- --ignored --exact --nocapture`
///
/// Expected (native Linux): `ret=-1 errno=14` (`EFAULT`) (or `out_of_bounds` on device).
/// Observed (device): `ret=0 errno=0`.
#[test]
#[ignore = "defect: readv/writev with iov=(void*)-8 wraps iov_len to address 0 and returns 0 (expected EFAULT or out_of_bounds)"]
fn readv_and_writev_reject_wrapping_iov_pointer() {
    for case in ["writev-wrap8", "readv-wrap8"] {
        let native = run_native("efault-iov", case);
        let (outcome, device) = run_device("efault-iov", case, false);
        let accepted = outcome == Outcome::Faulted(Fault::OutOfBounds)
            || (outcome == Outcome::Returned(Some(0)) && device == native);
        assert!(accepted, "{case}: native={native:?}, device=({outcome:?}, {device:?})");
    }
}

/// Defect witness: `rt/musl/runtime.c`'s `SYS_rt_sigaction` writes `*oldact` before reading `*act`.
/// When `act == oldact` (a valid, readable struct pointer), writing `*oldact` overwrites the new
/// action with the old disposition before installing it.
///
/// Reproduce:
///   `front/test.sh --test syscall_efault_independent rt_sigaction_handles_aliased_act_and_oldact -- --ignored --exact --nocapture`
///
/// Expected (native Linux): `sigaction-alias: ret=0 errno=0 installed_ign=1 old_dfl=1`
/// Observed (device): `sigaction-alias: ret=0 errno=0 installed_ign=0 old_dfl=1`
#[test]
#[ignore = "defect: rt_sigaction writes oldact before reading act, breaking act == oldact (observed installed_ign=0, expected installed_ign=1)"]
fn rt_sigaction_handles_aliased_act_and_oldact() {
    assert_agrees("efault-signals", "sigaction-alias", false);
}

/// Defect witness: `rt/musl/runtime.c`'s `SYS_rt_sigpending` does not check for a null `set`
/// pointer; it writes `pending` to address 0 (which is in bounds of device memory) and returns `0`.
///
/// Reproduce:
///   `front/test.sh --test syscall_efault_independent rt_sigpending_rejects_null_pointer_with_efault -- --ignored --exact --nocapture`
///
/// Expected (native Linux): `sigpending-null: ret=-1 errno=14`
/// Observed (device): `sigpending-null: ret=0 errno=0`
#[test]
#[ignore = "defect: rt_sigpending omits the null check, writes address 0, and returns 0 (expected -1 / EFAULT)"]
fn rt_sigpending_rejects_null_pointer_with_efault() {
    assert_agrees("efault-signals", "sigpending-null", false);
}

/// Defect witness: Linux's `rt_sigpending` accepts `sigsetsize <= sizeof(kernel_sigset_t)` (8),
/// so `sigsetsize = 4` succeeds with `0`. `rt/musl/runtime.c` requires `sigsetsize == 8` and
/// returns `-1 / EINVAL`.
///
/// Reproduce:
///   `front/test.sh --test syscall_efault_independent rt_sigpending_accepts_sigsetsize_smaller_than_eight -- --ignored --exact --nocapture`
///
/// Expected (native Linux): `sigpending-size4: ret=0 errno=0`
/// Observed (device): `sigpending-size4: ret=-1 errno=22`
#[test]
#[ignore = "defect: rt_sigpending rejects sigsetsize=4 with EINVAL (expected 0 as on Linux)"]
fn rt_sigpending_accepts_sigsetsize_smaller_than_eight() {
    assert_agrees("efault-signals", "sigpending-size4", false);
}

/// Defect witness: device memory is flat from address 0 (`firstDataAddress = 16`, where runtime
/// globals begin), and `rt/musl/` only checks `!ptr` (address 0). Every implemented structure-pointer
/// syscall accepts non-null addresses below 16 (here `(void *)8`), returning success instead of
/// `EFAULT` (and for output structures larger than 8 bytes, overwriting static globals from address 16).
///
/// Reproduce:
///   `front/test.sh --test syscall_efault_independent syscalls_reject_non_null_addresses_below_sixteen -- --ignored --exact --nocapture`
///
/// Expected (native Linux): `ret=-1 errno=14` (`ok=0 errno=14` for `time`) for every case.
/// Observed (device): `ret=0 errno=0` (`ret=4 errno=0` for `iov_base`, `ok=1 errno=0` for `time`) for every case.
#[test]
#[ignore = "defect: non-null addresses below 16 (e.g. (void*)8) are accepted as valid memory across all 13 structure-pointer syscalls (expected EFAULT)"]
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
