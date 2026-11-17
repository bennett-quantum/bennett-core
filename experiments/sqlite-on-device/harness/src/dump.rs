//! Print the amplitudes held just before an entry's measurement, for a check outside this observer.
use bennett_exec::{Executor, Options, Outcome};
use bennett_ir::{Inst, ValidModule};

pub fn run(module: &ValidModule, name: &str, configurations: usize) {
    let entry = module.function_named(name).expect("entry");
    let options = Options { max_configurations: Some(configurations), ..Options::default() };
    let mut exec = Executor::new(module, entry, &[], options).expect("start");
    // The entry's first instruction calls the classical setup; skip it without whole-memory snapshots.
    let setup = module.function_named("experiment_setup").expect("setup");
    let mut estimate = Executor::new(module, setup, &[], Options::default()).expect("setup start");
    assert_eq!(estimate.run(), Outcome::Returned(Some(0)));
    for _ in 0..estimate.stats().steps + 1 {
        assert!(exec.step().is_none(), "setup stopped");
    }
    loop {
        let state = exec.snapshot();
        let frame = state.classical.frames.last().expect("entry frame");
        let block = module.function(frame.func).block(frame.block);
        if frame.func == entry && matches!(block.insts.get(frame.inst), Some(Inst::Measure { .. })) {
            for (config, amplitude) in &state.quantum.amplitudes {
                println!("amplitude {config:?} {:.17e} {:.17e}", amplitude.re, amplitude.im);
            }
            break;
        }
        assert!(exec.step().is_none(), "stopped before measurement");
    }
    let outcome = exec.run();
    assert!(matches!(outcome, Outcome::Returned(Some(_))), "{outcome:?}");
    println!("outcome {outcome:?} region_faults {}", exec.stats().region_faults);
}
