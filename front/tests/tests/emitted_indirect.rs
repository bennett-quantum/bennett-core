//! A C predicate that selects a function pointer, applied to every eight-bit input.

use bennett_emit::{ApplyInput, ApplyMode, Argument, Construction, CountingDevice, compile_apply_with};
use bennett_exec::{Device, Executor, Outcome};
use bennett_front_tests::{CLANG_FLAGS, entry, lower_text_with_budget, options, write_c};
use bennett_ir::Inst;

const PREDICATE: &str = r#"
__attribute__((noinline)) static int high_bits(unsigned char x) {
  return (x & 0xc0u) == 0xc0u;
}

__attribute__((noinline)) static int low_bits(unsigned char x) {
  return (x & 0x06u) == 0x04u;
}

unsigned char dispatch(unsigned char x) {
  int (*predicate)(unsigned char) = (x & 1u) ? high_bits : low_bits;
  return (unsigned char)predicate(x);
}
"#;

const HARNESS: &str = r#"
func @oracle() {
entry:
    %x = qalloc 8
    h %x[0]
    h %x[1]
    h %x[2]
    h %x[3]
    h %x[4]
    h %x[5]
    h %x[6]
    h %x[7]
    %out = qalloc 8
    %fault = qalloc 5
    x %out[7]
    x %fault[4]
    apply @dispatch(%x) -> %out, %fault budget 20
    ret
}
"#;

#[test]
fn the_c_predicate_dispatches_to_two_targets() {
    let source = write_c("emitted-indirect", PREDICATE);
    let mut text =
        lower_text_with_budget(&source, "emitted-indirect", CLANG_FLAGS, &[], 20).expect("lower C predicate");
    text.push_str(HARNESS);
    let module = bennett_ir::load(&text).expect("valid device program");

    let predicate = module.function(entry(&module, "dispatch"));
    let signature = predicate
        .blocks
        .iter()
        .flat_map(|block| &block.insts)
        .find_map(|inst| if let Inst::CallIndirect { signature, .. } = inst { Some(signature) } else { None })
        .expect("the C call uses an indirect target");
    let targets: Vec<_> = module
        .functions
        .iter()
        .filter(|function| function.has_signature(signature))
        .map(|function| function.name.as_str())
        .collect();
    assert_eq!(targets, ["high_bits", "low_bits"]);

    let oracle = entry(&module, "oracle");
    let mut direct = Executor::new(&module, oracle, &[], options(0)).expect("direct run");
    assert_eq!(direct.run(), Outcome::Returned(None));
    let expected = direct.snapshot();
    assert_eq!(expected.quantum.amplitudes.len(), 256);
    for (basis, _) in &expected.quantum.amplitudes {
        let x = basis[0];
        let selected = if x & 1 == 1 { x & 0xc0 == 0xc0 } else { x & 6 == 4 };
        assert_eq!(*basis, [x, 0x80 ^ u64::from(selected), 0x10]);
    }

    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        let mut emitted = Executor::new(&module, oracle, &[], options(0)).expect("emitted run");
        let mut device = CountingDevice::default();
        let mut compile = |input: ApplyInput<'_>| {
            let [Argument::Register(x)] = input.args else { panic!("one register argument") };
            device.allocate(x.clone())?;
            device.allocate(input.out.clone())?;
            device.allocate(input.fault.clone())?;
            let compiled = compile_apply_with(input, mode)?;
            assert!(matches!(compiled.construction, Construction::StepMachine { .. }));
            assert_eq!(compiled.reason, "qubit-dependent indirect call uses multiway dispatch");
            compiled.send(&mut device)?;
            Ok(compiled.block)
        };
        assert_eq!(emitted.run_emitted(&mut compile), Outcome::Returned(None), "{mode:?}");
        assert_eq!(emitted.snapshot(), expected);
        assert_eq!(emitted.stats().nonzero_release_weight, 0.0);
        println!("{mode:?}: {} targets, {:?}", targets.len(), device.counts());
    }
}
