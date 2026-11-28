//! LLVM's relative loads (`front/src/RelativeLoads.h`): clang's optimiser makes a table of pointers to constants in
//! position-independent code into a table of 32-bit offsets read by `llvm.load.relative`, which `bennett-front` writes
//! out as an address computation, a 32-bit load, a sign extension and a sum. `bq_fault_name` of `bq.h` is such a
//! table, so at `-O2` every program that reads a fault name has one; before the lowering `bennett-front` stopped at it
//! ("not handled yet: the intrinsic @llvm.load.relative.i64").
use bennett_exec::{Executor, Outcome};
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, entry, front, options, out_dir, write_c};
use std::process::Command;

/// The names `bq_fault_name` gives the codes 0 to 25 (`front/include/bq.h`).
const NAMES: [&str; 26] = [
    "none",
    "budget",
    "divide_by_zero",
    "out_of_bounds",
    "bad_indirect",
    "unreachable",
    "stack_overflow",
    "effect_in_apply",
    "quantum_in_apply",
    "bad_handle",
    "control_in_use",
    "unused",
    "bad_stack_restore",
    "exit_in_apply",
    "unused",
    "unused",
    "bad_longjmp",
    "qubit_memory_access",
    "bad_angle",
    "out_of_memory",
    "bad_width",
    "out_of_handles",
    "bad_range",
    "bad_gate",
    "bad_argument",
    "unused",
];

/// Each fault name, printed through the output primitive, at `-O0` and `-O2`; at `-O2` clang's output has the
/// relative table, so the test keeps covering it.
#[test]
fn a_table_of_string_pointers_is_read_at_every_level() {
    let source = write_c(
        "relative_loads",
        "#include \"bq.h\"\n\
         int main(int code) {\n\
           const char *name = bq_fault_name((uint64_t)code);\n\
           int n = 0;\n\
           for (; *name; name++, n++) __bennett_effect(*name);\n\
           return n;\n\
         }\n",
    );
    for (level, flags) in [("O0", CLANG_FLAGS), ("O2", CLANG_O2_FLAGS)] {
        let name = format!("relative_loads.{level}");
        let unit = out_dir().join(format!("{name}.ll"));
        let compiled = Command::new(front().with_file_name("bennett-cc"))
            .args(flags)
            .arg("-o")
            .arg(&unit)
            .arg(&source)
            .output()
            .expect("bennett-cc");
        assert!(compiled.status.success(), "{name}: {}", String::from_utf8_lossy(&compiled.stderr));
        let text = std::fs::read_to_string(&unit).expect("unit");
        if level == "O2" {
            assert!(text.contains("@llvm.load.relative"), "{name}: clang made no relative table");
        }
        let device = out_dir().join(format!("{name}.bn"));
        let lowered = Command::new(front())
            .arg(&unit)
            .arg("-o")
            .arg(&device)
            .args(["--region-budget=64", "--stack-bytes=4096"])
            .output()
            .expect("bennett-front");
        assert!(lowered.status.success(), "{name}: {}", String::from_utf8_lossy(&lowered.stderr));
        let module = bennett_ir::load(&std::fs::read_to_string(&device).expect("device")).expect("valid");
        for (code, expected) in NAMES.iter().enumerate() {
            let mut executor =
                Executor::new(&module, entry(&module, "main"), &[code as u64], options(0)).expect("starts");
            let outcome = executor.run();
            assert_eq!(outcome, Outcome::Returned(Some(expected.len() as u128)), "{name} code {code}");
            assert_eq!(executor.tape(), expected.as_bytes(), "{name} code {code}");
        }
    }
}
