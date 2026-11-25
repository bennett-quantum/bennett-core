//! Registers wider than 64 qubits (`docs/DESIGN.md`, "The device program"): the simulator stores one in
//! several columns of at most 64 qubits, which must not show in what a program finds. Checked at the widths
//! 64, 65, 128 and 1000: values set by gates across the column boundary, correlations between qubits in
//! different columns, `apply` with arguments and results of 65 and 128 qubits, releases of whole registers
//! (kept or removed whole) and forward-then-reverse back to the entry state.

mod common;

use bennett_exec::Outcome;
use common::{assert_reverses, bounded, run};

/// The widths of the brief: the last that fits one column, the first that needs two, the widest an integer
/// can read whole, and one well above.
const WIDTHS: [u32; 4] = [64, 65, 128, 1000];

/// Sets qubits 0, 63 and the top one (the same as 63 at width 64) with `x` and measures the register whole:
/// the integer with those bits.
#[test]
fn bits_set_on_both_sides_of_the_column_boundary_are_measured_in_place() {
    for width in WIDTHS.into_iter().filter(|&width| width <= 128) {
        let bits = std::collections::BTreeSet::from([0, 63, width - 1]);
        let gates: String = bits.iter().map(|bit| format!("    x %q[{bit}]\n")).collect();
        let module = common::load(&format!(
            "memory 64
func @main() -> i{width} {{
entry:
    %q = qalloc {width}
{gates}    %m = measure %q
    ret %m
}}"
        ));
        let expected = bits.iter().fold(0u128, |value, bit| value | 1 << bit);
        let (outcome, executor) = run(&module, "main", &[], bounded());
        assert_eq!(outcome, Outcome::Returned(Some(expected)), "q{width}");
        assert_eq!(executor.stats().live_qubits, width, "q{width}");
    }
}

/// A Bell pair between qubit 0 and the top qubit, in different columns from 65 qubits on: every seed measures
/// both 0 or both 1, and over 16 seeds both outcomes occur.
#[test]
fn a_correlation_across_columns_survives_measurement() {
    for width in WIDTHS.into_iter().filter(|&width| width <= 128) {
        let top = width - 1;
        let module = common::load(&format!(
            "memory 64
func @main() -> i{width} {{
entry:
    %q = qalloc {width}
    h %q[0]
    x %q[{top}] ctrl %q[0]
    %m = measure %q
    ret %m
}}"
        ));
        let both = 1u128 | 1 << top;
        let mut seen = [false; 2];
        for seed in 0..16 {
            let (outcome, _) = run(&module, "main", &[], bennett_exec::Options { seed, ..bounded() });
            match outcome {
                Outcome::Returned(Some(0)) => seen[0] = true,
                Outcome::Returned(Some(value)) if value == both => seen[1] = true,
                other => panic!("q{width} seed {seed}: {other:?}"),
            }
        }
        assert_eq!(seen, [true, true], "q{width}");
    }
}

/// A register of 1000 qubits cannot be measured whole into an integer; its top 100 qubits are copied into
/// a fresh register with controlled X and that is measured (the route the validator names). Qubit 999 is
/// set; qubit 950 is correlated with qubit 700, which is in superposition.
#[test]
fn qubits_of_a_thousand_qubit_register_are_measured_through_a_copy() {
    let copy: String = (0..100).map(|bit| format!("    x %c[{bit}] ctrl %q[{}]\n", 900 + bit)).collect();
    let module = common::load(&format!(
        "memory 64
func @main() -> i100 {{
entry:
    %q = qalloc 1000
    x %q[999]
    h %q[700]
    x %q[950] ctrl %q[700]
    %c = qalloc 100
{copy}    %m = measure %c
    ret %m
}}"
    ));
    let mut seen = [false; 2];
    for seed in 0..16 {
        let (outcome, executor) = run(&module, "main", &[], bennett_exec::Options { seed, ..bounded() });
        match outcome {
            Outcome::Returned(Some(value)) if value == 1 << 99 => seen[0] = true,
            Outcome::Returned(Some(value)) if value == 1 << 99 | 1 << 50 => seen[1] = true,
            other => panic!("seed {seed}: {other:?}"),
        }
        let stats = executor.stats();
        assert_eq!((stats.live_qubits, stats.peak_live_qubits), (1100, 1100));
    }
    assert_eq!(seen, [true, true]);
}

/// At each width: a register entangled across its columns and then released is kept whole (all its
/// qubits left over, even those of columns that were zero); a register released while zero is removed
/// whole, and its ids are reused by the next allocation. Running forward and reversing every step returns
/// to the entry state exactly.
#[test]
fn releases_keep_or_remove_a_wide_register_whole_and_reverse_exactly() {
    for width in WIDTHS {
        let (top, middle) = (width - 1, width / 2);
        let module = common::load(&format!(
            "memory 64
func @main() -> i8 {{
entry:
    %a = qalloc {width}
    h %a[0]
    x %a[{top}] ctrl %a[0]
    ry(0.3) %a[{middle}]
    %clean = qalloc {width}
    x %clean[{top}] ctrl %a[{middle}]
    x %clean[{top}] ctrl %a[{middle}]
    qfree %clean
    %b = qalloc {width}
    h %b[0]
    qfree %a
    qfree %b
    %c = qalloc 65
    x %c[64]
    ret 3
}}"
        ));
        let (outcome, executor) = run(&module, "main", &[], bounded());
        assert_eq!(outcome, Outcome::Returned(Some(3)), "q{width}");
        let stats = executor.stats();
        // `%b` is dirty only in its head column and is still kept whole.
        assert_eq!(stats.leftover_qubits, 2 * width, "q{width}");
        assert_eq!(stats.nonzero_releases, 2, "q{width}");
        // `%a` is zero only where qubit 0 is 0 (1/2) and the rotated qubit is 0 (cos^2 0.15); `%b` is
        // nonzero with weight 1/2.
        let weight = 1.0 - 0.5 * 0.15f64.cos().powi(2) + 0.5;
        assert!((stats.nonzero_release_weight - weight).abs() < 1e-12, "q{width}: {}", stats.nonzero_release_weight);
        // Qubits left over still count as live: `%a` and `%b` kept, then `%c`.
        assert_eq!(stats.peak_live_qubits, 2 * width + 65, "q{width}");
        assert_eq!(assert_reverses(&module, "main", &[], bounded()), Outcome::Returned(Some(3)), "q{width}");
    }
}

/// `apply` reads an argument register of 65 or 128 qubits as one integer and XORs a result of that width
/// into the output register, for each definite value: `x + 1`, with the low 64 qubits all 1 so that the
/// carry crosses the column boundary, and the top qubit in superposition. The outcome is checked against the
/// measured argument for 8 seeds, and the run without measurements reverses exactly.
#[test]
fn apply_reads_and_writes_registers_wider_than_one_column() {
    for width in [65, 128] {
        let top = width - 1;
        let ones: String = (0..64).map(|bit| format!("    x %x[{bit}]\n")).collect();
        let prepare = format!(
            "    %x = qalloc {width}
{ones}    h %x[{top}]
    h %x[3]
    %o = qalloc {width}
    %f = qalloc 70
    apply @inc(%x) -> %o, %f budget 10
"
        );
        let inc = format!(
            "func @inc(i{width} %x) -> i{width} {{
entry:
    %y = add i{width} %x, 1
    ret %y
}}"
        );
        let measured = common::load(&format!(
            "memory 256
{inc}
func @main() -> i1 {{
entry:
{prepare}    %mo = measure %o
    %mx = measure %x
    %mf = measure %f
    %next = add i{width} %mx, 1
    %same = icmp eq i{width} %mo, %next
    %clear = icmp eq i70 %mf, 0
    %ok = and i1 %same, %clear
    ret %ok
}}"
        ));
        for seed in 0..8 {
            let (outcome, _) = run(&measured, "main", &[], bennett_exec::Options { seed, ..bounded() });
            assert_eq!(outcome, Outcome::Returned(Some(1)), "q{width} seed {seed}");
        }
        let unmeasured = common::load(&format!(
            "memory 256
{inc}
func @main() {{
entry:
{prepare}    ret
}}"
        ));
        assert_eq!(assert_reverses(&unmeasured, "main", &[], bounded()), Outcome::Returned(None), "q{width}");
    }
}

/// Two states whose columns hold the same words, with the same register table, but in which the columns
/// form different registers: `%big` takes columns 0 and 1 in one and 1 and 2 in the other, and the set
/// qubit is its qubit 64 in one and its qubit 0 in the other. Measuring `%big` tells them apart, so the
/// snapshots must too.
#[test]
fn snapshots_tell_apart_registers_whose_columns_hold_the_same_words() {
    let state = |text: &str| {
        let module = common::load(text);
        let mut executor = common::executor(&module, "main", &[], bounded());
        for _ in 0..3 {
            executor.step();
        }
        let snapshot = executor.snapshot().quantum;
        (snapshot, executor.run())
    };
    let (first, first_measured) = state(
        "memory 64
func @main() -> i128 {
entry:
    %big = qalloc 128
    %small = qalloc 64
    x %big[64]
    %m = measure %big
    ret %m
}",
    );
    let (second, second_measured) = state(
        "memory 64
func @main() -> i128 {
entry:
    %small = qalloc 64
    %big = qalloc 128
    x %big
    %m = measure %big
    ret %m
}",
    );
    assert_eq!((first_measured, second_measured), (Outcome::Returned(Some(1 << 64)), Outcome::Returned(Some(1))));
    assert_eq!((&first.registers, &first.amplitudes), (&second.registers, &second.amplitudes));
    // The wire table: `%big` from column 0 then `%small` from column 2, against `%small` from column 0 then
    // `%big` from column 1 (wires `c * 2^32 + i`, bits `64 * c + i`).
    let column = |c: u64| c << 32;
    assert_eq!(first.wires, [(0..128, 0), (column(2)..column(2) + 64, 128)]);
    assert_eq!(second.wires, [(0..64, 0), (column(1)..column(1) + 128, 64)]);
    assert_eq!(first.distance(&second), None);
}

/// The address-space limit for the runs below, in bytes (192 MiB): the test binary and the controller's
/// bookkeeping fit; the simulator's table for a register of 2^32 - 1 qubits (2^26 columns of 8 bytes)
/// does not.
const ADDRESS_SPACE: &str = "201326592";

/// Runs `test` in a child process of this binary under the address-space limit, when `prlimit` is present;
/// otherwise in this process if `in_process` (a run that needs no more than the limit), else not at all.
fn under_address_limit(test: &str, in_process: bool, body: fn()) {
    if std::env::var_os("BENNETT_UNDER_LIMIT").is_some() {
        return body();
    }
    let exe = std::env::current_exe().expect("the test binary");
    let child = std::process::Command::new("prlimit")
        .args([format!("--as={ADDRESS_SPACE}"), "--core=0".into(), "--".into()])
        .arg(exe)
        .args(["--exact", test, "--test-threads", "1"])
        .env("BENNETT_UNDER_LIMIT", "1")
        .output();
    match child {
        Ok(output) => assert!(
            output.status.success(),
            "{test} under {ADDRESS_SPACE} bytes: {:?}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout)
        ),
        Err(_) if in_process => body(),
        Err(_) => {}
    }
}

/// A device that records the wire ranges it allocates and the gates it receives.
#[derive(Default)]
struct RangeDevice {
    allocated: Vec<std::ops::Range<bennett_exec::Wire>>,
    targets: Vec<bennett_exec::Wire>,
}

impl bennett_exec::Device for RangeDevice {
    fn allocate(&mut self, wires: std::ops::Range<bennett_exec::Wire>) -> Result<(), bennett_exec::RunError> {
        self.allocated.push(wires);
        Ok(())
    }
    fn gate(&mut self, gate: bennett_exec::GateOp<&[bennett_exec::Wire]>) -> Result<(), bennett_exec::RunError> {
        self.targets.push(gate.target);
        Ok(())
    }
    fn measure(&mut self, _: bennett_exec::Wire) -> Result<bool, bennett_exec::RunError> {
        Ok(false)
    }
    fn discard(&mut self, _: std::ops::Range<bennett_exec::Wire>) -> Result<(), bennett_exec::RunError> {
        Ok(())
    }
    fn release_zero(&mut self, _: std::ops::Range<bennett_exec::Wire>) -> Result<(), bennett_exec::RunError> {
        Ok(())
    }
}

const WIDEST: &str = "memory 64
func @main() {
entry:
    %q = qalloc 4294967295
    x %q[4294967294]
    qfree %q
    ret
}";

/// A register of the greatest width reaches an external device as one range of wires, with its top qubit
/// addressed, within the address-space limit: the controller stores no columns for a device.
#[test]
fn the_widest_register_reaches_an_external_device_as_one_range() {
    under_address_limit("the_widest_register_reaches_an_external_device_as_one_range", true, || {
        let module = common::load(WIDEST);
        let mut device = RangeDevice::default();
        let entry = module.function_named("main").expect("entry");
        let run = bennett_exec::Executor::run_on(&module, entry, &[], None, &mut device).expect("started");
        assert_eq!(run.outcome, Outcome::Returned(None));
        assert_eq!(device.allocated, vec![std::ops::Range { start: 0, end: u64::from(u32::MAX) }]);
        assert_eq!(device.targets, [u64::from(u32::MAX) - 1]);
    });
}

/// The simulator cannot hold that register within the limit: the run stops with the simulator's
/// `QubitStorage`, naming the width, with the state as before the allocation, instead of aborting.
#[test]
fn a_register_the_host_cannot_hold_stops_the_simulator_by_name() {
    under_address_limit("a_register_the_host_cannot_hold_stops_the_simulator_by_name", false, || {
        let module = common::load(WIDEST);
        let mut executor = common::executor(&module, "main", &[], bounded());
        let before = executor.snapshot();
        let stop = bennett_exec::RunError::QubitStorage { qubits: u64::from(u32::MAX) };
        assert_eq!(executor.run(), Outcome::Stopped(stop));
        let after = executor.snapshot();
        assert_eq!(after.quantum, before.quantum, "the allocation left nothing behind");
        assert_eq!(after.classical.frames, before.classical.frames, "still at the allocation");
        assert_eq!(executor.stats().live_qubits, 0);
    });
}

/// A gate on two ordinary memory bytes needs 16 fresh qubits; with 2^32 - 9 held, only 8 fit under the
/// simulator's count. The stop must come before either byte becomes qubits: no byte held, the same count,
/// the same step. Reaching the count takes 2^26 columns (about 2.5 GB and a few seconds), so the test is
/// run on request.
#[test]
#[ignore = "allocates about 2.5 GB to hold 2^32 - 9 qubits; run with --include-ignored"]
fn a_memory_gate_stopped_by_the_qubit_count_leaves_no_byte_held() {
    let module = common::load(
        "memory 128
func @main() {
entry:
    %q = qalloc 4294967287
    x [16:0] ctrl [17:0]
    ret
}",
    );
    let mut executor = common::executor(&module, "main", &[], bennett_exec::Options { reversible: true, ..bounded() });
    assert_eq!(executor.step(), None);
    let before = executor.stats();
    let stop = bennett_exec::RunError::QubitCount { live: u64::from(u32::MAX) - 8, more: 16 };
    assert_eq!(executor.step(), Some(Outcome::Stopped(stop)));
    let after = executor.stats();
    assert_eq!(
        (after.live_qubits, after.memory_qubit_bytes, after.steps),
        (before.live_qubits, before.memory_qubit_bytes, before.steps)
    );
    assert_eq!(after.memory_qubit_bytes, 0);
}
