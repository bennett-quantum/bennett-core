//! Opt-in observer: native/dense answers are checked by coherent.py.
use bennett_exec::{Executor, Options, Outcome};
use bennett_ir::Inst;
use std::{env, fs};

fn main() {
    let args: Vec<_> = env::args().collect();
    let text = fs::read_to_string(&args[1]).expect("device text");
    let module = bennett_ir::load(&text).expect("valid device program");
    for name in args[2].split(',') {
        println!("entry {name}");
        let entry = module.function_named(name).expect("entry");
        let mut exec = Executor::new(
            &module,
            entry,
            &[],
            Options { reversible: true, budget: Some(100_000_000), max_configurations: Some(16), ..Options::default() },
        )
        .expect("start");
        let initial = exec.snapshot();
        // All generated entries set up the Lua state classically, then allocate
        // the input register. Stats avoids copying megabytes at every C step.
        while exec.stats().live_qubits == 0 {
            assert!(exec.step().is_none(), "stopped in setup");
        }
        let outcome = loop {
            let before = exec.snapshot();
            let frame = before.classical.frames.last().expect("entry frame");
            assert_eq!(frame.func, entry);
            let block = module.function(entry).block(frame.block);
            if matches!(block.insts.get(frame.inst), Some(Inst::Apply { .. })) {
                let old = exec.stats();
                assert!(exec.step().is_none(), "apply stopped");
                let after = exec.snapshot();
                assert_eq!(after.classical.memory, before.classical.memory);
                assert_eq!(after.classical.stack_pointer, before.classical.stack_pointer);
                assert_eq!(after.classical.slots, before.classical.slots);
                assert_eq!(after.classical.tape, before.classical.tape);
                assert_eq!(after.classical.outcome, before.classical.outcome);
                let mut frames = before.classical.frames.clone();
                frames.last_mut().expect("frame").inst += 1;
                assert_eq!(after.classical.frames, frames);
                assert_eq!(after.quantum.registers, before.quantum.registers);
                assert!(after.worlds.is_empty());
                assert!(after.quantum.cells.is_empty());
                assert!(after.quantum.amplitudes.iter().all(|(c, _)| c[2] == 0));
                let now = exec.stats();
                assert_eq!(now.region_faults, 0);
                assert_eq!(now.overlay_bytes, 0);
                assert_eq!(now.live_qubits, old.live_qubits);
                println!(
                    "oracle steps {} runs {} work {} journal {} overlay {} copied {}",
                    now.region_steps - old.region_steps,
                    now.region_cost.runs - old.region_cost.runs,
                    now.region_cost.work - old.region_cost.work,
                    now.region_cost.journal_bytes - old.region_cost.journal_bytes,
                    now.region_cost.overlay_bytes - old.region_cost.overlay_bytes,
                    now.region_cost.copied_bytes - old.region_cost.copied_bytes,
                );
            } else if let Some(outcome) = exec.step() {
                break outcome;
            }
        };
        assert_eq!(outcome, Outcome::Returned(Some(0)));
        let final_state = exec.snapshot();
        for (config, amplitude) in &final_state.quantum.amplitudes {
            println!("basis {config:?} amplitude {:.17e} {:.17e}", amplitude.re, amplitude.im);
        }
        println!("stats {:?}", exec.stats());
        exec.reverse().expect("reverse");
        let restored = exec.snapshot();
        assert_eq!(restored.classical, initial.classical);
        assert_eq!(restored.worlds, initial.worlds);
        let distance = restored.quantum.distance(&initial.quantum).expect("same registers");
        assert!(distance < 1e-12, "reverse amplitude distance {distance}");
        println!("reverse identical {distance:e}");
    }
}
