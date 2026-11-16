//! Semantic boundaries exercised by the IR mutation pass.

use bennett_ir::{
    BlockId, Const, DataItem, FuncId, Inst, Module, Operand, Param, Problem, Signature, Slot, Terminator, Type, Width,
    load, read, validate, write,
};

fn parsed(text: &str) -> Module {
    read(text).expect("test fixture reads")
}

fn rejected(module: Module) -> Vec<Problem> {
    validate(module).expect_err("malformed module must be rejected").0.into_iter().map(|e| e.problem).collect()
}

#[test]
fn direct_modules_check_indices_before_using_them() {
    let base = parsed("func @f(i8 %x) -> i8 {\nentry:\n ret %x\n}\n");
    let mut no_blocks = base.clone();
    no_blocks.functions[0].blocks.clear();
    assert!(rejected(no_blocks).contains(&Problem::NoBlocks));
    let mut entry_params = base.clone();
    entry_params.functions[0].blocks[0].params.push(Param { slot: Slot(0), ty: Type::Int(Width::BYTE) });
    assert!(rejected(entry_params).contains(&Problem::EntryParams));
    let mut slot = base.clone();
    slot.functions[0].blocks[0].term = Terminator::Return(Some(Operand::Slot(Slot(1))));
    assert!(rejected(slot).iter().any(|p| matches!(p, Problem::UnknownSlot(1))));
    let mut target = base.clone();
    target.functions[0].blocks[0].term = Terminator::Jump(bennett_ir::Target { block: BlockId(1), args: vec![] });
    assert!(rejected(target).iter().any(|p| matches!(p, Problem::UnknownBlock(1))));
    for inst in
        [Inst::FuncAddr { dst: Slot(0), func: FuncId(1) }, Inst::Call { dst: None, callee: FuncId(1), args: vec![] }]
    {
        let mut module = base.clone();
        module.functions[0].blocks[0].insts.push(inst);
        assert!(rejected(module).contains(&Problem::UnknownFunction(1)));
    }
}

#[test]
fn direct_modules_check_definitions_and_register_widths() {
    assert!(load("func @f(q64 %q) {\nentry:\n ret\n}\n").is_ok(), "64 qubits is allowed");
    let base = parsed("func @f(i8 %x) -> i8 {\nentry:\n ret %x\n}\n");
    let mut undefined = base.clone();
    undefined.functions[0].params.clear();
    assert!(rejected(undefined).contains(&Problem::Undefined("x".into())));
    let mut repeated = base.clone();
    repeated.functions[0].blocks[0].insts.push(Inst::Load {
        dst: Slot(0),
        width: Width::BYTE,
        address: Operand::Const(Const::new(Width::WORD, 0).expect("zero")),
    });
    assert!(rejected(repeated).contains(&Problem::Redefined("x".into())));
    let mut wide = base.clone();
    wide.functions[0].params[0].ty = Type::Qreg(Width::new(65).expect("integer width"));
    assert!(rejected(wide).contains(&Problem::RegisterTooWide(65)));
    let mut duplicate = base;
    duplicate.functions.push(duplicate.functions[0].clone());
    assert!(rejected(duplicate).contains(&Problem::DuplicateFunction));
}

#[test]
fn casts_and_ssa_have_strict_boundaries() {
    for (op, from, to, valid) in [
        ("zext", 8, 9, true),
        ("sext", 8, 9, true),
        ("trunc", 9, 8, true),
        ("zext", 8, 8, false),
        ("sext", 8, 8, false),
        ("trunc", 8, 8, false),
        ("zext", 9, 8, false),
        ("sext", 9, 8, false),
        ("trunc", 8, 9, false),
    ] {
        let text = format!("func @f(i{from} %x) -> i{to} {{\nentry:\n %y = {op} i{from} %x to i{to}\n ret %y\n}}\n");
        assert_eq!(load(&text).is_ok(), valid, "{text}");
    }
    for body in ["%x = add i8 %x, 1\n ret %x", "br next\nnext:\n %y = add i8 %x, 1\n %x = add i8 1, 2\n ret %y"] {
        let text = format!("func @f() -> i8 {{\nentry:\n {body}\n}}\n");
        assert!(rejected(parsed(&text)).iter().any(|p| matches!(p, Problem::NotDominated(_))));
    }
    // Unreachable blocks may use values whose definitions do not dominate them.
    assert!(load("func @f() -> i8 {\nentry:\n ret 0\ndead:\n %x = add i8 %x, 1\n ret %x\n}\n").is_ok());
}

#[test]
fn function_addresses_and_signatures_check_every_component() {
    let module =
        load("func @f(i8 %x, q2 %q) -> i16 {\nentry:\n ret 0\n}\nfunc @g() {\nentry:\n ret\n}\n").expect("valid");
    assert_eq!(module.function_named("f"), Some(FuncId(0)));
    assert_eq!(module.function_named("g"), Some(FuncId(1)));
    assert_eq!(module.function_named("missing"), None);
    for address in [0, 3, u64::MAX, (1u64 << 32) + 1] {
        assert_eq!(module.function_at(address), None);
    }
    assert_eq!(module.function_at(1), Some(FuncId(0)));
    assert_eq!(module.function_at(2), Some(FuncId(1)));
    let params = vec![Type::Int(Width::BYTE), Type::Qreg(Width::new(2).expect("width"))];
    let result = Some(Type::Int(Width::new(16).expect("width")));
    let function = module.function(FuncId(0));
    assert!(function.has_signature(&Signature { params: params.clone(), result }));
    assert!(!function.has_signature(&Signature { params: params.clone(), result: None }));
    assert!(!function.has_signature(&Signature { params: params[..1].to_vec(), result }));
    assert!(!function.has_signature(&Signature { params: vec![params[1], params[0]], result }));
}

#[test]
fn function_addresses_in_data_must_exist_and_fit_their_width() {
    let text = "memory 8\ndata 0 = i1 @second\nfunc @first() {\nentry:\n ret\n}\nfunc @second() {\nentry:\n ret\n}\n";
    assert!(rejected(parsed(text)).iter().any(|p| matches!(p, Problem::AddressTooWide { bits: 1, .. })));
    let mut module = parsed(text);
    module.data[0].items[0] = DataItem::FuncAddr { width: Width::WORD, func: FuncId(2) };
    assert!(rejected(module).contains(&Problem::UnknownFunction(2)));
    assert!(load(&text.replace("i1 @second", "i2 @second")).is_ok());
    assert!(load(&text.replace("i1 @second", "i1 @first")).is_ok(), "address 1 fits exactly in i1");
}

#[test]
fn forward_references_preserve_definition_order_and_bytes() {
    let text = "memory 64\ndata 0 = i16 @last, i16 @first\nfunc @first(i8 %x) -> i8 {\nentry:\n %v = call @last(%x)\n br later(%v)\nearlier(i8 %y):\n ret %y\nlater(i8 %z):\n br earlier(%z)\n}\nfunc @last(i8 %x) -> i8 {\nentry:\n ret %x\n}\n";
    let module = parsed(text);
    assert_eq!(module.data[0].encode(), [2, 0, 1, 0]);
    assert_eq!(module.functions.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["first", "last"]);
    assert_eq!(
        module.functions[0].blocks.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
        ["entry", "earlier", "later"]
    );
    assert_eq!(read(&write(&module)).expect("round trip"), module);
    validate(module).expect("valid");
}

#[test]
fn all_byte_values_round_trip_and_malformed_escapes_fail() {
    let mut module = parsed("memory 256\ndata 0 = \"\"\n");
    module.data[0].items[0] = DataItem::Bytes((0..=255).collect());
    assert_eq!(read(&write(&module)).expect("every byte reads back"), module);
    for escape in [r"\x", r"\x0", r"\xGG", r"\q"] {
        assert!(read(&format!("data 0 = \"{escape}\"\n")).is_err(), "{escape}");
    }
    assert_eq!(parsed("data 0 = \"éλ\"\n").data[0].encode(), "éλ".as_bytes());
    assert_eq!(parsed(r#"data 0 = "\n\t\r\0\\\"\x41""#).data[0].encode(), b"\n\t\r\0\\\"A");
}

#[test]
fn data_emptiness_is_about_byte_extent() {
    for (text, empty) in [
        ("data 0 = \"\"", true),
        ("data 0 = zero 0", true),
        ("data 0 = \"x\"", false),
        ("data 0 = zero 1", false),
        ("data 0 = i1 0", false),
    ] {
        let module = parsed(text);
        assert_eq!(module.data[0].is_empty(), empty, "{text}");
        assert_eq!(module.data[0].items[0].is_empty(), empty, "{text}");
        assert_eq!(module.data[0].encode().is_empty(), empty, "{text}");
    }
}

#[test]
fn validation_locations_count_instructions_from_one() {
    let module =
        parsed("func @f(i8 %x, i16 %y) -> i8 {\nentry:\n %a = add i8 %x, 1\n %b = add i8 %x, %y\n ret %a\n}\n");
    let errors = validate(module).expect_err("second instruction has a mismatched operand");
    assert_eq!(errors.0.len(), 1);
    assert_eq!(
        errors.0[0].location,
        bennett_ir::Location::Inst { function: "f".into(), block: "entry".into(), number: 2 }
    );
    assert!(matches!(errors.0[0].problem, Problem::TypeMismatch { .. }));
}

#[test]
fn regions_check_argument_counts_widths_and_all_alias_roles() {
    let bits = bennett_ir::Fault::CODE_WIDTH.bits();
    let short = bits - 1;
    let body = |apply: &str| {
        format!(
            "func @g(i{bits} %x) -> i{bits} {{\nentry:\n ret %x\n}}\nfunc @f(q{bits} %q, q{bits} %o, q{bits} %e, q{short} %short) {{\nentry:\n {apply}\n ret\n}}\n"
        )
    };
    assert!(load(&body("apply @g(%q) -> %o, %e budget 5")).is_ok());
    for apply in [
        "apply @g() -> %o, %e budget 5".to_owned(),
        format!("apply @g(%q, i{bits} 0) -> %o, %e budget 5"),
        "apply @g(%short) -> %o, %e budget 5".to_owned(),
        format!("apply @g(i{short} 0) -> %o, %e budget 5"),
    ] {
        assert!(load(&body(&apply)).is_err(), "accepted {apply}");
    }
    for apply in
        ["apply @g(%q) -> %o, %o budget 5", "apply @g(%o) -> %o, %e budget 5", "apply @g(%e) -> %o, %e budget 5"]
    {
        assert!(rejected(parsed(&body(apply))).iter().any(|p| matches!(p, Problem::RegionAlias(_))), "{apply}");
    }
}

#[test]
fn switch_cases_must_be_distinct_and_fit_including_the_maximum() {
    let text = "func @f(i2 %x) -> i8 {\nentry:\n switch i2 %x, 3: yes, default: no\nyes:\n ret 1\nno:\n ret 2\n}\n";
    let mut module = parsed(text);
    validate(module.clone()).expect("maximum case value fits");
    let duplicate = text.replace("3: yes, default", "3: yes, 3: no, default");
    assert!(rejected(parsed(&duplicate)).contains(&Problem::BadCase(3)));
    let Terminator::Switch { cases, .. } = &mut module.functions[0].blocks[0].term else {
        panic!("a switch");
    };
    cases[0].value = 4;
    assert!(rejected(module).contains(&Problem::BadCase(4)), "direct modules need the same range check");
}

#[test]
fn void_calls_cannot_define_a_result() {
    for call in ["call @g(i8 0)", "call_indirect fn(i8) %p(i8 0)"] {
        let text = format!(
            "func @g(i8 %x) {{\nentry:\n ret\n}}\nfunc @f() {{\nentry:\n %p = faddr @g\n %v = {call}\n ret\n}}\n"
        );
        assert!(rejected(parsed(&text)).contains(&Problem::NoResult), "{call}");
    }
}

#[test]
fn stack_selection_indirect_calls_and_return_addresses_round_trip() {
    let text = "memory 256\nfunc @g(i8 %x) -> i8 {\nentry:\n ret %x\n}\nfunc @f(i1 %c, i8 %x) -> i8 {\nentry:\n %s = select i8 %c, %x, 1\n %sp = stacksave\n stackrestore %sp\n %ra = retaddr 0\n %buf = alloca 32\n %jump = setjmp %buf\n %ptr = faddr @g\n %v = call_indirect fn(i8) -> i8 %ptr(%s)\n ret %v\n}\nfunc @jump(i64 %buffer, i32 %value) {\nentry:\n longjmp %buffer, %value\n}\n";
    let module = parsed(text);
    validate(module.clone()).expect("well typed classical instructions");
    assert_eq!(read(&write(&module)).expect("round trip"), module);
}

#[test]
fn return_addresses_count_instructions_and_terminators_in_prior_blocks() {
    let module = parsed(
        "func @f(i8 %x) -> i8 {\nentry:\n %a = add i8 %x, 1\n %b = add i8 %a, 1\n br middle\nmiddle:\n br last\nlast:\n %c = add i8 %b, 1\n %d = add i8 %c, 1\n ret %d\n}\n",
    );
    validate(module.clone()).expect("valid control flow");
    let function = &module.functions[0];
    // entry occupies positions 0,1,2; middle's terminator is 3; last starts at 4.
    for (block, inst, position_after) in [(0, 0, 1), (0, 1, 2), (2, 0, 5), (2, 1, 6)] {
        assert_eq!(function.code_address_after(FuncId(0), BlockId(block), inst), 4_294_967_296 + position_after);
    }
    assert_eq!(bennett_ir::code_address(FuncId(17), 7), 77_309_411_335);
}

#[test]
#[ignore = "reader accepts infinite rotation from exponent overflow, writer emits unreadable inf"]
fn every_readable_rotation_round_trips_even_when_validation_rejects_it() {
    // SYNTAX promises read(write(m)) == m for every module read from text, not only valid modules.
    let module = parsed("func @f(q1 %q) {\nentry:\n rx(1e999) %q\n ret\n}\n");
    assert!(rejected(module.clone()).contains(&Problem::AngleNotFinite));
    assert_eq!(read(&write(&module)).expect("a module read from text must read back"), module);
}
