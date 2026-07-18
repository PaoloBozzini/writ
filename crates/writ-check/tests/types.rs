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

// --- Generic instantiation of variant payloads (#78) -----------------------

#[test]
fn generic_constructor_infers_type_argument() {
    // `Some(3)` is `Option<Int>`; `Some("x")` is `Option<Text>`.
    assert_ok(
        "\
type Option<T> = Some(T) | None
fn f() {
    let a: Option<Int>  = Some(3);
    let b: Option<Text> = Some(\"x\");
}
",
    );
}

#[test]
fn nullary_generic_constructor_is_polymorphic() {
    // `None` has no field to fix `T`, so it fits any `Option<_>`.
    assert_ok(
        "\
type Option<T> = Some(T) | None
fn f() {
    let a: Option<Int>  = None;
    let b: Option<Text> = None;
}
",
    );
}

#[test]
fn generic_constructor_with_wrong_payload_is_rejected() {
    // `Some(\"x\")` is `Option<Text>`, not the annotated `Option<Int>`.
    let cs = codes(
        "\
type Option<T> = Some(T) | None
fn f() {
    let a: Option<Int> = Some(\"x\");
}
",
    );
    assert_eq!(cs, vec!["T0001"], "Option<Text> is not Option<Int>");
}

#[test]
fn concrete_variant_payload_type_is_checked() {
    // Non-generic payload: `W(Int)` given a `Text` must be refused.
    let cs = codes("type Wrap = W(Int)\nfn f() -> Wrap { return W(\"x\"); }");
    assert_eq!(cs, vec!["T0001"], "W expects an Int payload");
}

#[test]
fn match_binds_payload_at_instantiated_type() {
    // In `match o { Some(x) => .. }` with `o: Option<Int>`, `x` is `Int`.
    assert_ok(
        "\
type Option<T> = Some(T) | None
fn unwrap_or(o: Option<Int>, fallback: Int) -> Int {
    return match o {
        Some(x) => x,
        None    => fallback,
    };
}
",
    );
}

#[test]
fn match_payload_binding_is_not_opaque() {
    // `x` is precisely `Int`, so returning it where `Text` is expected fails.
    // An opaque binding would silently type-check.
    let cs = codes(
        "\
type Option<T> = Some(T) | None
fn f(o: Option<Int>) -> Text {
    return match o {
        Some(x) => x,
        None    => \"n\",
    };
}
",
    );
    assert!(
        cs.contains(&"T0001".to_string()),
        "Int payload must not unify with Text, got {cs:?}"
    );
}

#[test]
fn mismatched_generic_argument_is_rejected_at_a_call() {
    // Passing `Option<Int>` where `Option<Text>` is required must be refused.
    let cs = codes(
        "\
type Option<T> = Some(T) | None
fn need(o: Option<Text>) {}
fn f() { need(Some(3)); }
",
    );
    assert_eq!(cs, vec!["T0001"], "Option<Int> is not Option<Text>");
}

#[test]
fn diagnostics_are_deterministic() {
    let src = "fn f() { let x: Int = true; if 1 { return; } }";
    assert_eq!(codes(src), codes(src));
}

// --- Duplicate top-level names (#101) -------------------------------------

#[test]
fn duplicate_function_names_are_a_static_error() {
    let cs = codes(
        "fn f() -> Int { return 1; }\nfn f() -> Int { return 2; }\nfn main() { print(f()); }",
    );
    assert_eq!(cs, vec!["T0010"], "second `f` is refused");
}

#[test]
fn duplicate_variant_names_are_a_static_error() {
    // Same variant name in two type declarations.
    let cs = codes("type A = Dup | X\ntype B = Dup | Y\nfn main() {}");
    assert_eq!(cs, vec!["T0011"], "second `Dup` is refused");
}

#[test]
fn duplicate_variant_within_one_type_is_a_static_error() {
    let cs = codes("type A = Dup | Dup\nfn main() {}");
    assert_eq!(cs, vec!["T0011"]);
}

// --- Match patterns must belong to the scrutinee's type (#102) ------------

#[test]
fn an_alien_variant_pattern_is_rejected() {
    // `Some` belongs to `Option`, not `Color`; matching it on a `Color` is a
    // compile error even with a catch-all present.
    let cs = codes(
        "\
type Color = Red | Green | Blue
type Option = Some(Int) | None
fn f(c: Color) -> Int {
    return match c {
        Some(x) => x,
        _ => 0,
    };
}
",
    );
    assert_eq!(cs, vec!["T0012"], "alien Some pattern on a Color");
}

#[test]
fn an_alien_nullary_pattern_is_rejected() {
    let cs = codes(
        "\
type Color = Red | Green | Blue
type Flag = On | Off
fn f(c: Color) -> Int {
    return match c {
        Red => 1,
        On  => 2,
        _   => 0,
    };
}
",
    );
    assert_eq!(cs, vec!["T0012"], "alien On pattern on a Color");
}

#[test]
fn native_patterns_of_the_scrutinee_type_still_check() {
    assert_ok(
        "\
type Option<T> = Some(T) | None
fn f(o: Option<Int>) -> Int {
    return match o {
        Some(x) => x,
        None    => 0,
    };
}
",
    );
}

// --- Interpreter backstops as static checks (#104) ------------------------

#[test]
fn a_duplicate_pattern_binder_is_rejected() {
    let cs = codes(
        "\
type Pair = P(Int, Int)
fn f(p: Pair) -> Int { return match p { P(x, x) => x }; }
",
    );
    assert_eq!(cs, vec!["T0013"], "P(x, x) binds `x` twice");
}

#[test]
fn a_non_capability_main_parameter_is_rejected() {
    let cs = codes("fn main(n: Int) { print(n); }");
    assert_eq!(cs, vec!["T0014"], "main may take only capabilities");
}

#[test]
fn a_capability_main_parameter_is_allowed() {
    assert_ok("fn main(root: Cap<Root>) { return; }");
}

#[test]
fn a_no_argument_main_is_allowed() {
    assert_ok("fn main() { return; }");
}

// --- Text built-ins (#122) ------------------------------------------------

#[test]
fn text_builtins_type_check() {
    assert_ok(
        "\
fn f() -> Int {
    let s = concat(\"a\", \"b\");
    let c = char_at(s, 0);
    let sub = substring(s, 0, 1);
    return text_len(concat(c, sub));
}
",
    );
}

#[test]
fn a_text_builtin_with_a_wrong_argument_type_is_rejected() {
    let cs = codes("fn f() -> Int { return text_len(42); }");
    assert_eq!(cs, vec!["T0001"], "text_len needs Text");
}

#[test]
fn char_at_needs_an_int_index() {
    let cs = codes("fn f() -> Text { return char_at(\"a\", \"b\"); }");
    assert_eq!(cs, vec!["T0001"], "index must be Int");
}

#[test]
fn char_code_and_code_char_type_check() {
    assert_ok("fn f() -> Text { return code_char(char_code(char_at(\"ab\", 0))); }");
}

// --- Higher-order functions (#124) ----------------------------------------

#[test]
fn a_pure_function_can_be_passed_and_called() {
    assert_ok(
        "fn apply(f: fn(Int) -> Int, x: Int) -> Int { return f(x); }\n\
         fn inc(n: Int) -> Int { return n + 1; }\n\
         fn main() { print(apply(inc, 5)); }",
    );
}

#[test]
fn an_effectful_function_cannot_be_used_as_a_value() {
    let cs = codes(
        "fn logit(out: Cap<Write>, n: Int) -> Int uses { Write } { return n; }\n\
         fn apply(f: fn(Cap<Write>, Int) -> Int, out: Cap<Write>, x: Int) -> Int { return f(out, x); }\n\
         fn main(root: Cap<Root>) uses { Write } { print(apply(logit, grant<Write>(root), 5)); }",
    );
    assert!(
        cs.contains(&"T0015".to_string()),
        "effectful fn as value: {cs:?}"
    );
}

#[test]
fn passing_a_wrongly_typed_function_is_rejected() {
    let cs = codes(
        "fn apply(f: fn(Int) -> Int, x: Int) -> Int { return f(x); }\n\
         fn to_len(s: Text) -> Int { return 0; }\n\
         fn main() { print(apply(to_len, 5)); }",
    );
    assert_eq!(cs, vec!["T0001"]);
}

#[test]
fn calling_a_function_value_checks_its_arguments() {
    let cs = codes("fn apply(f: fn(Int) -> Int) -> Int { return f(true); }\nfn main() {}");
    assert_eq!(cs, vec!["T0001"]);
}
