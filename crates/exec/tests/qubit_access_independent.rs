//! Byte-range access checks with an oracle derived from integer interval overlap, independent of cells.

mod common;

use bennett_exec::{Options, Outcome};
use bennett_ir::Fault;
use common::{assert_same_state, bounded, executor, load};

#[test]
fn every_integer_width_faults_exactly_when_load_or_store_overlaps_a_qubit_byte() {
    // Every width 1..128; a cell before, at each byte of, and after the access. Gating bit 7 must
    // protect the whole byte even when a narrow access touches only its low, ungated bits.
    for width in 1u32..=128 {
        let bytes = width.div_ceil(8);
        for access in [format!("%v = load i{width} 64"), format!("store i{width} 1, 64")] {
            for prepare in ["z [%cell:7]", "h [%cell:7]", "h [%cell:7]\n%measured = measure i8 [%cell]"] {
                let module = load(&format!(
                    "memory 512\nfunc @main(i64 %cell) -> i8 {{\nentry:\n{prepare}\n{access}\nret 27\n}}"
                ));
                for offset in -1..=i64::from(bytes) {
                    let address = (64 + offset) as u64;
                    let mut e = executor(&module, "main", &[address], Options { reversible: true, ..bounded() });
                    for _ in 0..prepare.lines().count() {
                        assert_eq!(e.step(), None);
                    }
                    let before = e.snapshot();
                    let journal = e.stats().journal_bytes;
                    if (0..i64::from(bytes)).contains(&offset) {
                        assert_eq!(
                            e.step(),
                            Some(Outcome::Faulted(Fault::QubitMemoryAccess)),
                            "i{width} {access}, {prepare}, offset {offset}"
                        );
                        let mut after = e.snapshot();
                        after.classical.outcome = None;
                        assert_same_state(&after, &before, "fault leaves state intact");
                        assert_eq!(e.stats().journal_bytes, journal);
                    } else {
                        assert_eq!(e.run(), Outcome::Returned(Some(27)), "i{width}, offset {offset}");
                    }
                }
            }
        }
    }
}

#[test]
fn applied_loads_and_stores_xor_fault_separately_for_each_argument_address() {
    for width in [1u32, 7, 8, 9, 64, 128] {
        for access in [format!("%v = load i{width} %address"), format!("store i{width} 1, %address")] {
            let module = load(&format!(
                "memory 1024
func @probe(i3 %x) -> i8 {{
entry:
 %wide = zext i3 %x to i64
 %offset = mul i64 %wide, 4
 %address = add i64 %offset, 48
 store i8 99, 8
 {access}
 ret 29
}}
func @main() {{
entry:
 %arg = qalloc 3
 %out = qalloc 8
 %fault = qalloc 5
 h %arg[0]
 h %arg[1]
 h %arg[2]
 s %arg[1]
 x %out[0]
 x %out[2]
 x %fault[0]
 x %fault[1]
 h [64:7]
 apply @probe(%arg) -> %out, %fault budget 20
 ret
}}"
            ));
            let options = Options { reversible: true, allow_region_faults: true, ..bounded() };
            let mut e = executor(&module, "main", &[], options);
            let entry = e.snapshot();
            assert_eq!(e.run(), Outcome::Returned(None));
            let snapshot = e.snapshot();
            assert_eq!(snapshot.classical.memory[8], 0, "writes before faults and successful returns are private");
            let bytes = u64::from(width.div_ceil(8));
            let mut faulting = 0;
            assert_eq!(snapshot.quantum.amplitudes.len(), 16);
            for (config, amplitude) in &snapshot.quantum.amplitudes {
                let address = 48 + config[0] * 4;
                let hits = address <= 64 && 64 < address + bytes;
                assert_eq!(config[1], if hits { 5 } else { 5 ^ 29 }, "i{width}, {config:?}");
                assert_eq!(config[2], if hits { 3 ^ 17 } else { 3 }, "i{width}, {config:?}");
                let phase = if config[0] & 2 == 0 {
                    num_complex::Complex64::new(0.25, 0.0)
                } else {
                    num_complex::Complex64::new(0.0, 0.25)
                };
                assert!((amplitude - phase).norm() < 1e-12);
                faulting += usize::from(hits);
            }
            assert_eq!(e.stats().region_faults, 1);
            assert!((e.stats().region_fault_weight - faulting as f64 / 16.0).abs() < 1e-12);
            e.reverse().expect("reverse per-argument faults");
            assert_same_state(&e.snapshot(), &entry, "applied accesses entry");
        }
    }
}

#[test]
fn every_jump_record_byte_is_protected_inside_and_outside_apply() {
    // A record has four 64-bit words. Probe each byte plus the two neighboring bytes. The longjmp
    // input is deliberately malformed: touching a cell must fault before decoding any record field.
    for access in ["%saved = setjmp 64\nret 29", "longjmp 64, 1"] {
        for applied in [false, true] {
            for cell in 63..=96 {
                let body = if applied {
                    "%out = qalloc 8\n%fault = qalloc 5\nx %fault[1]\napply @probe() -> %out, %fault budget 10\nret"
                } else {
                    "%v = call @probe()\nret"
                };
                let module = load(&format!(
                    "memory 512\nfunc @probe() -> i8 {{\nentry:\n{access}\n}}\nfunc @main() {{\nentry:\nz [{cell}:7]\n{body}\n}}"
                ));
                let mut e = executor(&module, "main", &[], Options { allow_region_faults: true, ..bounded() });
                let outcome = e.run();
                let hits = (64..96).contains(&cell);
                let fault = if hits {
                    Some(Fault::QubitMemoryAccess)
                } else if access.starts_with("longjmp") {
                    Some(Fault::BadLongjmp)
                } else {
                    None
                };
                if applied {
                    assert_eq!(outcome, Outcome::Returned(None));
                    let snapshot = e.snapshot();
                    for (config, _) in &snapshot.quantum.amplitudes {
                        // Column 0 is the cell, then output and fault.
                        assert_eq!(config[1], if fault.is_none() { 29 } else { 0 }, "cell {cell}: {access}");
                        assert_eq!(config[2], 2 ^ fault.map_or(0, Fault::code), "cell {cell}: {access}");
                    }
                    assert!(snapshot.classical.memory.iter().all(|&byte| byte == 0));
                } else {
                    assert_eq!(
                        outcome,
                        fault.map_or(Outcome::Returned(None), Outcome::Faulted),
                        "cell {cell}: {access}"
                    );
                }
            }
        }
    }
}

#[test]
fn byte_copy_stops_at_the_first_qubit_byte_without_changing_it_or_later_bytes() {
    for quantum_source in [false, true] {
        let cell = if quantum_source { 18 } else { 34 };
        let module = load(&format!(
            "memory 512
data 16 = i32 67305985
data 32 = i32 134678021
func @main() {{
entry:
 z [{cell}:7]
 br copy(i64 0)
copy(i64 %i):
 %source = add i64 %i, 16
 %dest = add i64 %i, 32
 %v = load i8 %source
 store i8 %v, %dest
 %next = add i64 %i, 1
 %more = icmp ult i64 %next, 4
 br %more, copy(%next), done
done:
 ret
}}"
        ));
        let mut e = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
        let entry = e.snapshot();
        assert_eq!(e.run(), Outcome::Faulted(Fault::QubitMemoryAccess));
        let snapshot = e.snapshot();
        assert_eq!(snapshot.classical.memory[32..36], if quantum_source { [1, 2, 7, 8] } else { [1, 2, 0, 8] });
        assert_eq!(snapshot.quantum.amplitudes[0].0, [if quantum_source { 3 } else { 7 }]);
        e.reverse().expect("reverse completed copy prefix");
        assert_same_state(&e.snapshot(), &entry, "copy prefix entry");
    }
}

#[test]
fn gates_and_measurements_still_work_after_ordinary_access_faults_are_introduced() {
    let module = load(
        "memory 256
data 64 = i8 165
func @main() -> i8 {
entry:
 x [64:7]
 z [64:3]
 %low = measure i1 [64]
 x [64:1]
 %all = measure i8 [64]
 ret %all
}",
    );
    assert_eq!(common::run(&module, "main", &[], bounded()).0, Outcome::Returned(Some(39)));
}

#[test]
fn a_measured_stack_byte_stays_protected_after_its_frame_returns() {
    for access in ["%v = load i1 %address", "store i1 0, %address"] {
        let module = load(&format!(
            "memory 512
func @promote() -> i64 {{
entry:
 %p = alloca 8
 store i8 165, %p
 h [%p:7]
 %measured = measure i8 [%p]
 ret %p
}}
func @main() {{
entry:
 %address = call @promote()
 {access}
 ret
}}"
        ));
        for seed in 0..8 {
            let mut e = executor(&module, "main", &[], Options { seed, ..bounded() });
            assert_eq!(e.run(), Outcome::Faulted(Fault::QubitMemoryAccess));
            assert_eq!(e.stats().memory_qubit_bytes, 1);
            assert_eq!(e.snapshot().classical.frames.len(), 1, "callee returned before access");
        }
    }
}
