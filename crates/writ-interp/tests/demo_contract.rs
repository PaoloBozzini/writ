//! Flagship contracts demo (#28): a pure, side-effect-free function that simply
//! returns the wrong answer is rejected for violating its `ensures` clause, with
//! implementation blame.
//!
//! Capabilities catch dangerous code (see the capability demo); contracts catch
//! wrong code.

use std::fs;
use std::path::PathBuf;

use writ_interp::{run, Blame, Value};

fn example_source(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {}", path.display()))
}

#[test]
fn wrong_answer_is_rejected_with_implementation_blame() {
    let src = example_source("reject_wrong_answer.writ");
    let parsed = writ_parser::parse(&src);
    assert!(
        parsed.diagnostics.is_empty(),
        "example should parse: {:?}",
        parsed.diagnostics
    );

    // max(3, 5) should be 5; this implementation returns 3 - 5 = -2, which
    // violates `ensures result >= a`.
    let err = run(&parsed.module, "max", vec![Value::Int(3), Value::Int(5)]).unwrap_err();

    assert_eq!(
        err.blame,
        Some(Blame::Implementation),
        "the body is at fault, not the caller"
    );
    assert!(err.message.contains("postcondition"), "{}", err.message);
}
