use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;

/// Exhaust the ENTIRE supplied finite domain, including history/marks/control state in S.
/// Input and output domains may differ (e.g. push/pop a bounded history record) but must
/// have the same cardinality. No assumption that an involution is therefore injective.
/// The caller must enumerate all encodings it claims are legal, not just reachable clean states.
pub fn check_bijection<S: Ord + Clone + Debug>(
    inputs: &[S],
    outputs: &[S],
    mut step: impl FnMut(&S) -> S,
) -> Result<usize, String> {
    let source: BTreeSet<_> = inputs.iter().cloned().collect();
    let target: BTreeSet<_> = outputs.iter().cloned().collect();
    if source.len() != inputs.len() || target.len() != outputs.len() || inputs.len() != outputs.len() {
        return Err("domains must have distinct states and equal cardinality".into());
    }
    let mut preimages = BTreeMap::new();
    for input in inputs {
        let output = step(input);
        if !target.contains(&output) {
            return Err(format!("not closed: {input:?} -> {output:?}"));
        }
        if let Some(previous) = preimages.insert(output.clone(), input) {
            return Err(format!("collision: {previous:?} and {input:?} -> {output:?}"));
        }
    }
    if preimages.len() != target.len() {
        return Err("not surjective".into());
    }
    Ok(preimages.len())
}

/// Cartesian product, least significant field first. Include every history word as a field.
pub fn words(widths: &[u32]) -> Vec<Vec<u64>> {
    let bits: u32 = widths.iter().sum();
    assert!(bits <= 16, "this checker deliberately exhausts only small spaces");
    (0..1u64 << bits)
        .map(|mut packed| {
            widths
                .iter()
                .map(|&w| {
                    let value = packed & ((1 << w) - 1);
                    packed >>= w;
                    value
                })
                .collect()
        })
        .collect()
}
