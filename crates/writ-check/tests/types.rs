//! Type-checker tests over real source.

use writ_check::check_types;

/// Parse `src` and return the type diagnostics' codes in order.
fn codes(src: &str) -> Vec<String> {
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

/// Assert that `src` type-checks with no diagnostics.
fn assert_ok(src: &str) {
    let parsed = writ_parser::parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "source should parse: {:?}",
        parsed.diagnostics
    );
    let diags = check_types(&parsed.module);
    assert!(diags.is_empty(), "expected no type errors, got: {diags:?}");
}

#[test]
fn well_typed_program_has_no_errors() {
    assert_ok(
        "\
fn add(a: Int, b: Int) -> Int {
    let sum = a + b;
    return sum;
}
fn main() {
    let ok = 1 < 2 && true;
    print(add(3, 4));
}
",
    );
}

#[test]
fn opaque_types_pass_through() {
    // `Cap<Write>` has no built-in rules; it is treated opaquely and a function
    // may take and return it without a type error.
    assert_ok("fn use_cap(c: Cap<Write>) -> Cap<Write> { return c; }");
}

#[test]
fn conditionals_and_calls_type_check() {
    assert_ok(
        "\
fn fact(n: Int) -> Int {
    if n <= 1 { return 1; }
    return n * fact(n - 1);
}
",
    );
}

// --- Negative tests: the compile error is the feature. --------------------

#[test]
fn no_implicit_coercion_in_let_annotation() {
    // `let x: Int = true;` must not compile.
    let cs = codes("fn f() { let x: Int = true; }");
    assert_eq!(cs, vec!["T0001"], "expected one type mismatch");
}

#[test]
fn arithmetic_on_non_int_is_rejected() {
    let cs = codes(r#"fn f() -> Int { return 1 + "x"; }"#);
    assert!(
        cs.contains(&"T0001".to_string()),
        "expected a type mismatch, got {cs:?}"
    );
}

#[test]
fn if_condition_must_be_bool() {
    let cs = codes("fn f() { if 1 { return; } }");
    assert_eq!(cs, vec!["T0001"]);
}

#[test]
fn return_type_must_match() {
    let cs = codes("fn f() -> Int { return true; }");
    assert_eq!(cs, vec!["T0005"]);
}

#[test]
fn unknown_variable_is_reported() {
    let cs = codes("fn f() -> Int { return missing; }");
    assert_eq!(cs, vec!["T0002"]);
}

#[test]
fn wrong_argument_type_is_reported() {
    let cs = codes(
        "\
fn twice(n: Int) -> Int { return n + n; }
fn f() -> Int { return twice(true); }
",
    );
    assert_eq!(cs, vec!["T0001"]);
}

#[test]
fn wrong_argument_count_is_reported() {
    let cs = codes(
        "\
fn twice(n: Int) -> Int { return n + n; }
fn f() -> Int { return twice(1, 2); }
",
    );
    assert_eq!(cs, vec!["T0004"]);
}

#[test]
fn unknown_function_is_reported() {
    let cs = codes("fn f() -> Int { return nope(1); }");
    assert_eq!(cs, vec!["T0003"]);
}

// --- Contract predicate type-checking (#25) -------------------------------

#[test]
fn well_typed_contracts_check() {
    assert_ok(
        "\
fn half(n: Int) -> Int
    requires n > 0
    ensures result >= 0
{
    return n / 2;
}
",
    );
}

#[test]
fn non_bool_requires_is_rejected() {
    let cs = codes("fn f(n: Int) -> Int requires n + 1 { return n; }");
    assert_eq!(cs, vec!["T0007"]);
}

#[test]
fn ensures_referencing_unknown_name_is_rejected() {
    // `result` is in scope for `ensures`, but `bogus` is not.
    let cs = codes("fn f(n: Int) -> Int ensures bogus > 0 { return n; }");
    assert_eq!(cs, vec!["T0002"]);
}

// --- Sum types + exhaustiveness (#17) -------------------------------------

#[test]
fn exhaustive_match_type_checks() {
    assert_ok(
        "\
type Option = Some(Int) | None
fn unwrap_or(o: Option, fallback: Int) -> Int {
    return match o {
        Some(x) => x,
        None    => fallback,
    };
}
",
    );
}

#[test]
fn a_wildcard_makes_a_match_exhaustive() {
    assert_ok(
        "\
type Color = Red | Green | Blue
fn code(c: Color) -> Int {
    return match c {
        Red => 1,
        _   => 0,
    };
}
",
    );
}

#[test]
fn non_exhaustive_match_is_rejected_at_compile_time() {
    let cs = codes(
        "\
type Color = Red | Green | Blue
fn code(c: Color) -> Int {
    return match c {
        Red   => 1,
        Green => 2,
    };
}
",
    );
    assert_eq!(cs, vec!["T0006"], "Blue is uncovered");
}

#[test]
fn non_exhaustive_diagnostic_names_the_missing_variant() {
    let parsed = writ_parser::parse(
        "\
type Color = Red | Green | Blue
fn code(c: Color) -> Int { return match c { Red => 1 }; }
",
    );
    let diags = writ_check::check_types(&parsed.module);
    let d = diags
        .iter()
        .find(|d| d.code == "T0006")
        .expect("a non-exhaustive diagnostic");
    assert!(d.message.contains("`Green`"), "{}", d.message);
    assert!(d.message.contains("`Blue`"), "{}", d.message);
}

#[test]
fn constructor_arity_mismatch_is_rejected() {
    let cs = codes("type Pair = Pair(Int, Int)\nfn f() -> Pair { return Pair(1); }");
    assert_eq!(cs, vec!["T0004"]);
}

#[test]
fn match_arms_must_agree_on_a_type() {
    let cs = codes(
        "\
type Option = Some(Int) | None
fn f(o: Option) -> Int {
    return match o {
        Some(x) => 1,
        None    => true,
    };
}
",
    );
    assert!(
        cs.contains(&"T0001".to_string()),
        "expected a type mismatch, got {cs:?}"
    );
}

#[test]
fn diagnostics_are_deterministic() {
    let src = "fn f() { let x: Int = true; if 1 { return; } }";
    assert_eq!(codes(src), codes(src));
}
