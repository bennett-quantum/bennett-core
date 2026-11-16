//! Observe each SQL apply and the full state immediately before measurement.
use bennett_exec::{Executor, Options, Outcome};
use bennett_ir::{Inst, ValidModule};

pub fn run(module: &ValidModule, names: &str) {
    for name in names.split(',') {
        println!("entry {name}");
        let entry = module.function_named(name).expect("entry");
        let setup_name = format!("{name}_setup");
        let setup = module.function_named(&setup_name).expect("classical setup");
        let mut estimate = Executor::new(module, setup, &[], Options::default()).expect("setup");
        assert_eq!(estimate.run(), Outcome::Returned(Some(0)));
        let setup_steps = estimate.stats().steps;
        let mut exec = Executor::new(
            module,
            entry,
            &[],
            Options { budget: Some(100_000_000), max_configurations: Some(16), ..Options::default() },
        )
        .expect("start");
        // The entry's first instruction calls this exact classical setup.
        for _ in 0..setup_steps + 1 {
            assert!(exec.step().is_none(), "setup stopped");
        }
        let fixed = exec.snapshot();
        let mut calls = 0;
        loop {
            let before = exec.snapshot();
            let frame = before.classical.frames.last().expect("entry frame");
            assert_eq!(frame.func, entry);
            let block = module.function(entry).block(frame.block);
            match block.insts.get(frame.inst) {
                Some(Inst::Apply { .. }) => {
                    let old = exec.stats();
                    assert!(exec.step().is_none(), "SQL apply stopped");
                    let after = exec.snapshot();
                    assert_eq!(after.classical.memory, before.classical.memory);
                    assert_eq!(after.classical.stack_pointer, before.classical.stack_pointer);
                    assert_eq!(after.classical.slots, before.classical.slots);
                    assert_eq!(after.classical.tape, before.classical.tape);
                    assert_eq!(after.quantum.registers, before.quantum.registers);
                    assert!(after.worlds.is_empty());
                    assert!(after.quantum.cells.is_empty());
                    assert!(after.quantum.amplitudes.iter().all(|(c, _)| c[2] == 0));
                    let now = exec.stats();
                    assert_eq!(now.region_faults, 0);
                    assert_eq!(now.journal_bytes, old.journal_bytes);
                    assert_eq!(now.overlay_bytes, 0);
                    assert_eq!(now.live_qubits, old.live_qubits);
                    assert_eq!(now.worlds, 1);
                    calls += 1;
                    println!(
                        "oracle {calls} region_steps {} runs {} work {} journal {} overlay {} copied {} retained_journal {:?} retained_overlay {} live_qubits {}",
                        now.region_steps - old.region_steps,
                        now.region_cost.runs - old.region_cost.runs,
                        now.region_cost.work - old.region_cost.work,
                        now.region_cost.journal_bytes - old.region_cost.journal_bytes,
                        now.region_cost.overlay_bytes - old.region_cost.overlay_bytes,
                        now.region_cost.copied_bytes - old.region_cost.copied_bytes,
                        now.journal_bytes,
                        now.overlay_bytes,
                        now.live_qubits,
                    );
                }
                Some(Inst::Measure { .. }) => {
                    assert_eq!(before.classical.memory, fixed.classical.memory);
                    assert_eq!(before.classical.stack_pointer, fixed.classical.stack_pointer);
                    assert_eq!(before.classical.tape, fixed.classical.tape);
                    assert!(before.worlds.is_empty());
                    assert!(before.quantum.cells.is_empty());
                    assert_eq!(exec.stats().live_qubits + 5, exec.stats().peak_live_qubits);
                    for (config, amplitude) in &before.quantum.amplitudes {
                        println!("basis {config:?} amplitude {:.17e} {:.17e}", amplitude.re, amplitude.im);
                    }
                    println!("premeasurement {:?}", exec.stats());
                    break;
                }
                _ => assert!(exec.step().is_none(), "stopped before measurement"),
            }
        }
        let result = exec.run();
        assert!(matches!(result, Outcome::Returned(Some(_))), "{result:?}");
        println!("outcome {result:?}");
        println!("stats {:?}", exec.stats());
        println!("all memory, database, stack and tape preserved; fault and kick qubits freed cleanly");
    }
}
