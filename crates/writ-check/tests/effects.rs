//! Effect-system tests over real source.

use writ_check::check_effects;

fn codes(src: &str) -> Vec<String> {
    let parsed = writ_parser::parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "source should parse: {:?}",
        parsed.diagnostics
    );
    check_effects(&parsed.module)
        .into_iter()
        .map(|d| d.code)
        .collect()
}

fn assert_ok(src: &str) {
    let parsed = writ_parser::parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "source should parse: {:?}",
        parsed.diagnostics
    );
    let diags = check_effects(&parsed.module);
    assert!(
        diags.is_empty(),
        "expected no effect errors, got: {diags:?}"
    );
}

#[test]
fn declaring_the_effect_is_honest() {
    // `log` is the leaf effectful function (declares Write); `greet` declares
    // Write and calls it, so its declaration covers what it performs.
    assert_ok(
        "\
fn log(m: Text) uses { Write } { return; }
fn greet() uses { Write } { log(\"hi\"); }
",
    );
}

#[test]
fn over_declaring_is_allowed() {
    // Declaring an effect you never perform is not a subset violation.
    assert_ok("fn f() uses { Write } { return; }");
}

#[test]
fn a_function_with_no_effects_and_no_calls_is_fine() {
    assert_ok("fn add(a: Int, b: Int) -> Int { return a + b; }");
}

// --- Negative tests: an undeclared effect is refused. ---------------------

#[test]
fn direct_undeclared_effect_is_rejected() {
    let cs = codes(
        "\
fn log(m: Text) uses { Write } { return; }
fn bad() { log(\"hi\"); }
",
    );
    assert_eq!(
        cs,
        vec!["E0101"],
        "bad performs Write via log but declares nothing"
    );
}

#[test]
fn effect_reached_through_a_callee_is_rejected() {
    let cs = codes(
        "\
fn log(m: Text) uses { Write } { return; }
fn helper() uses { Write } { log(\"x\"); }
fn caller() { helper(); }
",
    );
    // `caller` performs Write via `helper` (which got it from `log`).
    assert_eq!(cs, vec!["E0101"]);
}

#[test]
fn only_the_undeclared_effect_of_a_set_is_reported() {
    let cs = codes(
        "\
fn io(m: Text) uses { Write, Net } { return; }
fn f() uses { Write } { io(\"x\"); }
",
    );
    // Write is declared; only Net is missing.
    assert_eq!(cs, vec!["E0101"]);
}

#[test]
fn diagnostics_are_deterministic() {
    let src = "\
fn io(m: Text) uses { Write, Net } { return; }
fn f() { io(\"x\"); }
";
    assert_eq!(codes(src), codes(src));
}

// --- Contract predicates must be pure (#97) -------------------------------

#[test]
fn an_effectful_call_in_a_requires_predicate_is_refused() {
    // `noisy` declares `Write`; calling it inside `requires` would perform an
    // undeclared effect at runtime. It must be refused.
    let cs = codes(
        "\
fn noisy(out: Cap<Write>) -> Bool uses { Write } { return true; }
fn f(out: Cap<Write>, n: Int) -> Int requires noisy(out) { return n; }
",
    );
    assert_eq!(cs, vec!["E0102"], "effectful requires predicate");
}

#[test]
fn an_effectful_call_in_an_ensures_predicate_is_refused() {
    let cs = codes(
        "\
fn noisy(out: Cap<Write>) -> Bool uses { Write } { return true; }
fn f(out: Cap<Write>, n: Int) -> Int ensures noisy(out) { return n; }
",
    );
    assert_eq!(cs, vec!["E0102"], "effectful ensures predicate");
}

#[test]
fn a_pure_call_in_a_predicate_is_allowed() {
    // A predicate may call a pure (effect-free) function.
    assert_ok(
        "\
fn is_positive(n: Int) -> Bool { return n > 0; }
fn f(n: Int) -> Int requires is_positive(n) ensures is_positive(result) { return n; }
",
    );
}
