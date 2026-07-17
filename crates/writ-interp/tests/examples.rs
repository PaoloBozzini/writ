//! End-to-end tests: parse each example program in the repo's `examples/`
//! directory and run it, asserting the output the `print` built-in produced.

use std::fs;
use std::path::PathBuf;

use writ_interp::Interpreter;

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
