//! Berkeley SoftFloat 3e, the floating point of a device without floating-point hardware, against this
//! machine's: `front/tests/c/softfloat_exact.c`, built natively with the vendored sources
//! (`rt/softfloat/berkeley-softfloat-3e`, specialisation 8086-SSE) and compared bit for bit with SSE
//! (float, double) and x87 (long double) on directed operands and a fixed-seed random sample.
//!
//! The differences it accepts, each a class of its own in the program's report:
//! - `fma-invalid-nan`: `fma(0, inf, NaN)` gives the quieted NaN addend on x86 and the default NaN in
//!   SoftFloat.
//! - `f64_rem` against glibc's `remainder` (not an instruction): glibc returns the second NaN of two
//!   and, for some subnormal divisors, -0 where IEEE 754 requires +0.
//! - extF80 encodings that no arithmetic produces: pseudo-denormals (x87 reads them as values; SoftFloat
//!   gets some sums and comparisons wrong) and unnormals, pseudo-infinities and pseudo-NaNs (x87 rejects
//!   them as invalid operands; SoftFloat reads them as numbers or NaNs).
//! - to-integer conversions whose result is undefined in C (out of range, NaN).
//!
//! Longer runs (10^7 random operands per operation, all 2^32 float inputs of the unary operations) take
//! arguments: `softfloat_exact <random-per-op> <threads> 1`.

#![cfg(target_arch = "x86_64")]

use std::path::{Path, PathBuf};
use std::process::Command;

use bennett_front_tests::{c_dir, out_dir, root};

/// The vendored release.
fn softfloat() -> PathBuf {
    root().join("rt/softfloat/berkeley-softfloat-3e")
}

fn clang() -> PathBuf {
    std::env::var_os("CLANG").map_or_else(|| PathBuf::from("clang"), PathBuf::from)
}

/// The defines of SoftFloat's own `build/Linux-x86_64-GCC/Makefile` (`SOFTFLOAT_OPTS` and the compile
/// line): 64-bit integer arithmetic, round-to-odd support, inlined primitives.
const DEFINES: &[&str] = &[
    "-DSOFTFLOAT_FAST_INT64",
    "-DSOFTFLOAT_ROUND_ODD",
    "-DINLINE_LEVEL=5",
    "-DSOFTFLOAT_FAST_DIV32TO16",
    "-DSOFTFLOAT_FAST_DIV64TO32",
];

/// The sources the Makefile builds: every `name$(OBJ)` it lists, found in `source/` or in the
/// specialisation.
fn sources() -> Vec<PathBuf> {
    let makefile = softfloat().join("build/Linux-x86_64-GCC/Makefile");
    let text = std::fs::read_to_string(&makefile).unwrap_or_else(|e| panic!("cannot read {makefile:?}: {e}"));
    let names = text
        .split_whitespace()
        .filter_map(|word| word.strip_suffix("$(OBJ)"))
        .filter(|name| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    let mut files: Vec<PathBuf> = names
        .map(|name| {
            let file = format!("{name}.c");
            [softfloat().join("source").join(&file), softfloat().join("source/8086-SSE").join(&file)]
                .into_iter()
                .find(|path| path.exists())
                .unwrap_or_else(|| panic!("the Makefile lists {name}, which is in neither source directory"))
        })
        .collect();
    files.sort();
    files.dedup();
    files
}

fn run_tool(command: &mut Command) {
    let output = command.output().unwrap_or_else(|e| panic!("cannot run {command:?}: {e}"));
    assert!(output.status.success(), "{command:?} failed:\n{}", String::from_utf8_lossy(&output.stderr));
}

/// Compiles the library's sources to objects in `dir`, in parallel.
fn compile_library(dir: &Path) -> Vec<PathBuf> {
    let sources = sources();
    let includes = [
        softfloat().join("build/Linux-x86_64-GCC"),
        softfloat().join("source/8086-SSE"),
        softfloat().join("source/include"),
    ];
    let objects: Vec<PathBuf> =
        sources.iter().map(|source| dir.join(source.file_stem().unwrap_or_default()).with_extension("o")).collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let jobs: Vec<(&PathBuf, &PathBuf)> = sources.iter().zip(&objects).collect();
    std::thread::scope(|scope| {
        for chunk in jobs.chunks(jobs.len().div_ceil(threads)) {
            let includes = &includes;
            scope.spawn(move || {
                for (source, object) in chunk {
                    let mut command = Command::new(clang());
                    command.args(["-c", "-O2", "-Werror-implicit-function-declaration"]).args(DEFINES);
                    for include in includes {
                        command.arg("-I").arg(include);
                    }
                    run_tool(command.arg(source).arg("-o").arg(object));
                }
            });
        }
    });
    objects
}

#[test]
fn softfloat_is_bit_exact_against_sse_and_x87_except_the_known_classes() {
    let dir = out_dir().join("softfloat");
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("cannot create {dir:?}: {e}"));
    let objects = compile_library(&dir);
    assert!(objects.len() > 250, "the Makefile lists only {} sources", objects.len());

    let program = dir.join("softfloat_exact");
    run_tool(
        Command::new(clang())
            .args(["-O2", "-ffp-contract=off", "-fno-fast-math", "-DSOFTFLOAT_FAST_INT64", "-I"])
            .arg(softfloat().join("source/include"))
            .arg(c_dir().join("softfloat_exact.c"))
            .args(&objects)
            .args(["-lm", "-lpthread", "-o"])
            .arg(&program),
    );

    let output = Command::new(&program).output().unwrap_or_else(|e| panic!("cannot run {program:?}: {e}"));
    let report = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "unexpected differences from hardware:\n{report}");
    assert!(report.contains("unexpected mismatch classes: 0"), "{report}");

    // The known differences still show, so that a change in either side is noticed.
    let line = |op: &str| {
        report
            .lines()
            .find(|line| line.split_whitespace().next() == Some(op))
            .unwrap_or_else(|| panic!("no line for {op}:\n{report}"))
    };
    for (op, class) in [
        ("f32_fma", "fma-invalid-nan"),
        ("f64_fma", "fma-invalid-nan"),
        ("x80_add", "pseudo-denormal-value"),
        ("x80_add", "unsupported-value"),
        ("x80_oeq", "pseudo-denormal-value"),
    ] {
        assert!(line(op).contains(class), "{op} no longer shows {class}:\n{report}");
    }
    // Everything on canonical operands outside fma and remainder is exact, NaN bits included.
    for op in ["f32_add", "f32_div", "f32_sqrt", "f64_mul", "f64_sqrt", "f64_trunc", "f64_ole", "f32_f64"] {
        assert!(line(op).ends_with(" ok"), "{op}: {}", line(op));
    }
    for op in ["x80_add", "x80_mul", "x80_div", "x80_sqrt", "x80_f64", "f64_x80", "ui64_x80"] {
        let line = line(op);
        assert!(!line.contains(" value=") && !line.contains(" nan-bits="), "{line}");
    }
}
