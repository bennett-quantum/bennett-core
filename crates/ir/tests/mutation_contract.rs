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
}

#[test]
#[ignore = "reader accepts infinite rotation from exponent overflow, writer emits unreadable inf"]
fn every_readable_rotation_round_trips_even_when_validation_rejects_it() {
    // SYNTAX promises read(write(m)) == m for every module read from text, not only valid modules.
    let module = parsed("func @f(q1 %q) {\nentry:\n rx(1e999) %q\n ret\n}\n");
    assert!(rejected(module.clone()).contains(&Problem::AngleNotFinite));
    assert_eq!(read(&write(&module)).expect("a module read from text must read back"), module);
}
