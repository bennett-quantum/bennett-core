//! Compare compact and explicit initializers independently of the emitter's own tests.
mod common;

use bennett_exec::{Executor, Options, Outcome};
use bennett_ir::{DataItem, Fault};
use common::{bounded, entry, load};

#[test]
fn mixed_zero_items_produce_the_expected_entire_initial_memory() {
    for zeros in [0, 1, 15, 16, 17, 257] {
        let text = format!(
            "memory 512\ndata 400 = zero 3, \"tail\"\ndata 3 = \"\\xff;\\\"\\\\\", zero {zeros}, \
             i9 511, zero 0, i64 @main, \"\\0z\"\nfunc @main() {{\nentry:\n ret\n}}\n"
        );
        let module = load(&text);
        let executor = Executor::new(&module, entry(&module, "main"), &[], bounded()).expect("starts");
        let mut expected = vec![0; 512];
        expected[3..7].copy_from_slice(b"\xff;\"\\");
        let at = 7 + zeros;
        expected[at..at + 2].copy_from_slice(&[255, 1]);
        expected[at + 2..at + 10].copy_from_slice(&1u64.to_le_bytes());
        expected[at + 10..at + 12].copy_from_slice(b"\0z");
        expected[403..407].copy_from_slice(b"tail");
        assert_eq!(executor.snapshot().classical.memory, expected, "zero {zeros}");
        assert_eq!(module.static_end(), 407);
    }
}

#[test]
fn expanding_zero_items_preserves_stack_boundaries_calls_writes_and_reversal() {
    let mut returned = 0;
    let mut overflowed = 0;
    for floor in [17, 23, 24, 31, 32, 63, 64, 65, 127] {
        for size in 0..=136 {
            let text = format!(
                "memory 256\ndata {floor} = zero 0\ndata 1 = i8 73, zero {}, i8 91\n\
                 func @main() -> i64 {{\nentry:\n %p = alloca {size}\n %v = call @write(%p)\n ret %p\n}}\n\
                 func @write(i64 %p) -> i8 {{\nentry:\n store i8 33, %p\n %v = load i8 1\n ret %v\n}}\n",
                floor - 3
            );
            let compact = load(&text);
            let mut spelled = (*compact).clone();
            for segment in &mut spelled.data {
                for item in &mut segment.items {
                    if let DataItem::Zero(count) = item {
                        *item = DataItem::Bytes(vec![0; *count as usize]);
                    }
                }
            }
            // Send the reference through both writer and reader, rather than using encode() as its oracle.
            let spelled = load(&bennett_ir::write(&spelled));
            assert_eq!(compact.static_end(), floor);
            assert_eq!(compact.static_end(), spelled.static_end());
            let options = Options { reversible: true, ..bounded() };
            let mut a = Executor::new(&compact, entry(&compact, "main"), &[], options).expect("starts");
            let mut b = Executor::new(&spelled, entry(&spelled, "main"), &[], options).expect("starts");
            let initial = a.snapshot();
            assert_eq!(initial, b.snapshot(), "floor {floor}, alloca {size}: entry");
            let actual = a.run();
            assert_eq!(actual, b.run(), "floor {floor}, alloca {size}: outcome");
            assert_eq!(a.snapshot(), b.snapshot(), "floor {floor}, alloca {size}: final state");
            assert_eq!(a.snapshot().classical.memory[1], 73);
            assert_eq!(a.snapshot().classical.memory[(floor - 1) as usize], 91);
            match actual {
                Outcome::Returned(Some(address)) => {
                    assert!(address >= u128::from(floor), "floor {floor}, alloca {size}: address {address}");
                    returned += 1;
                }
                Outcome::Faulted(Fault::StackOverflow) => overflowed += 1,
                other => panic!("floor {floor}, alloca {size}: {other:?}"),
            }
            assert_eq!(a.reverse().expect("reverses"), b.reverse().expect("reference reverses"));
            assert_eq!(a.snapshot(), initial, "floor {floor}, alloca {size}: restored state");
            assert_eq!(a.snapshot(), b.snapshot());
        }
    }
    assert_eq!(returned + overflowed, 1233);
    assert!(returned > 0 && overflowed > 0);
    eprintln!("stack comparisons: {returned} returned, {overflowed} stack_overflow");
}
