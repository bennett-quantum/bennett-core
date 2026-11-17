//! Independent regression probes for noncanonical operands and minimal ignored defect witnesses.
#![cfg(target_arch = "x86_64")]

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use bennett_exec::Outcome;
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, c_dir, clang, float_runtime, lower, native_build_with, out_dir, run, write_c,
};

fn tool(command: &mut Command) -> std::process::Output {
    let output = command.output().unwrap_or_else(|e| panic!("{command:?}: {e}"));
    assert!(output.status.success(), "{command:?}: {}", String::from_utf8_lossy(&output.stderr));
    output
}

fn matrix() -> &'static [PathBuf; 2] {
    static PROGRAMS: OnceLock<[PathBuf; 2]> = OnceLock::new();
    PROGRAMS.get_or_init(|| {
        let dir = out_dir().join("p0-independent");
        std::fs::create_dir_all(&dir).expect("test output directory");
        let object = dir.join("runtime.o");
        let prefixed = dir.join("prefixed.o");
        tool(Command::new(clang()).args(["-O2", "-c"]).arg(float_runtime()).arg("-o").arg(&object));
        tool(Command::new("objcopy").arg("--prefix-symbols=rt_").arg(&object).arg(&prefixed));
        ["-O0", "-O2"].map(|opt| {
            let program = dir.join(format!("matrix{opt}"));
            tool(
                Command::new(clang())
                    .args([opt, "-ffp-contract=off", "-fno-builtin-fminl", "-fno-builtin-fmaxl"])
                    .arg(c_dir().join("p0_x87_matrix.c"))
                    .arg(&prefixed)
                    .args(["-lm", "-o"])
                    .arg(&program),
            );
            program
        })
    })
}

#[test]
fn expanded_x87_matrix_and_integer_boundaries_agree_at_both_optimization_levels() {
    for program in matrix() {
        for mode in ["matrix", "powers", "integers", "floating"] {
            let output = Command::new(program).arg(mode).output().expect("native probe executes");
            let report = String::from_utf8_lossy(&output.stdout);
            assert!(output.status.success(), "{program:?} {mode}: {report}");
            assert!(report.contains("mismatches=0"), "{report}");
            eprintln!("{program:?}: {report}");
        }
    }
}

fn witness(entry: &str) {
    let source = c_dir().join("p0_x87_witness.c");
    let mut differences = Vec::new();
    for (opt, flags) in [("-O0", CLANG_FLAGS), ("-O2", CLANG_O2_FLAGS)] {
        let native = native_build_with(&source, &format!("p0-witness-{entry}{opt}"), &[opt, "-DNATIVE"], &["-lm"]);
        let output = tool(&mut Command::new(native));
        assert_eq!(String::from_utf8_lossy(&output.stdout), "0 0 0 0 0 0 0 0\n", "native witness changed");
        let module = lower(&source, &format!("p0-witness-{entry}{opt}"), flags, &[]);
        let actual = run(&module, entry, &[], 0);
        if actual != Outcome::Returned(Some(0)) {
            differences.push(format!("{entry} {opt}: {actual:?} versus native 0"));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn powi_one_preserves_a_pseudo_denormal_encoding() {
    witness("powi_preserves_pseudo");
}

#[test]
fn powi_one_preserves_an_unsupported_encoding() {
    witness("powi_preserves_unsupported");
}

#[test]
#[ignore = "defect: roundevenl of a tiny unnormal gives NaN instead of native glibc's zero"]
fn roundeven_of_an_unnormal_matches_native_software() {
    witness("roundeven_unnormal");
}

#[test]
fn raw_encodings_survive_the_complete_pipeline_at_both_optimization_levels() {
    let source = c_dir().join("p0_x87_pipeline.c");
    for (opt, flags) in [("-O0", CLANG_FLAGS), ("-O2", CLANG_O2_FLAGS)] {
        let native = native_build_with(&source, &format!("p0-pipeline{opt}"), &[opt, "-DNATIVE"], &["-lm"]);
        let output = tool(&mut Command::new(native));
        let expected: Vec<u128> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| u128::from_str_radix(line, 16).expect("native hex bits"))
            .collect();
        assert_eq!(expected.len(), 4640);
        let mut flags = flags.to_vec();
        flags.push("-fno-math-errno");
        let module = lower(&source, &format!("p0-pipeline{opt}"), &flags, &[]);
        assert_eq!(run(&module, "results", &[], 0), Outcome::Returned(Some(expected.len() as u128)));
        let mut different = Vec::new();
        for (i, expected) in expected.iter().enumerate() {
            let unary_op = i / 20;
            let operand = i % 20;
            // Noncanonical aggregate class tests have minimal ignored witnesses below. Canonical
            // classifications, explicit per-class masks and fpclassify remain in this passing sweep.
            if unary_op == 21 && operand < 14 && opt == "-O0" {
                let Outcome::Returned(Some(actual)) = run(&module, "result", &[i as u64], 0) else {
                    panic!("classification did not return");
                };
                // Only isnan (bit 0) and isfinite (bit 2) are known defects. Check every other
                // predicate here so that those witnesses cannot hide a different classification bug.
                assert_eq!(actual & !5, expected & !5, "{opt} class {operand}: {actual:x} versus {expected:x}");
                continue;
            }
            // Native libgcc half conversion reads noncanonical bits in software (existing debt).
            // The remaining half conversions and reuse of their results are compared exactly.
            if unary_op == 30 && (4..14).contains(&operand) {
                continue;
            }
            // Undefined C conversions: this native compiler gives 0 at O0 and 2^63 at O2 for
            // unsupported/NaN -> u64, while the runtime uses the O0 sequence. Measure and pin
            // both values explicitly, without counting this permitted difference as agreement.
            if opt == "-O2" && unary_op == 16 && ((4..14).contains(&operand) || operand >= 18) {
                assert_eq!(*expected, 1 << 63, "native undefined conversion changed");
                assert_eq!(run(&module, "result", &[i as u64], 0), Outcome::Returned(Some(0)));
                continue;
            }
            let actual = run(&module, "result", &[i as u64], 0);
            if actual != Outcome::Returned(Some(*expected)) {
                different.push(format!("{opt} result {i}: {actual:?}, native {expected:020x}"));
            }
        }
        assert!(different.is_empty(), "{} differences:\n{}", different.len(), different.join("\n"));
    }
}

#[test]
#[ignore = "defect: isfinite returns false for an x87 pseudo-denormal that native treats as finite"]
fn a_pseudo_denormal_is_finite() {
    witness("pseudo_is_finite");
}

#[test]
#[ignore = "defect: isnan returns false for an unsupported x87 encoding that native treats as NaN"]
fn an_unsupported_encoding_is_nan() {
    witness("unnormal_is_nan");
}

#[test]
#[ignore = "defect: roundl of a tiny unnormal gives NaN instead of native glibc's zero (known debt)"]
fn round_of_an_unnormal_matches_native_software() {
    witness("round_unnormal");
}

#[test]
#[ignore = "defect: unnormal to half gives NaN instead of native libgcc's zero (known debt)"]
fn half_conversion_of_an_unnormal_matches_native_software() {
    witness("half_unnormal");
}

#[test]
#[ignore = "defect: unnormal to bfloat gives NaN instead of native libgcc's zero (known debt)"]
fn bfloat_conversion_of_an_unnormal_matches_native_software() {
    witness("bfloat_unnormal");
}

#[test]
fn every_supported_integer_width_converts_to_and_from_long_double() {
    // Include all _BitInt widths, not just the C ABI's usual 8/16/32/64/128. All floating-to-integer
    // inputs here are representable after truncation; undefined out-of-range cases live in the matrix.
    let mut text = String::from(
        "typedef unsigned __int128 U;\n\
         static volatile U high = ((U)0xb5913a5be924769fULL << 64) | 0x13579bdf87ace024ULL;\n\
         static volatile U low = ((U)0x2468ace013579bdfULL << 64) | 0xdeadbeef456789abULL;\n\
         static volatile long double positive = 1.75L, negative = -1.75L;\n\
         static U bits(long double x) { U u=0; __builtin_memcpy(&u,&x,10); return u; }\n\
         U result(unsigned i) { unsigned mode=i/128; switch(i%128) {\n",
    );
    for width in 1..=128 {
        let signed = if width == 1 { "unsigned " } else { "" };
        let negative = if width == 1 { "positive" } else { "negative" };
        text.push_str(&format!(
            "case {}: {{ typedef unsigned _BitInt({width}) UW; typedef {signed}_BitInt({width}) SW;\n\
             if(mode==0) return bits((long double)(UW)high);\n\
             if(mode==1) return bits((long double)(UW)low);\n\
             if(mode==2) return (U)(UW)positive;\n\
             if(mode==3) return bits((long double)(UW)positive);\n\
             if(mode==4) return bits((long double)(SW)high);\n\
             if(mode==5) return bits((long double)(SW)low);\n\
             if(mode==6) return (U)(SW){negative};\n\
             return bits((long double)(SW){negative}); }}\n",
            width - 1,
        ));
    }
    text.push_str(
        "} return 0; }\n#ifdef NATIVE\nint printf(const char *,...);\n\
         int main(void) { for(unsigned i=0;i<1024;i++) { U u=result(i);\n\
         printf(\"%016llx%016llx\\n\",(unsigned long long)(u>>64),(unsigned long long)u); } return 0; }\n#endif\n",
    );
    let source = write_c("p0-all-integer-widths", &text);
    for (opt, flags) in [("-O0", CLANG_FLAGS), ("-O2", CLANG_O2_FLAGS)] {
        let name = format!("p0-all-integer-widths{opt}");
        let native = native_build_with(&source, &name, &[opt, "-DNATIVE"], &[]);
        let output = tool(&mut Command::new(native));
        // One dispatch includes every width: reserve its large generated SSA frame explicitly.
        let module = lower(&source, &name, flags, &["--stack-bytes=1048576"]);
        let expected: Vec<u128> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| u128::from_str_radix(line, 16).expect("native integer bits"))
            .collect();
        assert_eq!(expected.len(), 1024);
        for (i, value) in expected.into_iter().enumerate() {
            assert_eq!(run(&module, "result", &[i as u64], 0), Outcome::Returned(Some(value)), "{opt} case {i}");
        }
    }
}
