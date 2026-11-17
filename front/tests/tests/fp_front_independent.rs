//! Independent end-to-end native comparisons for the fp-front changes. Keep exact NaN bits too:
//! these unary operations have no competing NaN operands. Native optimization levels are separate oracles.
#![cfg(target_arch = "x86_64")]

use std::process::Command;
use std::sync::OnceLock;

use bennett_exec::Outcome;
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, c_dir, lower, native_build_with, run};
use bennett_ir::ValidModule;

struct Probe {
    opt: &'static str,
    module: ValidModule,
    native: std::path::PathBuf,
}

fn probes() -> &'static [Probe; 2] {
    static PROBES: OnceLock<[Probe; 2]> = OnceLock::new();
    PROBES.get_or_init(|| {
        [("-O0", CLANG_FLAGS), ("-O2", CLANG_O2_FLAGS)].map(|(opt, flags)| {
            let source = c_dir().join("fp_front_independent.c");
            let name = format!("fp-front-independent{opt}");
            // Admit lrint/lround as intrinsics, without the unrelated errno/FENV library fallback.
            let mut flags = flags.to_vec();
            flags.push("-fno-math-errno");
            Probe {
                opt,
                module: lower(&source, &name, &flags, &[]),
                native: native_build_with(&source, &name, &[opt, "-DNATIVE", "-fno-math-errno"], &["-lm"]),
            }
        })
    })
}

fn compare(mode: &str, entry: &str, cases: &[Vec<u64>]) {
    let mut all_differences = Vec::new();
    for probe in probes() {
        let output = Command::new(&probe.native).arg(mode).output().expect("native oracle runs");
        assert!(output.status.success(), "native {} {mode}: {:?}", probe.opt, output.status);
        let expected: Vec<u128> = String::from_utf8(output.stdout)
            .expect("hex output")
            .lines()
            .map(|s| u128::from_str_radix(s, 16).expect("native bits"))
            .collect();
        assert_eq!(expected.len(), cases.len(), "complete, nonempty native coverage");
        assert!(!cases.is_empty());
        let mut differences = Vec::new();
        let mut excluded = 0;
        for (args, expected) in cases.iter().zip(expected) {
            // Exactly the previously measured differing tuples, each with an ignored minimal witness
            // below. These are exclusions, never counted as successful comparisons; other inputs to
            // the same functions (canonical boundaries included) remain checked.
            if known_difference(mode, probe.opt, args) {
                if mode == "classification" {
                    // Pin both sides of the native bug; do not lose the correct device sign check.
                    assert_eq!(expected, 0xa8_0026);
                    assert_eq!(run(&probe.module, entry, args, 0), Outcome::Returned(Some(0x1_0057_0093)));
                }
                excluded += 1;
                continue;
            }
            let actual = run(&probe.module, entry, args, 0);
            if actual != Outcome::Returned(Some(expected)) {
                differences.push(format!("{} {entry}{args:?}: {actual:?}, native {expected:020x}", probe.opt));
            }
        }
        if !differences.is_empty() {
            all_differences.extend(differences);
        }
        eprintln!(
            "{} {entry}: {} exact native/device results; {excluded} witnessed exclusions",
            probe.opt,
            cases.len() - excluded
        );
    }
    assert!(all_differences.is_empty(), "{} differences:\n{}", all_differences.len(), all_differences.join("\n"));
}

fn known_difference(mode: &str, opt: &str, args: &[u64]) -> bool {
    if mode == "classification" {
        // LLVM 18's native optimized bfloat signbit returns false even for -1, unlike its O0 code.
        return opt == "-O2" && args[0] == 4 && args[1] >= 18 && args[2] == 5;
    }
    false
}

#[test]
fn powi_every_encoding_class_and_extreme_exponents_match_native() {
    let mut cases = Vec::new();
    for (w, n) in [(0, 36), (1, 36), (2, 60)] {
        for i in 0..n {
            for p in 0..16 {
                cases.push(vec![w, i, p]);
            }
        }
    }
    compare("powers", "power_result", &cases);
}

#[test]
fn rounding_noncanonical_encodings_and_all_binary_points_match_native() {
    let cases: Vec<Vec<u64>> = (0..2 * (17 * 12 + 63 * 3)).flat_map(|i| (0..11).map(move |op| vec![i, op])).collect();
    compare("rounding", "rounding_result", &cases);
}

#[test]
fn classification_macros_branches_and_selects_match_native_at_every_width() {
    let mut cases = Vec::new();
    for (w, n) in [(0, 36), (1, 36), (2, 60), (3, 36), (4, 36)] {
        for i in 0..n {
            for op in 0..6 {
                cases.push(vec![w, i, op]);
            }
        }
    }
    compare("classification", "class_result", &cases);
}

fn witness(entry: &str) {
    let source = c_dir().join("fp_front_witness.c");
    let mut differences = Vec::new();
    for (opt, flags) in [("-O0", CLANG_FLAGS), ("-O2", CLANG_O2_FLAGS)] {
        let name = format!("fp-front-witness-{entry}{opt}");
        let native = native_build_with(&source, &name, &[opt, "-DNATIVE", "-fno-math-errno"], &["-lm"]);
        let output = Command::new(native).output().expect("native witness runs");
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).expect("native text");
        let native: Vec<u128> = text.split_whitespace().map(|word| word.parse().expect("native integer")).collect();
        assert_eq!(native.len(), 3);
        let mut flags = flags.to_vec();
        flags.push("-fno-math-errno");
        let module = lower(&source, &name, &flags, &[]);
        let expected = if entry == "bf_signbit" && opt == "-O0" { 1 } else { 0 };
        let column = match entry {
            "bf_signbit" => 0,
            "unnormal_lround" => 1,
            "unnormal_llround" => 2,
            _ => panic!("unknown witness {entry}"),
        };
        assert_eq!(native[column], expected, "native {entry} changed");
        let args = if entry == "bf_signbit" { vec![0xbf80] } else { vec![0, 0x3fff] };
        let actual = run(&module, entry, &args, 0);
        if actual != Outcome::Returned(Some(expected)) {
            differences.push(format!("{opt} {entry}: {actual:?}, native {expected}"));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn lroundl_unnormal_zero_matches_glibc_software() {
    // glibc's software integer rounding reads the significand (zero) directly. The front composed roundl
    // with an x87-style integer conversion, which rejects the unsupported encoding (INT64_MIN), until
    // 2026-11-17; it now calls the runtime's lroundl, which follows glibc.
    witness("unnormal_lround");
}

#[test]
fn llroundl_unnormal_zero_matches_glibc_software() {
    witness("unnormal_llround");
}

#[test]
#[ignore = "defect: native-reference divergence: LLVM 18 O2 bfloat signbit returns zero for negative one"]
fn bfloat_signbit_matches_the_optimized_native_reference() {
    // 0xbf80 is exactly -1. The mathematically correct sign bit is 1 (and the device and native O0
    // return 1). Native O2's generated code loads the word and then returns 0 unconditionally. This
    // witness records target disagreement, not a recommendation to copy that native compiler bug.
    witness("bf_signbit");
}
