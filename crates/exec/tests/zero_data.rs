//! A `zero N` data item is the same memory as N zero bytes spelled out: the same initial image and the same
//! stack floor, so every run has the same outcome.
mod common;

use bennett_exec::Outcome;
use bennett_ir::Fault;
use common::{bounded, load, run};

/// Reserves `size` bytes of stack, writes the first byte and returns the address, so the run either
/// returns an address above static data or faults with `stack_overflow`.
fn program(data: &str, size: u64) -> String {
    format!(
        "memory 128\ndata 16 = {data}\nfunc @main() -> i64 {{\nentry:\n    %p = alloca {size}\n    \
         store i8 9, %p\n    %first = load i8 16\n    %zext = zext i8 %first to i64\n    %sum = add i64 %p, %zext\n    \
         ret %sum\n}}\n"
    )
}

#[test]
fn a_zero_extent_moves_the_stack_floor_like_the_bytes_it_stands_for() {
    let zero = "i8 1, zero 46, i8 2";
    let spelled = format!("i8 1, \"{}\", i8 2", "\\0".repeat(46));
    let (mut returned, mut overflowed) = (0, 0);
    for size in (0..=128).step_by(8) {
        let zero_module = load(&program(zero, size));
        let spelled_module = load(&program(&spelled, size));
        assert_eq!(zero_module.static_end(), 64);
        assert_eq!(spelled_module.static_end(), 64);
        let (outcome, executor) = run(&zero_module, "main", &[], bounded());
        let (expected, reference) = run(&spelled_module, "main", &[], bounded());
        assert_eq!(outcome, expected, "alloca {size}");
        assert_eq!(executor.snapshot().classical, reference.snapshot().classical, "alloca {size}");
        match outcome {
            Outcome::Returned(Some(address)) => {
                assert!(address > 64, "alloca {size} returned {address}, inside static data");
                returned += 1;
            }
            Outcome::Faulted(Fault::StackOverflow) => overflowed += 1,
            other => panic!("alloca {size}: {other:?}"),
        }
    }
    assert!(
        returned > 0 && overflowed > 0,
        "the sizes straddle the floor ({returned} returned, {overflowed} overflowed)"
    );
}
