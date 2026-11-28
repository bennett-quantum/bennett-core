//! The work counts cover overlapping loads and incomplete address replays.

use std::fmt::Write;

use bennett_exec::{Executor, GateBlock, Options, Outcome};
use bennett_ir::load;

use super::Argument;
use super::bits::{Bit, Circuit, ONE};
use super::memory::{AccessError, AddressProof};
use super::{ApplyInput, compile_apply};

#[test]
fn dense_load_retains_every_overlapping_start() {
    const STARTS: usize = 1 << 18;
    const BYTES: usize = 16;
    const DECLARED: usize = STARTS + BYTES - 1;
    let mut source = format!(
        r#"
memory 263183
data 0 = zero {DECLARED}
func @f(i18 %x) -> i128 {{
entry:
    %address = zext i18 %x to i64
    %word = load i128 %address
    ret %word
}}
func @main(i18 %value) {{
entry:
    %x = qalloc 18
    %out = qalloc 128
    %fault = qalloc 5
    br bit0
"#,
    );
    for bit in 0..18 {
        writeln!(
            source,
            "bit{bit}:
    %shift{bit} = lshr i18 %value, {bit}
    %set{bit} = trunc i18 %shift{bit} to i1
    br %set{bit}, flip{bit}, bit{}
flip{bit}:
    x %x[{bit}]
    br bit{}",
            bit + 1,
            bit + 1,
        )
        .expect("write bit preparation");
    }
    source.push_str(
        "bit18:
    apply @f(%x) -> %out, %fault budget 3
    ret
}
",
    );
    let module = load(&source).expect("valid dense load");
    let main = module.function_named("main").expect("runner");
    let mut block: Option<GateBlock> = None;
    for address in [0, 1, (STARTS / 2) as u64, (STARTS - 1) as u64] {
        let mut direct = Executor::new(&module, main, &[address], Options::default()).expect("direct");
        let mut emitted = Executor::new(&module, main, &[address], Options::default()).expect("emitted");
        let mut compile = |input: ApplyInput<'_>| {
            if let Some(block) = &block {
                return Ok(block.clone());
            }
            let result = compile_apply(input)?;
            let work = result.work;
            assert_eq!(work.interval_addresses, STARTS);
            assert_eq!(work.memory_candidates, STARTS);
            assert_eq!(work.declaration_checks, 1 + STARTS);
            assert_eq!(work.candidate_bytes, STARTS * BYTES);
            assert_eq!(work.memory_bytes, STARTS * BYTES);
            assert_eq!(work.memory_bits, STARTS * BYTES * 8 * 2);
            assert_eq!(result.memory.candidate_bytes, STARTS * BYTES);
            assert_eq!(result.block.ops().len(), 2);
            block = Some(result.block.clone());
            Ok(result.block)
        };
        assert_eq!(direct.run(), Outcome::Returned(None));
        assert_eq!(emitted.run_emitted(&mut compile), Outcome::Returned(None));
        assert_eq!(direct.snapshot().quantum, emitted.snapshot().quantum, "address={address}");
        assert_eq!(direct.snapshot().classical, emitted.snapshot().classical, "address={address}");
        assert_eq!(emitted.stats().nonzero_release_weight, 0.0);
    }
}

#[test]
fn address_replay_rejects_untracked_inputs_and_restarts_after_rewind() {
    let mut proof = AddressProof::new(&[Argument::Register(0..1)]).expect("one input");
    let mut circuit = Circuit::new(2);
    assert!(matches!(proof.constant_under(&circuit, &[Bit::input(1)], ONE), Err(AccessError::Address)));
    let checkpoint = circuit.checkpoint();
    let wire = circuit.fresh();
    circuit.xor_into(wire, ONE, ONE);
    assert!(matches!(proof.constant_under(&circuit, &[circuit.value(wire)], ONE), Ok(1)));
    circuit.rewind(checkpoint);
    let wire = circuit.fresh();
    assert!(matches!(proof.constant_under(&circuit, &[circuit.value(wire)], ONE), Ok(0)));
}

#[test]
fn sparse_known_bits_charge_rejected_starts_and_kept_bytes_separately() {
    let module = load(
        r#"
memory 262144
data 0 = zero 131073
func @f(i9 %x) -> i8 {
entry:
    %bit = trunc i9 %x to i1
    %wide = zext i1 %bit to i64
    %address = shl i64 %wide, 17
    %result = load i8 %address
    ret %result
}
"#,
    )
    .expect("valid sparse interval");
    let bytes = vec![0; 262_144];
    let held = Default::default();
    let compiled = compile_apply(ApplyInput {
        module: &module,
        callee: module.function_named("f").expect("entry"),
        args: &[Argument::Register(0..9)],
        out: 9..17,
        fault: 17..22,
        budget: 8,
        entry_fault: None,
        stack_available: 32,
        memory: Some(bennett_exec::ApplyMemory::new(&bytes, &held)),
        scratch_start: 22,
    })
    .expect("compile a fully declared bound");
    assert_eq!(compiled.work.interval_addresses, 131_073);
    assert_eq!(compiled.work.declaration_checks, 3);
    assert_eq!(compiled.work.memory_candidates, 2);
    assert_eq!(compiled.work.candidate_bytes, 2);
    assert_eq!(compiled.work.memory_bytes, 2);
}
