//! Adapter only: this file can read public executor snapshots; `worlds.rs` cannot.
use std::collections::BTreeMap;

use bennett_exec::{Executor, Options, Outcome};
use bennett_ir::{Inst, ValidModule};
use num_complex::Complex64 as C;

use super::worlds::{Action, Finish, Model, Superposition, Value, basis};

pub fn assert_snapshot(model: &Model<'_>, state: &Superposition, actual: &Executor<'_>) {
    let snapshot = actual.snapshot();
    assert_worlds(model, state, &snapshot);

    assert!((state.values().map(|a| a.norm_sqr()).sum::<f64>() - 1.0).abs() < 1e-11, "full-world norm");
}

/// Match each complete reference world group to one public executor world, in any order.
/// Histories remain in the reference grouping: equal visible marginals do not erase history.
/// Public snapshots omit private journals/dead slots; a cfg(test) adapter checks those separately.
pub fn assert_worlds(model: &Model<'_>, state: &Superposition, snapshot: &bennett_exec::Snapshot) {
    let mut groups = BTreeMap::<_, Superposition>::new();
    for (w, &a) in state {
        groups.entry((w.classical.clone(), w.history.clone())).or_default().insert(w.clone(), a);
    }
    let actual: Vec<_> = std::iter::once(snapshot).chain(&snapshot.worlds).collect();
    assert_eq!(groups.len(), actual.len(), "number of whole classical/history worlds");
    let mut used = vec![false; actual.len()];
    for group in groups.into_values() {
        let found = actual.iter().enumerate().position(|(i, s)| !used[i] && matches_group(model, &group, s));
        let Some(i) = found else {
            panic!("unmatched full-world group: {group:?}\nactual: {snapshot:?}");
        };
        used[i] = true;
    }
}

/// Whether `snapshot` is one of the complete world groups of `state`, renormalised: the world a host
/// observes when a run ends with several.
pub fn observed_one_of(model: &Model<'_>, state: &Superposition, snapshot: &bennett_exec::Snapshot) -> bool {
    let mut groups = BTreeMap::<_, Superposition>::new();
    for (w, &a) in state {
        groups.entry((w.classical.clone(), w.history.clone())).or_default().insert(w.clone(), a);
    }
    snapshot.worlds.is_empty()
        && groups.into_values().any(|group| {
            let norm = group.values().map(|a| a.norm_sqr()).sum::<f64>().sqrt();
            let group = group.into_iter().map(|(w, a)| (w, a / norm)).collect();
            matches_group(model, &group, snapshot)
        })
}

fn matches_group(model: &Model<'_>, state: &Superposition, snapshot: &bennett_exec::Snapshot) -> bool {
    let w = state.keys().next().expect("world group");
    let c = &w.classical;
    if c.cells.iter().map(|(&a, &id)| (a, id)).collect::<Vec<_>>() != snapshot.quantum.cells
        || model.memory(w) != snapshot.classical.memory
        || c.sp != snapshot.classical.stack_pointer
        || c.tape != snapshot.classical.tape
        || c.frames.len() != snapshot.classical.frames.len()
        || c.registers.len() != snapshot.quantum.registers.len()
    {
        return false;
    }
    for (f, a) in c.frames.iter().zip(&snapshot.classical.frames) {
        if (f.function, f.block, f.instruction, f.base, f.entry_sp)
            != (a.func, a.block, a.inst, a.base, a.entry_stack_pointer)
        {
            return false;
        }
    }
    for (v, &a) in c.slots.iter().zip(&snapshot.classical.slots) {
        let encoded = match v {
            Value::Int(n) => *n,
            Value::Register(id, g) => (u128::from(*g) << 32) | (*id as u128),
        };
        if encoded != a {
            return false;
        }
    }
    for (r, a) in c.registers.iter().zip(&snapshot.quantum.registers) {
        if r.width != a.width.map(|w| w.bits()) || r.generation != u64::from(a.generation) {
            return false;
        }
    }
    let outcome = match &c.finish {
        Some(Finish::Returned(n)) => Some(Outcome::Returned(*n)),
        Some(Finish::Faulted(f)) => Some(Outcome::Faulted(*f)),
        None | Some(Finish::BeforeMeasurement) => None,
    };
    if outcome != snapshot.classical.outcome {
        return false;
    }
    let columns = snapshot.quantum.registers.iter().rposition(|r| r.width.is_some()).map_or(0, |i| i + 1);
    let mut expected = BTreeMap::<Vec<u64>, C>::new();
    for (w, &a) in state {
        if w.quantum[columns..].iter().any(|&n| n != 0) {
            return false;
        }
        *expected.entry(w.quantum[..columns].to_vec()).or_default() += a;
    }
    expected
        .keys()
        .chain(snapshot.quantum.amplitudes.iter().map(|(c, _)| c))
        .all(|c| (expected.get(c).copied().unwrap_or_default() - snapshot.quantum.amplitude(c)).norm() < 1e-11)
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
            let next = model.step(&state);
            if next.len() > 64 {
                eprintln!("reference prefix stopped before more than 64 complete basis states at round {step}");
                return step;
            }
            state = next;
            actual.step();
        }
    }
    state = model.run(state, 0);
    actual.step();
    assert_snapshot(&model, &state, &actual);
    limit
}

/// Born instrument for completed finite programs. Each distinct classical/history world is
/// an observation channel; enumerate its register measurement outcomes independently of RNG.
/// This entry is deliberately bounded and does not prescribe strict-stop or tick policy.
pub fn observed_reference(model: &Model<'_>, input: Superposition, rounds: usize) -> Superposition {
    let boundary = model.run_joined(input, rounds);
    let mut groups = BTreeMap::<_, Superposition>::new();
    for (w, a) in boundary {
        groups.entry((w.classical.clone(), w.history.clone())).or_default().insert(w, a);
    }
    let mut result = Superposition::new();
    for group in groups.into_values() {
        if group.keys().all(|w| w.classical.finish != Some(Finish::BeforeMeasurement)) {
            for (w, a) in group {
                *result.entry(w).or_default() += a;
            }
            continue;
        }
        for (value, (weight, _)) in model.measurement_branches(&group) {
            let collapsed = model.collapse(&group, value);
            for (w, a) in observed_reference(model, collapsed, rounds) {
                *result.entry(w).or_default() += a * weight.sqrt();
            }
        }
    }
    result
}

/// A host sample must equal a normalized positive-weight complete world of the reference.
pub fn assert_observed(model: &Model<'_>, state: &Superposition, actual: &Executor<'_>) {
    let mut groups = BTreeMap::<_, Superposition>::new();
    for (w, &a) in state {
        groups.entry((w.classical.clone(), w.history.clone())).or_default().insert(w.clone(), a);
    }
    let snapshot = actual.snapshot();
    assert!(snapshot.worlds.is_empty(), "host returns one observed world");
    assert!(
        groups.into_values().any(|group| {
            let weight = group.values().map(|a| a.norm_sqr()).sum::<f64>();
            if weight <= 1e-15 {
                return false;
            }
            let normalized = group.into_iter().map(|(w, a)| (w, a / weight.sqrt())).collect();
            matches_group(model, &normalized, &snapshot)
        }),
        "observed world is absent from independent Born support: {snapshot:?}"
    );
}

/// Opt-in expanded-suite adapter. The original test still executes its full arguments/limits.
/// Cost/infinite cases have explicit small semantic probes instead of pretending to finish
/// the original unbounded workload. Simulator policy remains with the original assertions.
pub fn compare_world_entry(program: &ValidModule, entry: &str, args: &[u64]) -> bool {
    let reads_registers = program.functions.iter().any(|f| {
        f.blocks.iter().any(|b| {
            b.insts.iter().any(|inst| {
                let (args, params): (_, Vec<_>) = match inst {
                    Inst::Call { callee, args, .. } => {
                        (args, program.function(*callee).params.iter().map(|p| p.ty).collect())
                    }
                    Inst::CallIndirect { signature, args, .. } => (args, signature.params.clone()),
                    _ => return false,
                };
                args.iter().zip(params).any(|(arg, ty)| {
                    if !matches!(ty, bennett_ir::Type::Int(_)) {
                        return false;
                    }
                    let bennett_ir::Operand::Slot(slot) = arg else {
                        return false;
                    };
                    let func = program.function_named(&f.name).expect("function");
                    matches!(program.slot_type(func, *slot), bennett_ir::Type::Qreg(_))
                })
            })
        })
    });
    if !reads_registers {
        return false;
    }
    // This is the existing explicit two-site schedule-question witness; it is exercised
    // separately by documented_stops_fault_scope_and_tick_budget_boundaries.
    if entry == "unaligned" {
        eprintln!("worlds reference @{entry}: explicit Unaligned policy probe");
        return true;
    }
    let args: Vec<_> = args.iter().map(|&a| a.min(3)).collect();
    let model = Model::new(program, entry);
    let initial = model.initial(entry, &args.iter().map(|&a| u128::from(a)).collect::<Vec<_>>());
    let reference = observed_reference(&model, basis(initial), 10000);
    let mut actual = Executor::new(
        program,
        program.function_named(entry).expect("entry"),
        &args,
        Options { reversible: true, allow_region_faults: true, budget: Some(10000), ..Options::default() },
    )
    .expect("reference companion starts");
    actual.run();
    assert_observed(&model, &reference, &actual);
    eprintln!("worlds joined reference @{entry}{args:?}: {} ticks", actual.stats().steps);
    true
}
