//! Authority-pass tests: an effect site needs a matching capability in scope.

use writ_check::check_authority;

fn codes(src: &str) -> Vec<String> {
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

fn diagnostics(src: &str) -> Vec<writ_ast::Diagnostic> {
    let parsed = writ_parser::parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "source should parse: {:?}",
        parsed.diagnostics
    );
    check_authority(&parsed.module)
}

#[test]
fn holding_the_capability_authorizes_the_effect() {
    // `greet` holds Cap<Write> and forwards it to the effectful `write_line`.
    assert!(codes(
        "\
fn write_line(out: Cap<Write>, msg: Text) uses { Write } { return; }
fn greet(out: Cap<Write>) uses { Write } { write_line(out, \"hi\"); }
"
    )
    .is_empty());
}

#[test]
fn a_pure_function_needs_no_capability() {
    assert!(codes(
        "\
fn inc(n: Int) -> Int { return n + 1; }
fn main() { inc(1); }
"
    )
    .is_empty());
}

// --- Negative tests: dangerous power is unreachable without the token. -----

#[test]
fn performing_an_effect_without_the_capability_is_refused() {
    let cs = codes(
        "\
fn write_line(out: Cap<Write>, msg: Text) uses { Write } { return; }
fn bad(out: Cap<Write>) uses { Write } { write_line(out, \"hi\"); }
fn worse(x: Int) uses { Write } { write_line(x, \"hi\"); }
",
    );
    // `worse` holds no Cap<Write>; `bad` does.
    assert_eq!(cs, vec!["E0301"]);
}

#[test]
fn diagnostic_names_effect_callee_capability_and_function() {
    let d = &diagnostics(
        "\
fn logger(out: Cap<Write>) uses { Write } { return; }
fn worse(x: Int) uses { Write } { logger(x); }
",
    )[0];
    assert_eq!(d.code, "E0301");
    assert!(d.message.contains("`Write`"), "{}", d.message);
    assert!(d.message.contains("`logger`"), "{}", d.message);
    assert!(d.message.contains("Cap<Write>"), "{}", d.message);
    assert!(d.message.contains("`worse`"), "{}", d.message);
}

#[test]
fn holding_the_wrong_capability_is_still_refused() {
    // Holds Cap<Net> but the effect performed is Write.
    let cs = codes(
        "\
fn logger(out: Cap<Write>) uses { Write } { return; }
fn f(net: Cap<Net>) uses { Write } { logger(net); }
",
    );
    assert_eq!(cs, vec!["E0301"]);
}
