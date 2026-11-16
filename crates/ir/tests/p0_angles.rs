//! Independent text round-trip and rotation-boundary probes.
use bennett_ir::{Gate, Inst, Module, ReadErrorKind, read, write};

fn source(gate: &str, angle: &str, place: &str) -> String {
    format!("memory 1024\nfunc @f(q2 %q, i64 %p) {{\nentry:\n {gate}({angle}) {place}\n ret\n}}\n")
}

fn angle_bits(module: &Module) -> u64 {
    let (Inst::Gate { gate: Gate::Rotate(_, a), .. } | Inst::GateAt { gate: Gate::Rotate(_, a), .. }) =
        &module.functions[0].blocks[0].insts[0]
    else {
        panic!("one rotation")
    };
    a.to_bits()
}

#[test]
fn all_rotation_axes_and_address_forms_refuse_nonfinite_spellings() {
    for gate in ["rx", "ry", "rz"] {
        for place in ["%q[1]", "[%p:127]", "%q ctrl [0:2]", "[16] ctrl %q[1]"] {
            for angle in [
                "inf",
                "-inf",
                "+inf",
                "Inf",
                "INF",
                "infinity",
                "-Infinity",
                "NaN",
                "nan",
                "-nan",
                "nan(1)",
                "0x7ff0000000000000",
                "1e999",
                "-1E+999",
                "1.8e308",
                "-1.7976931348623159e308",
            ] {
                let result = read(&source(gate, angle, place));
                // A hex integer's digits can also denote a finite f64 only if the float parser accepts
                // hex; the current decimal-angle grammar must reject it.
                assert!(result.is_err(), "{gate}({angle}) {place}: {result:?}");
                if ["1e999", "-1E+999", "1.8e308", "-1.7976931348623159e308"].contains(&angle) {
                    assert_eq!(result.expect_err("overflow").kind, ReadErrorKind::BadNumber(angle.into()));
                }
            }
        }
    }
}

#[test]
fn generated_overflows_name_the_literal_at_every_rotation_site() {
    for gate in ["rx", "ry", "rz"] {
        for place in ["%q[1]", "[%p:127]", "%q ctrl [0:2]", "[16] ctrl %q[1]"] {
            for sign in ["", "-"] {
                for exponent_mark in ["e", "E"] {
                    for exponent_sign in ["", "+"] {
                        for magnitude in ["309", "999", "000009999", "999999999999999999999999999999999999"] {
                            let angle = format!("{sign}1.23456789{exponent_mark}{exponent_sign}{magnitude}");
                            let error = read(&source(gate, &angle, place)).expect_err("overflow is refused");
                            assert_eq!(error.kind, ReadErrorKind::BadNumber(angle.clone()));
                            assert_eq!(error.line, 4);
                            assert!(error.to_string().contains(&angle), "literal is named: {error}");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn valid_boundary_angles_and_generated_finite_bits_are_never_refused_or_changed() {
    let mut angles = vec![
        "0".to_owned(),
        "-0".to_owned(),
        "-0.0".to_owned(),
        "1e-9999".to_owned(),
        "-1e-9999".to_owned(),
        "2e-324".to_owned(),
        "3e-324".to_owned(),
        "5e-324".to_owned(),
        "1.7976931348623157e308".to_owned(),
        "1.7976931348623158e308".to_owned(),
        "-1.7976931348623158e308".to_owned(),
    ];
    // Deterministic independent samples of every exponent, with both signs and varied significands.
    let mut bits = 0x65ac_7d10_2bf9_3301u64;
    for exponent in 0..2047u64 {
        bits ^= bits << 13;
        bits ^= bits >> 7;
        bits ^= bits << 17;
        let value = f64::from_bits((bits & 0x800f_ffff_ffff_ffff) | (exponent << 52));
        angles.push(value.to_string());
        angles.push(format!("{value:e}"));
    }
    for (i, angle) in angles.iter().enumerate() {
        let expected = angle.parse::<f64>().expect("generated decimal").to_bits();
        for gate in ["rx", "ry", "rz"] {
            let place = ["%q[1]", "[%p:127]", "%q ctrl [0:2]", "[16] ctrl %q[1]"][i % 4];
            let module = read(&source(gate, angle, place)).unwrap_or_else(|e| panic!("{angle}: {e}"));
            assert_eq!(angle_bits(&module), expected, "{gate}({angle})");
            let written = write(&module);
            let again = read(&written).unwrap_or_else(|e| panic!("{written}: {e}"));
            assert_eq!(angle_bits(&again), expected, "signed zero and subnormals survive writing");
            assert_eq!(again, module, "{gate}({angle})");
        }
    }
}

#[test]
fn generated_readable_modules_round_trip_even_without_validation() {
    // Forward names, non-definition slot order, unreachable blocks, all integer widths, escaped bytes,
    // zero data, and deliberately invalid memory extents/argument widths exercise read/write's contract
    // independently of validation. The original modules come from text, not hand-built IR.
    for width in 1..=128 {
        for reverse in [false, true] {
            let functions = [
                format!(
                    "func @first(i{width} %x) -> i{width} {{\nentry:\n br use\nuse:\n %v = call @last(%x)\n ret %v\ndead:\n ret 0\n}}\n"
                ),
                format!(
                    "func @last(i{width} %p) -> i{width} {{\nentry:\n %later = add i{width} %p, -1\n ret %later\n}}\n"
                ),
            ];
            let order = if reverse { [1, 0] } else { [0, 1] };
            let text = format!(
                "memory {}\ndata 9 = i{width} -1, zero 0, \"\\0\\xff\\n\\t\\r\\\\\\\"\", i64 @last\n{}{}",
                width % 17,
                functions[order[0]],
                functions[order[1]],
            );
            let module = read(&text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(read(&write(&module)).expect("written readable module"), module, "{text}");
        }
    }
}
