//! Tests for capability narrowing (`grant`) and the root capability (#22).

use writ_check::{check_authority, check_types};

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

fn authority_codes(src: &str) -> Vec<String> {
    let parsed = writ_parser::parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "source should parse: {:?}",
        parsed.diagnostics
    );
    check_authority(&parsed.module)
        .into_iter()
        .map(|d| d.code)
        .collect()
}

const NARROW_AND_PASS: &str = "\
fn write_line(out: Cap<Write>, msg: Text) uses { Write } { return; }
fn main(root: Cap<Root>) uses { Write } {
    write_line(grant<Write>(root), \"hi\");
}
";

#[test]
fn root_narrows_to_a_specific_power_and_type_checks() {
    assert!(
        type_codes(NARROW_AND_PASS).is_empty(),
        "{:?}",
        type_codes(NARROW_AND_PASS)
    );
}

#[test]
fn root_holder_is_authorized_and_narrowed_cap_checks_at_the_effect_site() {
    // `main` holds the root capability (authorized for everything); the narrowed
    // Cap<Write> it passes satisfies `write_line`'s effect site.
    assert!(
        authority_codes(NARROW_AND_PASS).is_empty(),
        "{:?}",
        authority_codes(NARROW_AND_PASS)
    );
}

// --- Negative tests: narrowing can only shed authority. -------------------

#[test]
fn narrowing_cannot_amplify_authority() {
    // Holding Cap<Net>, you cannot grant yourself Cap<Write>.
    let cs = type_codes("fn f(net: Cap<Net>) { let x = grant<Write>(net); }");
    assert_eq!(cs, vec!["T0009"]);
}

#[test]
fn grant_requires_a_type_argument() {
    let cs = type_codes("fn f(net: Cap<Net>) { let x = grant(net); }");
    assert_eq!(cs, vec!["T0008"]);
}

#[test]
fn identity_narrowing_is_allowed() {
    // Cap<Write> -> Cap<Write> is a no-op narrowing, permitted.
    assert!(type_codes("fn f(w: Cap<Write>) { let x = grant<Write>(w); }").is_empty());
}
