//! Standalone experiment observer, using only the executor's public interface.
use bennett_exec::{Executor, Options, Outcome};
use std::{env, fs, time::Instant};
mod dump;
mod grover;
mod scale;

fn main() {
    let args: Vec<_> = env::args().collect();
    let text = fs::read_to_string(&args[1]).expect("device text");
    let module = bennett_ir::load(&text).expect("valid device program");
    if args[2] == "scale" {
        scale::run(&module, &args[3..]);
        return;
    }
    if args[2] == "dump" {
        dump::run(&module, &args[3], args[4].parse().expect("configuration bound"));
        return;
    }
    if args[2] == "grover" {
        grover::run(&module, &args[3]);
        return;
    }
    if args[2] == "stack" {
        stack_sweep(module);
        return;
    }
    let reversible = args.get(3).is_some_and(|arg| arg == "reverse");
    for name in args[2].split(',') {
        observe(&module, name, reversible);
    }
}

fn observe(module: &bennett_ir::ValidModule, name: &str, reversible: bool) {
    println!("entry {name}");
    let entry = module.function_named(name).expect("entry");
    let mut exec = Executor::new(
        module,
        entry,
        &[],
        Options {
            reversible,
            budget: Some(100_000_000),
            max_configurations: Some(8),
            allow_region_faults: true,
            ..Options::default()
        },
    )
    .expect("start");
    let before = exec.snapshot();
    let start = Instant::now();
    let result = if name == "uniform0" { run_checking_memory(&mut exec, module, entry) } else { exec.run() };
    println!("outcome {result:?}, seconds {:.6}", start.elapsed().as_secs_f64());
    println!("stats {:?}", exec.stats());
    let after = exec.snapshot();
    println!("memory {} static {}", module.memory_size, module.static_end());
    for frame in &after.classical.frames {
        let function = module.function(frame.func);
        let block = function.block(frame.block);
        println!("frame {} block {} instruction {}", function.name, frame.block.index(), frame.inst);
        if frame.inst < block.insts.len() {
            println!("instruction {:?}", block.insts[frame.inst]);
        }
    }
    println!("registers {:?}", after.quantum.registers);
    for (config, amplitude) in &after.quantum.amplitudes {
        println!("basis {config:?} amplitude {:.15} {:.15}", amplitude.re, amplitude.im);
    }
    if reversible {
        exec.reverse().expect("reverse");
        let restored = exec.snapshot();
        assert_eq!(restored.classical, before.classical);
        assert_eq!(restored.worlds, before.worlds);
        let distance = restored.quantum.distance(&before.quantum).expect("same register table");
        assert!(distance < 1e-12, "amplitude distance {distance}");
        println!("reverse identical, amplitude distance {distance:e}");
    }
    if !matches!(result, Outcome::Returned(_)) {
        std::process::exit(1);
    }
}

fn stack_sweep(module: bennett_ir::ValidModule) {
    let image = module.into_module();
    let original = bennett_ir::validate(image.clone()).expect("original valid");
    let entry = original.function_named("__bennett_entry").expect("entry");
    let mut baseline = Executor::new(&original, entry, &[], Options::default()).expect("start");
    assert_eq!(baseline.run(), Outcome::Returned(Some(0)));
    let expected = baseline.tape().to_vec();
    let mut low = 0;
    let mut high = image.memory_size - image.static_end();
    let floor = image.static_end();
    while high - low > 1 {
        let middle = (high + low) / 2;
        let mut trial = image.clone();
        trial.memory_size = floor + middle;
        let valid = bennett_ir::validate(trial).expect("valid memory bound");
        let entry = valid.function_named("__bennett_entry").expect("entry");
        let mut exec = Executor::new(&valid, entry, &[], Options::default()).expect("start");
        let result = exec.run();
        match result {
            Outcome::Returned(Some(0)) => {
                assert_eq!(exec.tape(), expected);
                high = middle;
            }
            Outcome::Faulted(bennett_ir::Fault::StackOverflow) => low = middle,
            other => panic!("unexpected {other:?}"),
        }
        println!("stack {middle}: {result:?}, steps {}", exec.stats().steps);
        if high - low == 1 {
            let mut last = image.clone();
            last.memory_size = floor + low;
            let valid = bennett_ir::validate(last).expect("valid");
            let entry = valid.function_named("__bennett_entry").expect("entry");
            let mut failed = Executor::new(&valid, entry, &[], Options::default()).expect("start");
            assert_eq!(failed.run(), Outcome::Faulted(bennett_ir::Fault::StackOverflow));
            let state = failed.snapshot();
            let frame = state.classical.frames.last().expect("fault frame");
            let function = valid.function(frame.func);
            println!("failed at {} block {} inst {}", function.name, frame.block.index(), frame.inst);
            let block = function.block(frame.block);
            if frame.inst < block.insts.len() {
                println!("fault instruction {:?}", block.insts[frame.inst]);
            }
        }
    }
    println!("minimum_stack_bytes {high}, static_end {floor}");
}

// Estimate when setup returns by a separate classical run, then inspect only the
// short suffix. Taking a whole-memory snapshot at every setup instruction would
// overwhelm the measurement. Only the snapshot at the actual apply is compared.
fn run_checking_memory(
    exec: &mut Executor<'_>,
    module: &bennett_ir::ValidModule,
    entry: bennett_ir::FuncId,
) -> Outcome {
    let setup = module.function_named("experiment_setup").expect("setup");
    let mut estimate = Executor::new(module, setup, &[], Options::default()).expect("setup start");
    assert_eq!(estimate.run(), Outcome::Returned(Some(0)));
    for _ in 0..estimate.stats().steps.saturating_sub(2048) {
        assert!(exec.step().is_none(), "stopped before apply");
    }
    loop {
        let before = exec.snapshot();
        let frame = before.classical.frames.last().expect("live frame");
        let block = module.function(frame.func).block(frame.block);
        if frame.func == entry && matches!(block.insts.get(frame.inst), Some(bennett_ir::Inst::Apply { .. })) {
            assert!(exec.step().is_none(), "apply stopped");
            let after = exec.snapshot();
            assert_eq!(after.classical.memory, before.classical.memory);
            assert_eq!(after.classical.stack_pointer, before.classical.stack_pointer);
            assert_eq!(after.classical.slots, before.classical.slots);
            assert_eq!(after.classical.tape, before.classical.tape);
            println!("apply preserved all memory, stack, slots and tape");
            return exec.run();
        }
        assert!(exec.step().is_none(), "stopped before apply");
    }
}
