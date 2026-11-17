//! Attribute policies checked against executed native code, at both compiler inputs.
use std::process::Command;

use bennett_exec::Outcome;
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, bitcode, c_dir, lower, lower_text, native_build_with, run};

const ABI_INPUTS: &[u64] = &[0, 1, 127, 128, 255, 256, 0x0123_4567_89ab_cdef, u64::MAX];
const FP_INPUTS: &[u64] = &[
    1,
    0x8000_0000_0000_0001,
    0x0010_0000_0000_0000,
    0x8010_0000_0000_0000,
    0x0080_0001,
    0x8000_0001,
    0x0080_0000,
    0x8080_0000,
];

fn compare(name: &str, tag: &str, shapes: usize, inputs: &[u64], flags: &[&str], native_extra: &[&str]) {
    let source = c_dir().join(if name.ends_with(".cpp") { name.to_owned() } else { format!("{name}.c") });
    let mut differences = Vec::new();
    for (level, base) in [("o0", CLANG_FLAGS), ("o2", CLANG_O2_FLAGS)] {
        let mut native_flags = vec![if level == "o0" { "-O0" } else { "-O2" }, "-DNATIVE"];
        native_flags.extend(flags);
        native_flags.extend(native_extra);
        let exe = native_build_with(&source, &format!("attrs.{tag}.{level}"), &native_flags, &[]);
        let output = Command::new(exe).output().expect("execute native attribute witness");
        assert!(output.status.success(), "native {tag} {level}");
        let words: Vec<u64> = String::from_utf8(output.stdout)
            .expect("native text")
            .lines()
            .map(|s| u64::from_str_radix(s, 16).expect("native bits"))
            .collect();
        assert_eq!(words.len(), shapes * inputs.len());
        let mut device_flags = base.to_vec();
        device_flags.extend(flags);
        let module = lower(&source, &format!("attrs.{tag}.{level}"), &device_flags, &[]);
        for (i, &expected) in words.iter().enumerate() {
            let actual = run(&module, "result", &[(i / inputs.len()) as u64, inputs[i % inputs.len()]], 0);
            if actual != Outcome::Returned(Some(u128::from(expected))) {
                differences.push(format!(
                    "{level} shape {} input {:#x}: native {expected:#x}, device {actual:?}",
                    i / inputs.len(),
                    inputs[i % inputs.len()]
                ));
            }
        }
        eprintln!("{tag} {level}: {} native/device words", words.len());
    }
    assert!(differences.is_empty(), "{tag}: {} mismatches:\n{}", differences.len(), differences.join("\n"));
}

#[test]
fn enum_abi_and_valid_pointer_promises_preserve_native_results() {
    compare("attribute_semantics", "abi", 7, ABI_INPUTS, &[], &[]);
    compare("attribute_references.cpp", "references", 3, ABI_INPUTS, &["-fno-exceptions", "-fno-rtti"], &[]);
}

#[test]
fn no_builtin_strings_preserve_a_programs_own_definition() {
    compare(
        "attribute_semantics",
        "no-builtins",
        8,
        ABI_INPUTS,
        &["-DNO_BUILTIN", "-fno-builtin", "-fno-builtin-abort"],
        &[],
    );
    compare(
        "attribute_semantics",
        "no-builtin-abort",
        8,
        ABI_INPUTS,
        &["-DNO_ABORT_BUILTIN", "-fno-builtin-abort"],
        &[],
    );
}

#[test]
#[ignore = "needs the separately built musl library; run after rt/musl/build.py --jobs 3"]
fn allocation_attributes_preserve_distinct_live_objects_with_musl() {
    use bennett_exec::Executor;
    use bennett_front_tests::{entry, front, options, out_dir, root};
    let source = c_dir().join("attribute_allocator.c");
    for level in ["-O0", "-O2"] {
        let tag = format!("attrs.allocator.{level}");
        let exe = native_build_with(&source, &tag, &[level], &[]);
        let expected = Command::new(exe).output().expect("native allocation oracle");
        assert!(expected.status.success());
        assert_eq!(String::from_utf8_lossy(&expected.stdout).lines().count(), 16);
        let program = out_dir().join(format!("{tag}.bn"));
        let output = Command::new("python3")
            .arg(root().join("rt/musl/link.py"))
            .arg(&source)
            .arg("--front")
            .arg(front())
            .arg("--keep")
            .arg(out_dir().join(&tag))
            .arg("-o")
            .arg(&program)
            .arg(format!("--clang-flag={level}"))
            .output()
            .expect("musl link");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let module = bennett_ir::load(&std::fs::read_to_string(program).expect("device text")).expect("valid module");
        let mut executor =
            Executor::new(&module, entry(&module, "__bennett_entry"), &[], options(0)).expect("executor");
        assert_eq!(executor.run(), Outcome::Returned(Some(0)), "{tag}");
        assert_eq!(executor.tape(), expected.stdout, "{tag}");
        eprintln!("{tag}: 16 native/device allocation hashes, {} steps", executor.stats().steps);
    }
}

#[test]
fn ieee_denormals_and_no_trapping_math_preserve_native_bits() {
    for (tag, flags) in [
        ("ieee", &[][..]),
        ("ieee-explicit", &["-fdenormal-fp-math=ieee,ieee"][..]),
        ("no-trapping", &["-fno-trapping-math"][..]),
        ("fast-finite", &["-fno-signed-zeros", "-fno-honor-nans", "-fno-honor-infinities", "-freciprocal-math"]),
    ] {
        compare("attribute_fp", tag, 6, FP_INPUTS, flags, &[]);
    }
}

/// A function promising a subnormal mode other than IEEE is refused by name at its first operation whose result
/// depends on the mode: SoftFloat keeps subnormals, while the native program, run in the environment it
/// promises (MXCSR DAZ and FTZ, `-DFLUSH`), flushes them (48 of 96 words at both levels differed before the
/// refusal). Route past it: the floating-point environment through SoftFloat's state.
fn assert_denormal_mode_refused(tag: &str, flags: &[&str], promise: &str) {
    let source = c_dir().join("attribute_fp.c");
    for (base, level) in [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")] {
        let mut device = base.to_vec();
        device.extend(flags);
        let error = lower_text(&source, &format!("attrs.{tag}.{level}"), &device, &[]).expect_err("denormal refusal");
        assert!(error.contains("not handled yet"), "{level}: {error}");
        // The first such operation is in @half or in @result, whichever the front lowers first.
        assert!(error.contains(" in @half,") || error.contains(" in @result,"), "{level}: {error}");
        assert!(error.contains(promise), "{level}: {error}");
    }
}

#[test]
fn a_preserve_sign_denormal_mode_is_refused_by_name() {
    let promise = "\"denormal-fp-math\"=\"preserve-sign,preserve-sign\"";
    assert_denormal_mode_refused("flush", &["-fdenormal-fp-math=preserve-sign,preserve-sign"], promise);
}

#[test]
fn bit_operations_under_a_preserve_sign_mode_are_accepted_and_match_native() {
    // fneg, fabs, copysign and moves read no subnormal mode: not refused, and equal to native under DAZ/FTZ.
    let flags = ["-fdenormal-fp-math=preserve-sign,preserve-sign"];
    compare("attribute_fp_bits", "flush-bits", 3, FP_INPUTS, &flags, &["-DFLUSH"]);
}

#[test]
fn positive_zero_and_dynamic_denormal_modes_are_refused_by_name() {
    for mode in ["positive-zero", "dynamic"] {
        let flag = format!("-fdenormal-fp-math={mode},{mode}");
        let promise = format!("\"denormal-fp-math\"=\"{mode},{mode}\"");
        assert_denormal_mode_refused(&format!("flush-{mode}"), &[&flag], &promise);
    }
}

#[test]
fn an_f32_preserve_sign_denormal_mode_is_refused_by_name() {
    assert_denormal_mode_refused(
        "flush-f32",
        &["-DF32_ONLY", "-Xclang", "-fdenormal-fp-math-f32=preserve-sign,preserve-sign"],
        "on `float` in @result, which promises the subnormal mode \"denormal-fp-math-f32\"=\"preserve-sign,preserve-sign\"",
    );
}

#[test]
fn strict_fp_comparison_is_refused_before_it_can_claim_native_agreement() {
    let source = c_dir().join("attribute_fp.c");
    for (flags, suffix) in [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")] {
        let level = if suffix == "o0" { "-O0" } else { "-O2" };
        let exe = native_build_with(&source, &format!("attrs.strict.{suffix}"), &[level, "-DSTRICT", "-DNATIVE"], &[]);
        let output = Command::new(exe).output().expect("execute strict native witness");
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).expect("native text").lines().count(), 48);
        let mut flags = flags.to_vec();
        flags.push("-DSTRICT");
        let error = lower_text(&source, &format!("attrs.strict.{suffix}"), &flags, &[]).expect_err("strict FP refusal");
        assert!(error.contains("constrained floating-point intrinsic"), "{error}");
    }
}

#[test]
fn witnesses_carry_attributes_on_the_placements_being_compared() {
    let disassemble = |name: &str, flags: &[&str]| {
        let file = bitcode(&c_dir().join(name), &format!("attrs.placements.{name}"), flags);
        let output = Command::new("llvm-dis-18").arg(file).args(["-o", "-"]).output().expect("LLVM disassembly");
        assert!(output.status.success());
        String::from_utf8(output.stdout).expect("LLVM text")
    };
    for flags in [CLANG_FLAGS, CLANG_O2_FLAGS] {
        let ir = disassemble("attribute_semantics.c", flags);
        for (name, attributes) in [
            ("narrow_s", &["signext i8", "i8 noundef signext"][..]),
            ("narrow_u", &["zeroext i8", "i8 noundef zeroext"][..]),
            ("narrow_b", &["zeroext i1", "i1 noundef zeroext"][..]),
            ("hidden", &["sret(%struct.Big)", "byval(%struct.Big)", "align 8"][..]),
            ("aligned_copy", &["byval(%struct.Aligned)", "align 64"][..]),
        ] {
            let signature = ir
                .lines()
                .find(|line| line.starts_with("define ") && line.contains(&format!("@{name}(")))
                .expect("fixture definition");
            for attr in attributes {
                assert!(signature.contains(attr), "{signature}: missing {attr}");
            }
        }
        let ir = disassemble("attribute_references.cpp", flags);
        let signature = ir
            .lines()
            .find(|line| line.starts_with("define ") && line.contains("@_Z6changeR3Boxm("))
            .expect("reference definition");
        let (ret, params) = signature.split_once("@_Z6changeR3Boxm(").expect("signature");
        for placement in [ret, params] {
            for attr in ["noundef", "nonnull", "align 8", "dereferenceable(16)"] {
                assert!(placement.contains(attr), "{placement}: missing {attr}");
            }
        }
        if flags == CLANG_O2_FLAGS {
            assert!(params.contains("returned"), "{params}");
        }
    }
    let mut flags = CLANG_O2_FLAGS.to_vec();
    flags.extend(["-fno-honor-nans", "-fno-honor-infinities"]);
    let ir = disassemble("attribute_fp.c", &flags);
    let signature = ir.lines().find(|line| line.starts_with("define ") && line.contains("@half(")).expect("half");
    let (ret, params) = signature.split_once("@half(").expect("signature");
    for placement in [ret, params] {
        assert!(placement.contains("nofpclass(nan inf)"), "{placement}");
    }
}
