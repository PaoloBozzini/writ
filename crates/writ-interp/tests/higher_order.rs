//! Higher-order functions (#124): a pure top-level function can be passed as a
//! value and called.

use writ_interp::{run, Value};

fn parse(src: &str) -> writ_ast::Module {
    let parsed = writ_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    parsed.module
}

fn f(name: &str) -> Value {
    Value::Function {
        name: name.to_string(),
    }
}

#[test]
fn a_function_value_is_applied() {
    let m = parse(
        "fn apply(g: fn(Int) -> Int, x: Int) -> Int { return g(x); }\n\
         fn inc(n: Int) -> Int { return n + 1; }",
    );
    let out = run(&m, "apply", vec![f("inc"), Value::Int(5)]).unwrap();
    assert_eq!(out, Value::Int(6));
}

#[test]
fn a_function_value_can_be_applied_twice() {
    let m = parse(
        "fn twice(g: fn(Int) -> Int, x: Int) -> Int { return g(g(x)); }\n\
         fn double(n: Int) -> Int { return n * 2; }",
    );
    let out = run(&m, "twice", vec![f("double"), Value::Int(3)]).unwrap();
    assert_eq!(out, Value::Int(12));
}

#[test]
fn a_bare_function_name_evaluates_to_a_function_value() {
    // `pick` returns a function value directly.
    let m = parse(
        "fn pick() -> fn(Int) -> Int { return inc; }\n\
         fn inc(n: Int) -> Int { return n + 1; }",
    );
    assert_eq!(run(&m, "pick", vec![]).unwrap(), f("inc"));
}
