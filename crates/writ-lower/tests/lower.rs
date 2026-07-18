//! Tests for contract desugaring: `requires` / `ensures` become shared
//! `Stmt::Check` nodes carrying the correct blame, on every exit path, and the
//! pass is idempotent.

use writ_ast::{Blame, Function, Item, Module, Stmt};
use writ_lower::lower;

fn lower_src(src: &str) -> Module {
    let parsed = writ_parser::parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "source should parse: {:?}",
        parsed.diagnostics
    );
    lower(&parsed.module)
}

fn only_fn(module: &Module) -> &Function {
    module
        .items
        .iter()
        .find_map(|i| match i {
            Item::Function(f) => Some(f),
            Item::Type(_) => None,
        })
        .expect("a function")
}

#[test]
fn requires_becomes_a_caller_blamed_check_on_entry() {
    let m = lower_src("fn f(n: Int) -> Int requires n > 0 { return n; }");
    let f = only_fn(&m);
    // The clause is gone from the signature...
    assert!(f.signature.requires.is_empty(), "requires cleared");
    // ...and appears as the first statement, blaming the caller.
    match &f.body.stmts[0] {
        Stmt::Check { blame, .. } => assert_eq!(*blame, Blame::Caller),
        other => panic!("expected a Check first, got {other:?}"),
    }
}

#[test]
fn ensures_becomes_an_implementation_blamed_check_before_return() {
    let m = lower_src("fn f(n: Int) -> Int ensures result >= 0 { return n; }");
    let f = only_fn(&m);
    assert!(f.signature.ensures.is_empty(), "ensures cleared");
    // Expect: let result = n; check(result >= 0, Implementation); return result;
    let stmts = &f.body.stmts;
    assert!(
        matches!(stmts[0], Stmt::Let { ref name, .. } if name == "result"),
        "result is bound before the check, got {:?}",
        stmts[0]
    );
    match &stmts[1] {
        Stmt::Check { blame, .. } => assert_eq!(*blame, Blame::Implementation),
        other => panic!("expected an implementation-blamed Check, got {other:?}"),
    }
    assert!(
        matches!(stmts[2], Stmt::Return { value: Some(_), .. }),
        "the value is returned after the check, got {:?}",
        stmts[2]
    );
}

#[test]
fn ensures_runs_on_every_return_path() {
    // Both branches return; each must get its own bound-result + check.
    let m = lower_src(
        "\
fn max(a: Int, b: Int) -> Int ensures result >= a {
    if a > b { return a; }
    return b;
}
",
    );
    let f = only_fn(&m);
    let checks = count_checks(&f.body.stmts);
    assert_eq!(checks, 2, "one ensures check per return path");
}

fn count_checks(stmts: &[Stmt]) -> usize {
    stmts
        .iter()
        .map(|s| match s {
            Stmt::Check { .. } => 1,
            Stmt::If {
                then_block,
                else_block,
                ..
            } => {
                count_checks(&then_block.stmts)
                    + else_block.as_ref().map_or(0, |b| count_checks(&b.stmts))
            }
            _ => 0,
        })
        .sum()
}

#[test]
fn a_function_without_contracts_is_left_alone() {
    let src = "fn f(n: Int) -> Int { return n + 1; }";
    let m = lower_src(src);
    // No contracts → the body is untouched (no Check nodes anywhere).
    assert_eq!(count_checks(&only_fn(&m).body.stmts), 0);
}

#[test]
fn lowering_is_idempotent() {
    let m1 = lower_src(
        "\
fn f(n: Int) -> Int requires n > 0 ensures result >= 0 { return n; }
",
    );
    let m2 = lower(&m1);
    assert_eq!(m1, m2, "lowering an already-lowered module is a no-op");
}
