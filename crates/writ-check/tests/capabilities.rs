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

// --- Structural escape check (#99) ----------------------------------------

#[test]
fn a_match_wrapper_cannot_stash_a_capability() {
    // Wrapping a capability in a `match` must not defeat E0202.
    let cs = cap_codes("fn f(out: Cap<Write>) { let stashed = match true { _ => out }; }");
    assert_eq!(cs, vec!["E0202"], "match must not launder a capability");
}

#[test]
fn a_grant_result_cannot_be_bound_to_a_local() {
    // A capability from `grant` is still a capability; binding it is E0202.
    let cs = cap_codes("fn f(root: Cap<Root>) { let w = grant<Write>(root); }");
    assert_eq!(cs, vec!["E0202"], "grant result cannot be stashed");
}

#[test]
fn returning_a_capability_via_a_match_is_refused() {
    let cs = cap_codes("fn f(out: Cap<Write>) -> Cap<Write> { return match true { _ => out }; }");
    // The return type is a capability (E0201 at the signature) and the returned
    // expression is a laundered capability (E0201 at the return).
    assert!(cs.iter().all(|c| c == "E0201"), "{cs:?}");
    assert!(cs.contains(&"E0201".to_string()));
}

#[test]
fn forwarding_a_capability_inline_is_still_allowed() {
    // The intended usage: narrow and forward in argument position, no local.
    assert!(cap_codes(
        "\
fn write_line(out: Cap<Write>, msg: Text) uses { Write } { return; }
fn f(root: Cap<Root>) uses { Write } { write_line(grant<Write>(root), \"hi\"); }
"
    )
    .is_empty());
}

// --- #144: sum-type wrapping must not launder a capability -----------------

const OPTION: &str = "type Option<T> = Some(T) | None\n";

#[test]
fn wrapping_a_capability_in_a_constructor_is_refused() {
    // `let x = Some(cap)` stashes a capability inside a sum value — E0202.
    let cs = cap_codes(&format!(
        "{OPTION}fn f(out: Cap<Write>) {{ let boxed = Some(out); }}"
    ));
    assert_eq!(cs, vec!["E0202"], "a constructor cannot stash a capability");
}

#[test]
fn returning_a_capability_wrapped_in_a_sum_type_is_refused() {
    // A return type that *contains* `Cap<..>` structurally is refused (E0201),
    // as is the returned `Some(out)` value.
    let cs = cap_codes(&format!(
        "{OPTION}fn stash(out: Cap<Write>) -> Option<Cap<Write>> {{ return Some(out); }}"
    ));
    assert!(cs.iter().all(|c| c == "E0201"), "{cs:?}");
    assert!(cs.contains(&"E0201".to_string()));
}

#[test]
fn wrap_then_unwrap_and_use_is_refused_at_the_stash() {
    // The full laundering attempt: wrap, then match it back out to use. The
    // stash itself is refused (E0202), so the capability never escapes.
    let cs = cap_codes(&format!(
        "{OPTION}\
fn write_line(out: Cap<Write>, msg: Text) uses {{ Write }} {{ return; }}
fn nothing() {{ return; }}
fn f(out: Cap<Write>) uses {{ Write }} {{
    let boxed = Some(out);
    match boxed {{ Some(c) => write_line(c, \"hi\"), None => nothing() }};
}}"
    ));
    assert!(cs.contains(&"E0202".to_string()), "wrap-unwrap-use: {cs:?}");
}

#[test]
fn a_non_capability_constructor_is_fine() {
    // Wrapping ordinary data in a constructor is unaffected.
    assert!(cap_codes(&format!("{OPTION}fn f(n: Int) {{ let boxed = Some(n); }}")).is_empty());
}
