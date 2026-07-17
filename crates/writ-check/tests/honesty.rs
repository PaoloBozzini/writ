//! Honesty-check tests: an undeclared effect fails to compile, and the
//! diagnostic names the effect, the effect site (the callee), and the signature
//! that omitted it — for both direct effects and effects reached through a
//! callee.

use writ_check::check_effects;

fn diagnostics(src: &str) -> Vec<writ_ast::Diagnostic> {
    let parsed = writ_parser::parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "source should parse: {:?}",
        parsed.diagnostics
    );
    check_effects(&parsed.module)
}

#[test]
fn direct_effect_names_effect_site_and_signature() {
    let src = "\
fn log(m: Text) uses { Write } { return; }
fn bad() { log(\"hi\"); }
";
    let diags = diagnostics(src);
    assert_eq!(diags.len(), 1);
    let d = &diags[0];
    assert_eq!(d.code, "E0101");
    assert!(d.is_error());
    // Names the omitting signature, the effect, and the callee (effect site).
    assert!(
        d.message.contains("`bad`"),
        "must name the signature: {}",
        d.message
    );
    assert!(
        d.message.contains("`Write`"),
        "must name the effect: {}",
        d.message
    );
    assert!(
        d.message.contains("`log`"),
        "must name the effect site (callee): {}",
        d.message
    );
    // The span points at the offending call site, not the whole function.
    let call_start = src.find("log(\"hi\")").unwrap();
    assert_eq!(d.span.start, call_start);
}

#[test]
fn effect_through_a_callee_is_reported_against_the_direct_call() {
    let src = "\
fn log(m: Text) uses { Write } { return; }
fn helper() uses { Write } { log(\"x\"); }
fn caller() { helper(); }
";
    let diags = diagnostics(src);
    assert_eq!(diags.len(), 1);
    let d = &diags[0];
    assert_eq!(d.code, "E0101");
    // `caller` reaches Write through `helper`; the diagnostic names both.
    assert!(d.message.contains("`caller`"), "{}", d.message);
    assert!(d.message.contains("`Write`"), "{}", d.message);
    assert!(d.message.contains("`helper`"), "{}", d.message);
}

#[test]
fn a_signature_can_never_under_report_its_power() {
    // Every effect a body performs must be declared; declaring them all compiles.
    let src = "\
fn log(m: Text) uses { Write } { return; }
fn net(m: Text) uses { Net } { return; }
fn honest() uses { Write, Net } { log(\"a\"); net(\"b\"); }
";
    assert!(
        diagnostics(src).is_empty(),
        "declaring every performed effect is honest"
    );
}
