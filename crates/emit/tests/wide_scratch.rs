//! The counter on a register of the greatest width, 2^32 - 1 qubits, of which a gate uses one: allocated
//! directly, and as a block's scratch summarised once. Each test runs under an address-space limit far
//! below one host entry per qubit, in a child process of this binary unless the run is already under it.

use std::process::Command;

use bennett_emit::{CountingDevice, Counts, LowerControls};
use bennett_exec::{BlockOp, Device, GateBlock, GateOp};
use bennett_ir::{FixedGate, Gate};

/// 192 MiB: a hash-set entry per qubit of the register would need over 60 GiB.
const ADDRESS_SPACE: u64 = 201_326_592;

/// The widest register's qubit count.
const WIDEST: u64 = u32::MAX as u64;

/// The variable by which a parent names itself to the child it starts under the limit.
const PARENT: &str = "BENNETT_LIMIT_PARENT";

/// This process's soft address-space limit in bytes, `None` if unlimited, as the kernel reports it in
/// `/proc/self/limits`.
fn address_space_limit() -> Option<u64> {
    let limits = std::fs::read_to_string("/proc/self/limits").expect("the kernel's /proc/self/limits");
    let row = "Max address space";
    let line = limits.lines().find(|line| line.starts_with(row)).expect("an address-space row");
    match line[row.len()..].split_whitespace().next().expect("a soft limit") {
        "unlimited" => None,
        bytes => Some(bytes.parse().expect("a byte count")),
    }
}

/// Runs `body` under the address-space limit: here if this process is already under it, otherwise in a
/// child process of this binary started by `prlimit`. Nothing inherited from the environment lifts the
/// limit. Fails by name when `prlimit` cannot be started, rather than running without the limit, and when
/// the child it started is not under the limit.
fn under_address_limit(test: &str, body: fn()) {
    if address_space_limit().is_some_and(|limit| limit <= ADDRESS_SPACE) {
        return body();
    }
    let started_by_a_parent =
        std::env::var(PARENT).is_ok_and(|parent| parent == std::os::unix::process::parent_id().to_string());
    assert!(!started_by_a_parent, "{test}: prlimit started this child without the limit of {ADDRESS_SPACE} bytes");
    let exe = std::env::current_exe().expect("the test binary");
    let output = Command::new("prlimit")
        .args([format!("--as={ADDRESS_SPACE}"), "--core=0".into(), "--".into()])
        .arg(exe)
        .args(["--exact", test, "--test-threads", "1"])
        .env(PARENT, std::process::id().to_string())
        .output()
        .unwrap_or_else(|error| panic!("{test} needs prlimit (util-linux) for its memory limit: {error}"));
    assert!(
        output.status.success(),
        "{test} under {ADDRESS_SPACE} bytes: {:?}\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Allocate the widest register from wire 1, a Z on its first qubit, release it in zero.
fn ops() -> Vec<BlockOp> {
    let scratch = 1..1 + WIDEST;
    vec![
        BlockOp::Allocate(scratch.clone()),
        BlockOp::Gate(GateOp { kind: Gate::Fixed(FixedGate::Z), target: 1, controls: vec![] }),
        BlockOp::ReleaseZero(scratch),
    ]
}

/// One Z at depth 1, the register at the peak, all of it released in zero.
fn expected() -> Counts {
    Counts {
        one_qubit: 1,
        peak_qubits: WIDEST,
        depth: 1,
        zero_released_qubits: u128::from(WIDEST),
        ..Counts::default()
    }
}

#[test]
fn the_widest_register_is_counted_when_allocated_directly() {
    under_address_limit("the_widest_register_is_counted_when_allocated_directly", || {
        let mut device = LowerControls::new(CountingDevice::default());
        for op in ops() {
            op.send(&mut device).expect("counted");
        }
        assert_eq!(device.inner().counts(), expected());
    });
}

#[test]
fn the_widest_register_is_counted_as_block_scratch() {
    under_address_limit("the_widest_register_is_counted_as_block_scratch", || {
        let mut device = LowerControls::new(CountingDevice::default());
        let block = GateBlock::from_ops("the widest scratch", ops());
        device.call(&block).expect("counted once");
        device.repeat(&block, 3).expect("and three more times");
        // Each call's scratch starts at layer 0, so the depth stays 1.
        let four = Counts { one_qubit: 4, zero_released_qubits: 4 * u128::from(WIDEST), ..expected() };
        assert_eq!(device.inner().counts(), four);
    });
}

/// Two registers whose qubits together pass 2^32: the peak is the exact sum on every host, not one capped
/// at a 32-bit host's word.
#[test]
fn the_peak_of_two_wide_registers_is_their_sum() {
    under_address_limit("the_peak_of_two_wide_registers_is_their_sum", || {
        let mut device = CountingDevice::default();
        device.allocate(0..WIDEST).expect("the widest register");
        device.allocate(WIDEST..WIDEST + 3).expect("three more qubits");
        let peak: u64 = device.counts().peak_qubits;
        assert_eq!(peak, WIDEST + 3);
    });
}

/// The body of a test run through `under_address_limit` sees the limit.
#[test]
fn the_body_runs_under_the_limit() {
    under_address_limit("the_body_runs_under_the_limit", || {
        let limit = address_space_limit();
        assert!(limit.is_some_and(|limit| limit <= ADDRESS_SPACE), "address-space limit {limit:?}");
    });
}

/// A marker of an earlier way of starting the child, inherited from the environment of the whole run,
/// does not lift the limit (it once did: the body then ran with no limit at all).
#[test]
fn an_inherited_marker_does_not_lift_the_limit() {
    let output = Command::new(std::env::current_exe().expect("the test binary"))
        .args(["--exact", "the_body_runs_under_the_limit", "--test-threads", "1"])
        .env("BENNETT_UNDER_LIMIT", "1")
        .env(PARENT, "1")
        .output()
        .expect("the test binary runs");
    assert!(
        output.status.success(),
        "with inherited markers: {:?}\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
