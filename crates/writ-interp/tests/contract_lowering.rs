//! Contracts flow through the shared `writ-lower` desugaring and are enforced by
//! the interpreter's `Stmt::Check` semantics — with blame preserved. A failed
//! precondition blames the **caller**; a failed postcondition blames the
//! **implementation**. The rejection, with the right blame, is the feature.

use writ_interp::{run, Blame, Value};

fn parse(src: &str) -> writ_ast::Module {
    let parsed = writ_parser::parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "source should parse: {:?}",
        parsed.diagnostics
    );
    parsed.module
}

#[test]
fn satisfied_contract_returns_normally() {
    let m = parse("fn inc(n: Int) -> Int requires n > 0 ensures result > n { return n + 1; }");
    let v = run(&m, "inc", vec![Value::Int(1)]).expect("contract satisfied");
    assert_eq!(v, Value::Int(2));
}

#[test]
fn failed_precondition_blames_the_caller() {
    // `inc` requires `n > 0`; calling with 0 is the caller's fault.
    let m = parse("fn inc(n: Int) -> Int requires n > 0 { return n + 1; }");
    let err = run(&m, "inc", vec![Value::Int(0)]).unwrap_err();
    assert_eq!(err.blame, Some(Blame::Caller), "{}", err.message);
    assert!(err.message.contains("precondition"), "{}", err.message);
}

#[test]
fn failed_postcondition_blames_the_implementation() {
    // `bad` promises `result > n` but returns `n`; the body is at fault.
    let m = parse("fn bad(n: Int) -> Int ensures result > n { return n; }");
    let err = run(&m, "bad", vec![Value::Int(5)]).unwrap_err();
    assert_eq!(err.blame, Some(Blame::Implementation), "{}", err.message);
    assert!(err.message.contains("postcondition"), "{}", err.message);
}

#[test]
fn postcondition_is_enforced_on_the_taken_return_path() {
    // `max` returns via one of two branches; the postcondition must hold on
    // whichever path executes. Here the body is correct, so both paths pass.
    let m = parse(
        "\
fn max(a: Int, b: Int) -> Int ensures result >= a ensures result >= b {
    if a > b { return a; }
    return b;
}
",
    );
    assert_eq!(
        run(&m, "max", vec![Value::Int(3), Value::Int(5)]).unwrap(),
        Value::Int(5)
    );
    assert_eq!(
        run(&m, "max", vec![Value::Int(9), Value::Int(2)]).unwrap(),
        Value::Int(9)
    );
}
