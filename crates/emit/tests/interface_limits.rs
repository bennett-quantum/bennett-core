//! The gate compiler's interface takes registers as the device language allows them: a fault register of
//! from 5 qubits to the widest register, 2^32 - 1 qubits, and no wider.

use bennett_emit::compile_apply;
use bennett_exec::{ApplyError, ApplyInput};

#[test]
fn a_fault_register_wider_than_the_widest_register_is_refused() {
    let module = bennett_ir::load("func @id() -> i1 {\nentry:\n    ret 0\n}\n").expect("valid");
    let compile = |width: u64| {
        compile_apply(ApplyInput {
            module: &module,
            callee: module.function_named("id").expect("the callee"),
            args: &[],
            out: 0..1,
            fault: 1..1 + width,
            budget: 4,
            entry_fault: None,
            stack_available: 256,
            memory: None,
            scratch_start: 1 + width,
        })
        .map(|_| ())
    };
    let widest = u64::from(bennett_ir::Qubits::MAX);
    assert_eq!(compile(widest), Ok(()));
    assert_eq!(compile(widest + 1), Err(ApplyError::Interface));
    assert_eq!(compile(4), Err(ApplyError::Interface));
    assert_eq!(compile(5), Ok(()));
}
