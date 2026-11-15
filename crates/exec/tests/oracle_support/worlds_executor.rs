//! Adapter only: this file can read public executor snapshots; `worlds.rs` cannot.
use std::collections::BTreeMap;

use bennett_exec::{Executor, Options, Outcome};
use bennett_ir::{Inst, ValidModule};
use num_complex::Complex64 as C;

use super::worlds::{Action, Finish, Model, Superposition, Value, basis};

pub fn assert_snapshot(model: &Model<'_>, state: &Superposition, actual: &Executor<'_>) {
    let snapshot = actual.snapshot();
    let mut expected = BTreeMap::<Vec<u64>, C>::new();
    for (world, &a) in state {
        let c = &world.classical;
        assert_eq!(model.memory(world), snapshot.classical.memory, "memory");
        assert_eq!(c.sp, snapshot.classical.stack_pointer, "SP");
        assert_eq!(c.tape, snapshot.classical.tape, "tape");
        assert_eq!(c.frames.len(), snapshot.classical.frames.len(), "frames");
        for (f, actual) in c.frames.iter().zip(&snapshot.classical.frames) {
            assert_eq!(
                (f.function, f.block, f.instruction, f.base, f.entry_sp),
                (actual.func, actual.block, actual.inst, actual.base, actual.entry_stack_pointer),
                "frame"
            );
        }
        for (i, v) in c.slots.iter().take(snapshot.classical.slots.len()).enumerate() {
            let encoded = match v {
                Value::Int(n) => *n,
                Value::Register(id, generation) => (u128::from(*generation) << 32) | (*id as u128),
            };
            assert_eq!(encoded, snapshot.classical.slots[i], "slot {i}");
        }
        for (i, r) in c.registers.iter().enumerate() {
            assert_eq!(r.width, snapshot.quantum.registers[i].width.map(|w| w.bits()), "register {i}");
            assert_eq!(r.generation, u64::from(snapshot.quantum.registers[i].generation), "generation {i}");
        }
        let columns = snapshot.quantum.registers.iter().rposition(|r| r.width.is_some()).map_or(0, |i| i + 1);
        let mut config = world.quantum.clone();
        assert!(config[columns..].iter().all(|&v| v == 0));
        config.truncate(columns);
        *expected.entry(config).or_default() += a;
        let outcome = match &c.finish {
            Some(Finish::Returned(n)) => Some(Outcome::Returned(*n)),
            Some(Finish::Faulted(f)) => Some(Outcome::Faulted(*f)),
            None | Some(Finish::BeforeMeasurement) => None,
        };
        assert_eq!(outcome, actual.outcome(), "outcome");
    }
    for (config, a) in expected.iter().chain(snapshot.quantum.amplitudes.iter().map(|(c, a)| (c, a))) {
        let expected = expected.get(config).copied().unwrap_or_default();
        let actual = snapshot.quantum.amplitude(config);
        assert!(
            (actual - expected).norm() < 1e-12,
            "config {config:?}: expected {expected}, actual {actual}; source {a}"
        );
    }
    assert!((state.values().map(|a| a.norm_sqr()).sum::<f64>() - 1.0).abs() < 1e-11, "full-world norm");
}

/// Compare every pre/post-step public field, not only the final return. Measurement is compared at
/// its boundary; optionally continue down an actual sampled outcome, checking independent Born weight.
pub fn compare(program: &ValidModule, entry: &str, args: &[u64], limit: usize, measure: bool) -> usize {
    let model = Model::new(program, entry);
    let args128: Vec<_> = args.iter().map(|&n| n.into()).collect();
    let mut state = basis(model.initial(entry, &args128));
    let mut actual = Executor::new(
        program,
        program.function_named(entry).expect("entry"),
        args,
        Options {
            reversible: true,
            allow_region_faults: true,
            budget: Some(limit as u64),
            max_configurations: Some(1 << 13),
            ..Options::default()
        },
    )
    .expect("executor starts");
    for step in 0..limit {
        assert_snapshot(&model, &state, &actual);
        if actual.outcome().is_some() {
            return step;
        }
        let work_limit = std::env::var("BENNETT_WORLDS_REPLAY_LIMIT")
            .ok()
            .map(|n| n.parse::<usize>().expect("replay limit"))
            .unwrap_or(256);
        if state
            .keys()
            .any(|w| w.history.iter().any(|r| matches!(r.action,Action::Rewind {count,..} if count>work_limit)))
        {
            eprintln!("reference prefix stopped before exponential replay at round {step}; replay limit {work_limit}");
            return step;
        }
        let w = state.keys().next().expect("normalized state");
        let f = w.frame();
        let measured = if let Some(Inst::Measure { register, .. }) =
            program.function(f.function).block(f.block).insts.get(f.instruction)
        {
            w.register(*register).ok()
        } else {
            None
        };
        if let Some(id) = measured {
            if !measure {
                return step;
            }
            let branches = model.measurement_branches(&state);
            assert!((branches.values().map(|(p, _)| p).sum::<f64>() - 1.0).abs() < 1e-12);
            actual.step();
            let value = actual.snapshot().quantum.amplitudes[0].0[id];
            assert!(branches[&value].0 > 0.0, "sample has independent positive Born probability");
            state = model.collapse(&state, value);
        } else {
            state = model.step(&state);
            actual.step();
        }
    }
    state = model.run(state, 0);
    actual.step();
    assert_snapshot(&model, &state, &actual);
    limit
}
