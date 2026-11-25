//! The counter on a register of the greatest width, 2^32 - 1 qubits, of which a gate uses one: allocated
//! directly, and as a block's scratch summarised once. Each test runs in a child process of this binary
//! under an address-space limit far below one host entry per qubit.

use std::process::Command;

use bennett_emit::{CountingDevice, Counts, LowerControls};
use bennett_exec::{BlockOp, Device, GateBlock, GateOp};
use bennett_ir::{FixedGate, Gate};

/// 192 MiB: a hash-set entry per qubit of the register would need over 60 GiB.
const ADDRESS_SPACE: &str = "201326592";

/// The widest register's qubit count.
const WIDEST: u64 = u32::MAX as u64;

/// Runs `test` in a child process of this binary under the address-space limit; fails by name when
/// `prlimit` cannot be started, rather than running without the limit.
fn under_address_limit(test: &str, body: fn()) {
    if std::env::var_os("BENNETT_UNDER_LIMIT").is_some() {
        return body();
    }
    let exe = std::env::current_exe().expect("the test binary");
    let output = Command::new("prlimit")
        .args([format!("--as={ADDRESS_SPACE}"), "--core=0".into(), "--".into()])
        .arg(exe)
        .args(["--exact", test, "--test-threads", "1"])
        .env("BENNETT_UNDER_LIMIT", "1")
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
