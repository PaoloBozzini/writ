//! End-to-end tests: parse each example program in the repo's `examples/`
//! directory and run it, asserting the output the `print` built-in produced.

use std::fs;
use std::path::PathBuf;

use writ_interp::{Interpreter, Value};

fn examples_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/writ-interp; the examples live at the root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

/// Parse the example at `name`, run `main`, and return the printed lines.
fn run_example(name: &str) -> Vec<String> {
    let path = examples_dir().join(name);
    let src = fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {}", path.display()));
    let parsed = writ_parser::parse(&src);
    assert!(
        parsed.diagnostics.is_empty(),
        "{name} should parse: {:?}",
        parsed.diagnostics
    );
    let interp = Interpreter::new(&parsed.module).expect("build interpreter");
    interp
        .call("main", vec![])
        .unwrap_or_else(|e| panic!("{name} should run: {e}"));
    interp.output()
}

#[test]
fn hello_prints_a_greeting() {
    assert_eq!(run_example("hello.writ"), vec!["Hello, Writ!".to_string()]);
}

#[test]
fn factorial_runs_end_to_end() {
    assert_eq!(run_example("factorial.writ"), vec!["120".to_string()]);
}

#[test]
fn rock_paper_scissors_example_decides_the_winner() {
    assert_eq!(
        run_example("rock_paper_scissors.writ"),
        vec!["player 1 wins", "player 2 wins", "draw"]
    );
}

#[test]
fn shapes_example_computes_areas() {
    assert_eq!(run_example("shapes.writ"), vec!["75", "12"]);
}

#[test]
fn contract_example_runs_with_contracts_that_hold() {
    assert_eq!(run_example("contract.writ"), vec!["7", "3", "5", "9"]);
}

#[test]
fn text_example_processes_strings() {
    assert_eq!(run_example("text.writ"), vec!["tirW", "HELLO!"]);
}

/// Parse the example at `name`, run `main` — handing it a root capability for
/// each of its capability parameters, as the runtime does — and return the
/// printed lines.
fn run_example_with_root(name: &str) -> Vec<String> {
    let path = examples_dir().join(name);
    let src = fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {}", path.display()));
    let parsed = writ_parser::parse(&src);
    assert!(
        parsed.diagnostics.is_empty(),
        "{name} should parse: {:?}",
        parsed.diagnostics
    );
    let interp = Interpreter::new(&parsed.module).expect("build interpreter");
    let main = parsed
        .module
        .items
        .iter()
        .find_map(|it| match it {
            writ_ast::Item::Function(f) if f.signature.name == "main" => Some(f),
            _ => None,
        })
        .expect("a `main` function");
    let args = main
        .signature
        .params
        .iter()
        .map(|p| {
            if p.ty.name == "Cap" {
                let authority =
                    p.ty.args
                        .first()
                        .map_or_else(|| "Root".to_string(), |a| a.name.clone());
                Value::Capability { authority }
            } else {
                Value::Unit
            }
        })
        .collect();
    interp
        .call("main", args)
        .unwrap_or_else(|e| panic!("{name} should run: {e}"));
    interp.output()
}

#[test]
fn custom_capability_example_audits_with_a_granted_token() {
    // A user-defined `Audit` authority, granted from the root capability and
    // forwarded through the call chain.
    assert_eq!(
        run_example_with_root("custom_capability.writ"),
        vec![
            "[audit] login: alice".to_string(),
            "[audit] system started".to_string(),
        ]
    );
}
