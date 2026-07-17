//! Capability-pass tests: `Cap<T>` is parameter-only and second-class.

use writ_check::{check_capabilities, check_types};

fn cap_codes(src: &str) -> Vec<String> {
    let parsed = writ_parser::parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "source should parse: {:?}",
        parsed.diagnostics
    );
    check_capabilities(&parsed.module)
        .into_iter()
        .map(|d| d.code)
        .collect()
}

fn type_codes(src: &str) -> Vec<String> {
    let parsed = writ_parser::parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "source should parse: {:?}",
        parsed.diagnostics
    );
    check_types(&parsed.module)
        .into_iter()
        .map(|d| d.code)
        .collect()
}

#[test]
fn capability_parameter_and_forwarding_is_allowed() {
    // A capability enters as a parameter and is passed on as an argument.
    assert!(cap_codes(
        "\
fn write_line(out: Cap<Write>, msg: Text) uses { Write } { return; }
fn greet(out: Cap<Write>) uses { Write } { write_line(out, \"hi\"); }
"
    )
    .is_empty());
}

#[test]
fn sandboxed_function_has_no_capability_and_is_fine() {
    assert!(cap_codes("fn pure(a: Int) -> Int { return a + 1; }").is_empty());
}

// --- Negative tests: the refusal is the feature. --------------------------

#[test]
fn returning_a_capability_is_refused() {
    let cs = cap_codes("fn leak(out: Cap<Write>) -> Cap<Write> { return out; }");
    // Both the return-type position and the returned value are flagged.
    assert!(
        cs.iter().all(|c| c == "E0201"),
        "expected E0201s, got {cs:?}"
    );
    assert!(!cs.is_empty());
}

#[test]
fn binding_a_capability_to_a_local_is_refused() {
    let cs = cap_codes("fn f(out: Cap<Write>) { let c = out; }");
    assert_eq!(cs, vec!["E0202"]);
}

#[test]
fn a_capability_typed_local_annotation_is_refused() {
    let cs = cap_codes("fn f(out: Cap<Write>) { let c: Cap<Write> = out; }");
    assert_eq!(cs, vec!["E0202"]);
}

#[test]
fn a_capability_cannot_be_constructed_from_a_value() {
    // There is no capability constructor; trying to make one from a literal is a
    // plain type error — user code cannot conjure authority.
    let cs = type_codes("fn f() { let c: Cap<Write> = 0; }");
    assert_eq!(cs, vec!["T0001"]);
}
