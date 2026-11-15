//! Independent witnesses for the data-item grammar in docs/SYNTAX.md.

use bennett_ir::{DataItem, LoadError, Problem, ReadErrorKind, load, read, validate, write};

#[test]
#[ignore = "ZERO-001: docs/SYNTAX.md promises BadNumber for a signed zero count; plus gives BadCharacter"]
fn a_plus_signed_zero_count_reports_the_documented_bad_number() {
    let error = read("data 0 = zero +0").expect_err("a signed count is invalid");
    assert_eq!(error.kind, ReadErrorKind::BadNumber("+0".into()));
}

#[test]
fn zero_counts_and_statement_boundaries_follow_the_documented_grammar() {
    for (literal, count) in [
        ("0", 0),
        ("00016", 16),
        ("0xAbCd", 43981),
        ("18446744073709551615", u64::MAX),
        ("0xffffffffffffffff", u64::MAX),
    ] {
        for ending in ["", "\n", " ; comment\n", "\r\n"] {
            let text = format!("memory {}\ndata 0 = zero\t{literal}{ending}", u64::MAX);
            let module = load(&text).expect("a valid count and statement ending");
            assert_eq!(module.data[0].items, [DataItem::Zero(count)]);
            assert_eq!(module.static_end(), count);
            assert_eq!(read(&write(&module)).as_ref(), Ok(&*module));
        }
    }
    for literal in ["-0", "-0x0", "-1", "1.0", "1e0", "1E+0", "18446744073709551616", "0x10000000000000000"] {
        let error = read(&format!("data 0 = zero {literal}\n")).expect_err("invalid count");
        assert_eq!(error.kind, ReadErrorKind::BadNumber(literal.into()));
    }
    for suffix in ["", " ; missing\n", "\n0", "0x", "0X10", "0xg", "1 2", "1,", "@zero", "i8"] {
        let error = read(&format!("data 0 = zero {suffix}\n")).expect_err("missing count or extra token");
        assert!(matches!(error.kind, ReadErrorKind::Unexpected { .. }), "{suffix:?}: {error}");
    }
    // `zero` remains an ordinary function, block and value name outside data-item position.
    let text = "memory 32\ndata 0 = zero 1, i64 @zero\nfunc @zero(i8 %zero) -> i8 {\nzero:\n ret %zero\n}\n";
    let module = load(text).expect("zero is not a reserved name");
    assert_eq!(read(&write(&module)).as_ref(), Ok(&*module));
}

#[test]
fn zero_bounds_and_length_overflow_match_a_wider_integer_oracle() {
    let values = [0, 1, 15, 16, 17, u64::MAX - 1, u64::MAX];
    let mut cases = 0;
    for memory in values {
        for address in values {
            for first in values {
                for second in values {
                    let text = format!("memory {memory}\ndata {address} = zero {first}, zero {second}\n");
                    let module = read(&text).expect("all literals fit u64");
                    let length = u128::from(first) + u128::from(second);
                    let expected = if length > u128::from(u64::MAX) {
                        Some(Problem::DataTooLong)
                    } else if u128::from(address) + length > u128::from(memory) {
                        Some(Problem::DataOutOfBounds { address, len: length as u64, memory_size: memory })
                    } else {
                        None
                    };
                    let actual = validate(module);
                    match expected {
                        Some(problem) => assert_eq!(actual.expect_err(&text).0[0].problem, problem, "{text}"),
                        None => {
                            let valid = actual.expect(&text);
                            assert_eq!(valid.static_end(), address + length as u64, "{text}");
                            assert_eq!(read(&write(&valid)).as_ref(), Ok(&*valid), "{text}");
                        }
                    }
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 2401);
}

#[test]
fn zero_overlap_matches_shared_bytes_in_either_segment_order() {
    let mut cases = 0;
    for a in 0..=6 {
        for b in 0..=6 {
            for n in 0..=6 {
                for m in 0..=6 {
                    let shares_byte = (a..a + n).any(|byte| (b..b + m).contains(&byte));
                    for reversed in [false, true] {
                        let first = format!("data {a} = zero {n}\n");
                        let second = format!("data {b} = \"{}\"\n", "\\0".repeat(m as usize));
                        let text = if reversed {
                            format!("memory 16\n{second}{first}")
                        } else {
                            format!("memory 16\n{first}{second}")
                        };
                        match load(&text) {
                            Err(LoadError::Invalid(errors)) if shares_byte => {
                                assert_eq!(errors.0.len(), 1, "{text}");
                                assert_eq!(errors.0[0].problem, Problem::DataOverlap(0), "{text}");
                            }
                            Ok(module) if !shares_byte => assert_eq!(module.static_end(), (a + n).max(b + m)),
                            other => panic!("shared bytes = {shares_byte}: {other:?}\n{text}"),
                        }
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 4802);
}

#[test]
fn zero_items_do_not_align_neighbouring_strings_integers_or_function_addresses() {
    for bits in 1u32..=128 {
        for zeros in [0, 1, 7, 15, 16, 17] {
            let text = format!(
                "memory 256\ndata 3 = \"a;\\\"\\\\\\n\\x80\", zero {zeros}, i{bits} -1, \
                 zero 0, i{bits} @zero, \"\\0z\"\nfunc @zero() {{\nentry:\n ret\n}}\n"
            );
            let module = load(&text).expect("mixed data");
            let bytes = bits.div_ceil(8) as usize;
            let mut expected = b"a;\"\\\n\x80".to_vec();
            expected.extend(vec![0; zeros]);
            let value = u128::MAX >> (128 - bits);
            expected.extend_from_slice(&value.to_le_bytes()[..bytes]);
            expected.extend_from_slice(&1u128.to_le_bytes()[..bytes]);
            expected.extend_from_slice(b"\0z");
            assert_eq!(module.data[0].encode(), expected, "i{bits}, zero {zeros}");
            assert_eq!(module.static_end(), 3 + expected.len() as u64);
            assert_eq!(read(&write(&module)).as_ref(), Ok(&*module));
        }
    }
}
